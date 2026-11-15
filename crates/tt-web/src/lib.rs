#![cfg(target_family = "wasm")]
//! Browser (WebAssembly) host for Tiny Thief.
//!
//! Entry point: [`TinyThiefHost::create`] — build a Ruffle player that renders
//! into a `<canvas>` and is driven by `requestAnimationFrame` from JS.
//!
//! Assets are fetched over HTTP and decrypted in memory (`tt-assets`), so the
//! encrypted app bundle can be served as static files (e.g. GitHub Pages).

mod audio;
mod idb;
mod locale;
mod navigator;
mod storage;

use std::sync::{Arc, Mutex};

use ruffle_core::backend::navigator::OwnedFuture;
use ruffle_core::backend::ui::NullUiBackend;
use ruffle_core::config::Letterbox;
use ruffle_core::events::MouseButton;
use ruffle_core::loader::Error;
use ruffle_core::tag_utils::SwfMovie;
use ruffle_core::{FloatDuration, Player, PlayerBuilder, PlayerEvent, StageAlign, StageScaleMode};
use ruffle_render::quality::StageQuality;
use ruffle_render_wgpu::backend::WgpuRenderBackend;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

use tt_bridge::Bridge;

use crate::locale::WebLocale;

use crate::audio::{AudioHandle, WebAudioBackend};
use crate::idb::IdbStorage;
use crate::navigator::WebNavigator;
use crate::storage::LocalStorageBackend;

const STAGE_WIDTH: u32 = 1024;
const STAGE_HEIGHT: u32 = 768;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();

    // Keep the console quiet by default; `?log=debug` / `?log=trace` opt in.
    let level = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|search| match log_level_param(&search) {
            Some("trace") => tracing::Level::TRACE,
            Some("debug") => tracing::Level::DEBUG,
            _ => tracing::Level::INFO,
        })
        .unwrap_or(tracing::Level::INFO);
    tracing_wasm::set_as_global_default_with_config(
        tracing_wasm::WASMLayerConfigBuilder::new()
            .set_max_level(level)
            .build(),
    );
    tracing::info!("tt-web booted");
}

fn log_level_param(search: &str) -> Option<&str> {
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("log="))
}

/// Owns the player; created from JS with [`TinyThiefHost::create`].
#[wasm_bindgen]
pub struct TinyThiefHost {
    player: Arc<Mutex<Player>>,
    audio_handle: Option<AudioHandle>,
    outbox: tt_bridge::Outbox,
    paused: bool,
    started: bool,
    always_render: bool,
    pending_move: Option<(f64, f64)>,
    ticks: u64,
    renders: u64,
}

