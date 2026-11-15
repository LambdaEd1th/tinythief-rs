//! WebAudio backend, adapted from Ruffle's `web/src/audio.rs` (MIT/Apache-2.0).
//!
//! Two ping-pong `AudioBufferSourceNode`s are scheduled ahead of the context
//! clock; each `onended` refills and reschedules its buffer with freshly mixed
//! samples. The adaptive buffer size logic is kept from upstream.
//!
//! **Browser autoplay policy.** Chrome can resume an `AudioContext` that was
//! created before a gesture, but Safari requires the context to be *created*
//! inside the gesture handler. This backend therefore starts lazily: it only
//! builds the `AudioContext` when the host calls [`AudioBackend::unlock`] from
//! a pointer/key event (`Player::unlock_audio`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ruffle_core::backend::audio::{
    swf, AudioBackend, AudioMixer, AudioMixerProxy, DecodeError, RegisterError, SoundHandle,
    SoundInstanceHandle, SoundStreamInfo, SoundTransform,
};
use ruffle_core::impl_audio_mixer_backend;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsError;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextOptions, AudioScheduledSourceNode,
};

/// The mixer always runs at 44.1 kHz; the `AudioBuffer` carries its own sample
/// rate, so the browser resamples to the device rate if needed.
const SAMPLE_RATE: f32 = 44_100.0;

/// Shared with the host so it can report `state` / `currentTime` for debugging.
#[derive(Clone, Default)]
pub struct AudioHandle {
    context: Rc<RefCell<Option<AudioContext>>>,
}

impl AudioHandle {
    pub fn state(&self) -> String {
        match self.context.borrow().as_ref() {
            Some(context) => format!("{:?}", context.state()).to_lowercase(),
            None => "not-started".to_string(),
        }
    }

    pub fn current_time(&self) -> f64 {
        self.context
            .borrow()
            .as_ref()
            .map(|context| context.current_time())
            .unwrap_or(0.0)
    }
}

/// Highest absolute sample value seen since the last [`take_peak`] call.
/// Used by the host's smoke-test hook to prove that audio is actually mixing.
static PEAK_BITS: AtomicU32 = AtomicU32::new(0);

pub fn take_peak() -> f32 {
    f32::from_bits(PEAK_BITS.swap(0, Ordering::Relaxed))
}

pub struct WebAudioBackend {
    mixer: AudioMixer,
    handle: AudioHandle,
    /// The current length of both buffers, in frames (pairs of left/right samples).
    buffer_size: Rc<Cell<u32>>,
    buffers: Vec<Rc<RefCell<Buffer>>>,
    /// When the last submitted buffer is expected to play out completely, in seconds.
    time: Rc<Cell<f64>>,
    /// For how many seconds were we able to continuously fill the next buffer "at a sufficiently early time".
    probation_elapsed: Rc<Cell<f32>>,
}

impl WebAudioBackend {
    /// These govern the adaptive buffer size algorithm, all are in number of frames (pairs of samples).
    /// They must all be integer powers of 2 (due to how the algorithm works).
    const INITIAL_BUFFER_SIZE: u32 = 2048; // 46.44 ms at 44.1 kHz
    const MIN_BUFFER_SIZE: u32 = 1024; // 23.22 ms at 44.1 kHz
    const MAX_BUFFER_SIZE: u32 = 16384; // 371.52 ms at 44.1 kHz
    /// Buffer size will not be increased until this many seconds have elapsed after startup,
    /// to account for any initialization (shape tessellation, WASM JIT, etc.) hitches.
    const WARMUP_PERIOD: f32 = 2.0;

    /// For how long we need to fill every single buffer "quickly enough" in order to decrease buffer size.
    /// Measured in seconds. A higher value is more conservative.
    const PROBATION_LENGTH: f32 = 10.0;
    /// The limit of playout ratio (progress) when filling the next buffer, under which it is
    /// considered "quick". Must be in 0..1, and less than `0.5 * NORMAL_PROGRESS_RANGE_MAX`.
    const NORMAL_PROGRESS_RANGE_MIN: f64 = 0.25;
    /// The limit of playout ratio (progress) when filling the next buffer, over which buffer size
    /// is increased immediately. Must be in 0..1, and greater than `2 * NORMAL_PROGRESS_RANGE_MIN`.
    const NORMAL_PROGRESS_RANGE_MAX: f64 = 0.75;

    pub fn new() -> Self {
        Self {
            mixer: AudioMixer::new(2, SAMPLE_RATE as u32),
            handle: AudioHandle::default(),
            buffer_size: Rc::new(Cell::new(Self::INITIAL_BUFFER_SIZE)),
            buffers: Vec::with_capacity(2),
            time: Rc::new(Cell::new(0.0)),
            probation_elapsed: Rc::new(Cell::new(0.0)),
        }
    }

    /// Handle for the host's `audio_state()` / `audio_time()` debugging hooks.
    pub fn handle(&self) -> AudioHandle {
        self.handle.clone()
    }