#[wasm_bindgen]
impl TinyThiefHost {
    /// Build the host: fetch + decrypt the boot movie, set up WebGL rendering
    /// and the AS3 bridge. `base_url` is where the app bundle is served from
    /// (e.g. `assets/`).
    #[wasm_bindgen]
    pub async fn create(
        canvas: web_sys::HtmlCanvasElement,
        base_url: String,
        quality: Option<String>,
        always_render: Option<bool>,
        frame_rate: Option<f64>,
        mute: Option<bool>,
    ) -> Result<TinyThiefHost, JsValue> {
        let mute = mute.unwrap_or(false);
        let quality = quality.as_deref().map(parse_quality).unwrap_or_default();
        let always_render = always_render.unwrap_or(false);
        console_error_panic_hook::set_once();

        canvas.set_width(STAGE_WIDTH);
        canvas.set_height(STAGE_HEIGHT);

        let renderer = WgpuRenderBackend::for_canvas(canvas, false)
            .await
            .map_err(|e| JsValue::from_str(&format!("webgl renderer: {e}")))?;

        let navigator = WebNavigator::new(base_url);

        // Browsers create the context suspended; JS calls `resume_audio()` on the
        // first pointer/key event to satisfy the autoplay policy.
        let (audio_backend, audio_handle) = if mute {
            (None, None)
        } else {
            let backend = WebAudioBackend::new();
            let handle = backend.handle();
            (Some(backend), Some(handle))
        };
        let (swf, url) = navigator
            .load_movie("TinyThief.swf")
            .await
            .map_err(|e| JsValue::from_str(&format!("load TinyThief.swf: {e}")))?;
        let movie = SwfMovie::from_data(&swf, url, None, None)
            .map_err(|e| JsValue::from_str(&format!("parse SWF: {e}")))?;
        tracing::info!(
            width = movie.width().to_pixels(),
            height = movie.height().to_pixels(),
            "movie loaded"
        );

        // Prefer IndexedDB; fall back to localStorage (older builds still migrate
        // their saves into IndexedDB when it is available).
        let storage: Option<Box<dyn ruffle_core::backend::storage::StorageBackend>> =
            match IdbStorage::open().await {
                Ok(idb) => Some(Box::new(idb)),
                Err(e) => {
                    tracing::warn!("indexeddb unavailable ({e:?}); falling back to localStorage");
                    web_sys::window()
                        .and_then(|w| w.local_storage().ok().flatten())
                        .map(|ls| {
                            Box::new(LocalStorageBackend::new(ls))
                                as Box<dyn ruffle_core::backend::storage::StorageBackend>
                        })
                }
            };

        let platform = tt_platform::Platform::null();
        let outbox: tt_bridge::Outbox = Default::default();

        let mut builder = PlayerBuilder::new()
            .with_frame_rate(frame_rate)
            .with_movie(movie)
            .with_renderer(renderer)
            .with_navigator(navigator)
            .with_boxed_audio(match audio_backend {
                Some(backend) => Box::new(backend),
                None => Box::new(ruffle_core::backend::audio::NullAudioBackend::new()),
            })
            .with_ui(NullUiBackend::new())
            .with_locale(WebLocale)
            .with_external_interface(Box::new(Bridge::with_outbox(
                platform.clone(),
                outbox.clone(),
            )))
            .with_fs_commands(Box::new(Bridge::with_outbox(platform, outbox.clone())))
            .with_viewport_dimensions(STAGE_WIDTH, STAGE_HEIGHT, 1.0)
            .with_scale_mode(StageScaleMode::ShowAll, true)
            .with_align(StageAlign::TOP | StageAlign::LEFT, true)
            .with_letterbox(Letterbox::On)
            .with_quality(quality)
            .with_autoplay(true);

        if let Some(storage) = storage {
            builder = builder.with_storage(storage);
        }

        let player = builder.build();

        // `for_canvas` creates the swap chain at 1x1; tell the player the real
        // stage size so the surface gets configured before the first frame.
        {
            let mut player_lock = player.lock().unwrap();
            player_lock.set_viewport_dimensions(ruffle_core::ViewportDimensions {
                width: STAGE_WIDTH,
                height: STAGE_HEIGHT,
                scale_factor: 1.0,
            });
        }
        tracing::info!("player ready");

        let host = TinyThiefHost {
            player,
            audio_handle,
            outbox,
            paused: false,
            started: false,
            always_render,
            pending_move: None,
            ticks: 0,
            renders: 0,
        };

        // Draw one frame immediately so the canvas is not blank before rAF runs.
        let mut host = host;
        host.tick(16.0);
        Ok(host)
    }