    /// Create the `AudioContext` and start the ping-pong buffers.
    ///
    /// Must run inside a user-gesture handler for Safari to allow playback.
    fn ensure_started(&mut self) -> Result<(), JsError> {
        if self.handle.context.borrow().is_some() {
            return Ok(());
        }

        // Pin to 44.1 kHz when supported; WebAudio resamples to the device rate.
        // Older Safari versions ignore/refuse the option, hence the fallback.
        let opts = AudioContextOptions::new();
        opts.set_sample_rate(SAMPLE_RATE);
        let context = match AudioContext::new_with_context_options(&opts) {
            Ok(context) => context,
            Err(_) => AudioContext::new().map_err(js_error)?,
        };
        let _ = context.resume();

        *self.handle.context.borrow_mut() = Some(context.clone());
        self.time.set(context.current_time());

        for _ in 0..2 {
            let buffer = Buffer::new(
                &context,
                self.mixer.proxy(),
                self.buffer_size.clone(),
                self.time.clone(),
                self.probation_elapsed.clone(),
            )?;
            buffer.borrow_mut().play()?;
            self.buffers.push(buffer);
        }
        Ok(())
    }
}

impl Default for WebAudioBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioBackend for WebAudioBackend {
    impl_audio_mixer_backend!(mixer);

    fn play(&mut self) {
        // Do not create the context here: Ruffle calls `play()` on autoplay,
        // before any user gesture. `unlock()` does the real initialization.
        if let Some(context) = self.handle.context.borrow().as_ref() {
            let _ = context.resume();
        }
    }

    fn pause(&mut self) {
        if let Some(context) = self.handle.context.borrow().as_ref() {
            let _ = context.suspend();
        }
    }

    fn unlock(&mut self) {
        if let Err(error) = self.ensure_started() {
            tracing::warn!("webaudio: failed to start: {error:?}");
        }
    }

    fn position_resolution(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            f64::from(self.buffer_size.get()) / f64::from(SAMPLE_RATE),
        ))
    }
}

impl Drop for WebAudioBackend {
    fn drop(&mut self) {
        if let Some(context) = self.handle.context.borrow().as_ref() {
            let _ = context.close();
        }
    }
}

struct Buffer {
    context: AudioContext,
    mixer_proxy: AudioMixerProxy,
    buffer_size: Rc<Cell<u32>>,
    audio_buffer: Vec<f32>,
    js_buffer: AudioBuffer,
    audio_node: Option<AudioBufferSourceNode>,
    on_ended_handler: Closure<dyn FnMut()>,
    time: Rc<Cell<f64>>,
    probation_elapsed: Rc<Cell<f32>>,
}

impl Buffer {
    fn new(
        context: &AudioContext,
        mixer_proxy: AudioMixerProxy,
        buffer_size: Rc<Cell<u32>>,
        time: Rc<Cell<f64>>,
        probation_elapsed: Rc<Cell<f32>>,
    ) -> Result<Rc<RefCell<Self>>, JsError> {
        let buffer = Rc::new(RefCell::new(Self {
            context: context.clone(),
            mixer_proxy,
            buffer_size,
            audio_buffer: vec![0.0; 2 * WebAudioBackend::INITIAL_BUFFER_SIZE as usize],
            js_buffer: context
                .create_buffer(2, WebAudioBackend::INITIAL_BUFFER_SIZE, SAMPLE_RATE)
                .map_err(js_error)?,
            audio_node: None,
            on_ended_handler: Closure::new(|| {}),
            time,
            probation_elapsed,
        }));

        // Swap in the onended handler.
        let buffer_handle = buffer.clone();
        buffer.borrow_mut().on_ended_handler = Closure::new(move || {
            // Refill and schedule the buffer for playback.
            let _ = buffer_handle.borrow_mut().play();
        });

        Ok(buffer)
    }