    /// Advance one frame. `dt_ms` comes from the JS `requestAnimationFrame` loop.
    pub fn tick(&mut self, dt_ms: f64) {
        if self.paused {
            return;
        }
        let dt = FloatDuration::from_secs((dt_ms.clamp(1.0, 100.0)) / 1000.0);
        let mut player = self.player.lock().unwrap();

        // Coalesce pointer moves: at most one hover update per frame.
        if let Some((x, y)) = self.pending_move.take() {
            player.handle_event(PlayerEvent::MouseMove { x, y });
        }

        player.tick(dt);
        self.ticks += 1;
        drain_outbox(&mut player, &self.outbox);

        // Render only when the movie advanced (24 fps) or input changed the
        // display list; rAF runs at ~60 fps, so this skips most frames.
        if self.always_render || !self.started || player.needs_render() {
            player.render();
            self.renders += 1;
            self.started = true;
        }
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    /// Start/resume audio after a user gesture.
    ///
    /// Safari only allows an `AudioContext` that was created inside the gesture
    /// handler to play, so the backend creates the context here rather than at
    /// startup (`Player::unlock_audio`).
    pub fn resume_audio(&mut self) {
        let mut player = self.player.lock().unwrap();
        player.unlock_audio();
    }

    /// `AudioContext.state` ("not-started" / "suspended" / "running") for debugging.
    pub fn audio_state(&self) -> String {
        self.audio_handle
            .as_ref()
            .map(AudioHandle::state)
            .unwrap_or_else(|| "none".to_string())
    }

    /// `AudioContext.currentTime`, for smoke tests (advances only while running).
    pub fn audio_time(&self) -> f64 {
        self.audio_handle
            .as_ref()
            .map(AudioHandle::current_time)
            .unwrap_or(0.0)
    }

    /// Peak sample amplitude mixed since the previous call (0.0 = silence).
    pub fn audio_peak(&self) -> f64 {
        f64::from(crate::audio::take_peak())
    }

    /// Number of `tick()` calls since startup (for profiling).
    pub fn tick_count(&self) -> u32 {
        self.ticks.min(u32::MAX as u64) as u32
    }

    /// Number of actual renders since startup (for profiling).
    pub fn render_count(&self) -> u32 {
        self.renders.min(u32::MAX as u64) as u32
    }

    /// Pause/resume: the original host calls AS3 `onPaused` / `onResumed`.
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        let mut player = self.player.lock().unwrap();
        if paused {
            player.call_root_method("onPaused", Vec::new());
            tracing::info!("paused (AS3 onPaused)");
        } else {
            player.call_root_method("onResumed", Vec::new());
            tracing::info!("resumed (AS3 onResumed)");
        }
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.pending_move = Some((x, y));
    }

    pub fn mouse_down(&mut self, x: f64, y: f64) {
        let mut player = self.player.lock().unwrap();
        player.handle_event(PlayerEvent::MouseMove { x, y });
        player.handle_event(PlayerEvent::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            index: None,
        });
    }

    pub fn mouse_up(&mut self, x: f64, y: f64) {
        let mut player = self.player.lock().unwrap();
        player.handle_event(PlayerEvent::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        });
    }
}

fn drain_outbox(player: &mut Player, outbox: &tt_bridge::Outbox) {
    let calls: Vec<_> = {
        let mut guard = outbox.lock().unwrap();
        std::mem::take(&mut *guard)
    };
    for call in calls {
        tracing::debug!(name = call.name, "invoking root AS3 method");
        player.call_root_method(call.name, call.args);
    }
}

/// Kept for parity with the native host's spawn-future plumbing.
#[allow(dead_code)]
pub(crate) fn spawn(fut: OwnedFuture<(), Error>) {
    spawn_local(async move {
        if let Err(e) = fut.await {
            tracing::warn!("spawned future failed: {e}");
        }
    });
}

#[allow(dead_code)]
async fn fetch_js(url: String) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_str(&url))
        .await
        .map_err(|e| format!("fetch {url}: {e:?}"))?;
    let resp: web_sys::Response = resp_value.dyn_into().map_err(|_| "not a Response")?;
    if !resp.ok() {
        return Err(format!("fetch {url}: HTTP {}", resp.status()));
    }
    let buf = JsFuture::from(resp.array_buffer().map_err(|_| "no array_buffer")?)
        .await
        .map_err(|e| format!("read body: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

fn parse_quality(value: &str) -> StageQuality {
    match value {
        "low" => StageQuality::Low,
        "medium" => StageQuality::Medium,
        "high" => StageQuality::High,
        "best" => StageQuality::Best,
        "high8x8" => StageQuality::High8x8,
        "high8x8linear" => StageQuality::High8x8Linear,
        "high16x16" => StageQuality::High16x16,
        "high16x16linear" => StageQuality::High16x16Linear,
        other => {
            tracing::warn!("unknown quality {other}; using high");
            StageQuality::High
        }
    }
}