    fn play(&mut self) -> Result<(), JsError> {
        let time_left = self.time.get() - self.context.current_time();
        let mut buffer_timestep = f64::from(self.buffer_size.get()) / f64::from(SAMPLE_RATE);

        // How far along the other buffer is in playing out right now:
        //  ~0: it has just started playing, we are well within time
        // 0.25 .. 0.75: "optimal range"
        //  ~1: we are just barely keeping up with feeding the output
        //  >1: we are falling behind, audio stutters
        let progress = (buffer_timestep - time_left) / buffer_timestep;
        tracing::trace!(
            "Audio buffer progress when filling the next one: {}%",
            progress * 100.0
        );

        if progress < WebAudioBackend::NORMAL_PROGRESS_RANGE_MIN {
            // This fill is considered quick, let's count it.
            self.probation_elapsed
                .set(self.probation_elapsed.get() + buffer_timestep as f32);
        } else if progress < WebAudioBackend::NORMAL_PROGRESS_RANGE_MAX {
            // This fill is in the "normal" range, only resetting the probation time.
            self.probation_elapsed.set(0.0);
        } else {
            // This fill is considered slow (maybe even too slow), increasing the buffer size.
            self.probation_elapsed.set(0.0);
            if progress >= 1.0 {
                tracing::debug!("Audio underrun detected!");
            }
            if self.time.get() as f32 > WebAudioBackend::WARMUP_PERIOD {
                if self.buffer_size.get() < WebAudioBackend::MAX_BUFFER_SIZE {
                    self.buffer_size.set(self.buffer_size.get() * 2);
                    tracing::debug!(
                        "Increased audio buffer size to {} frames",
                        self.buffer_size.get()
                    );
                } else {
                    tracing::debug!("Not increasing audio buffer size, already at max size");
                }
            } else {
                tracing::debug!(
                    "Not increasing audio buffer size, still in warmup period (at {} of {} sec)",
                    self.time.get(),
                    WebAudioBackend::WARMUP_PERIOD
                );
            }
        }

        // If enough quick fills happened, we decrease the buffer size.
        if self.probation_elapsed.get() > WebAudioBackend::PROBATION_LENGTH
            && self.buffer_size.get() > WebAudioBackend::MIN_BUFFER_SIZE
        {
            self.buffer_size.set(self.buffer_size.get() / 2);
            tracing::debug!(
                "Decreased audio buffer size to {} frames",
                self.buffer_size.get()
            );
            self.probation_elapsed.set(0.0);
        }

        // In case buffer_size changed above (or in the latest call in the other instance),
        // we need to recaulculate/recreate/resize a couple of things that depend on it.
        if self.js_buffer.length() != self.buffer_size.get() {
            tracing::trace!("Recreating JS side buffer with new length");
            buffer_timestep = f64::from(self.buffer_size.get()) / f64::from(SAMPLE_RATE);
            self.js_buffer = self
                .context
                .create_buffer(2, self.buffer_size.get(), SAMPLE_RATE)
                .map_err(js_error)?;
            self.audio_buffer
                .resize(2 * self.buffer_size.get() as usize, 0.0);
        }

        // Mix new audio into the output buffer and copy to JS.
        self.mixer_proxy.mix(&mut self.audio_buffer);
        copy_to_audio_buffer_interleaved(&self.js_buffer, &self.audio_buffer);

        // Create the audio node to play back the audio buffer.
        let audio_node = self.context.create_buffer_source().map_err(js_error)?;
        audio_node.set_buffer(Some(&self.js_buffer));
        audio_node
            .connect_with_audio_node(&self.context.destination())
            .map_err(js_error)?;
        let scheduled: &AudioScheduledSourceNode = &audio_node;
        scheduled.set_onended(Some(self.on_ended_handler.as_ref().unchecked_ref()));

        // Sanity: ensure our player time is not in the past. This can happen due to underruns.
        self.time
            .set(f64::max(self.time.get(), self.context.current_time()));

        // Schedule this buffer for playback and advance the player time.
        audio_node
            .start_with_when(self.time.get())
            .map_err(js_error)?;
        self.time.set(self.time.get() + buffer_timestep);

        self.audio_node = Some(audio_node);
        Ok(())
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if let Some(audio_node) = self.audio_node.take() {
            let scheduled: &AudioScheduledSourceNode = &audio_node;
            scheduled.set_onended(None);
        }
    }
}

/// Copy interleaved stereo samples into the two channels of an `AudioBuffer`.
///
/// Uses `getChannelData` rather than `copyToChannel`: older Safari versions had
/// a broken `copyToChannel`, while `getChannelData` is universally supported.
fn copy_to_audio_buffer_interleaved(audio_buffer: &AudioBuffer, interleaved_data: &[f32]) {
    let frames = audio_buffer.length() as usize;
    let peak = interleaved_data
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    if peak > take_peak() {
        PEAK_BITS.store(peak.to_bits(), Ordering::Relaxed);
    }

    for channel in 0..2u32 {
        let mut samples = vec![0.0f32; frames];
        for (i, sample) in samples.iter_mut().enumerate() {
            *sample = interleaved_data[2 * i + channel as usize];
        }
        if let Some(data) = channel_data(audio_buffer, channel) {
            data.copy_from(&samples);
        } else {
            // Fallback for engines where the raw object access fails.
            let _ = audio_buffer.copy_to_channel(&samples, channel as i32);
        }
    }
}

/// `AudioBuffer.getChannelData(channel)` as a writable `Float32Array`.
///
/// web-sys models this method as returning a copied `Vec<f32>`, so go through
/// `Reflect` to get the live array view instead.
fn channel_data(buffer: &AudioBuffer, channel: u32) -> Option<js_sys::Float32Array> {
    let function =
        js_sys::Reflect::get(buffer.as_ref(), &JsValue::from_str("getChannelData")).ok()?;
    let function: js_sys::Function = function.dyn_into().ok()?;
    let array = function
        .call1(buffer.as_ref(), &JsValue::from_f64(f64::from(channel)))
        .ok()?;
    array.dyn_into().ok()
}

fn js_error(error: JsValue) -> JsError {
    let message = error.as_string().unwrap_or_else(|| format!("{error:?}"));
    JsError::new(&message)
}
