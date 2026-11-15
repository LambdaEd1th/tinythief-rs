//! winit + wgpu host that drives a `ruffle_core::Player`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use ruffle_core::backend::audio::{AudioBackend, NullAudioBackend};
use ruffle_core::backend::navigator::OwnedFuture;
use ruffle_core::backend::storage::MemoryStorageBackend;
use ruffle_core::backend::storage::StorageBackend;
use ruffle_core::backend::ui::NullUiBackend;
use ruffle_core::config::Letterbox;
use ruffle_core::loader::Error;
use ruffle_core::tag_utils::SwfMovie;
use ruffle_core::{FloatDuration, Player, PlayerBuilder, PlayerEvent, StageAlign, StageScaleMode};
use ruffle_frontend_utils::backends::audio::CpalAudioBackend;
use ruffle_frontend_utils::backends::storage::DiskStorageBackend;
use ruffle_render::quality::StageQuality;
use ruffle_render_wgpu::backend::WgpuRenderBackend;
use ruffle_render_wgpu::backend::{create_wgpu_instance, request_adapter_and_device};
use ruffle_render_wgpu::descriptors::Descriptors;
use ruffle_render_wgpu::target::{SwapChainTarget, TextureTarget};
use ruffle_render_wgpu::wgpu;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::navigator::BundleNavigator;
use tt_assets::AppBundle;
use tt_bridge::Bridge;
use tt_platform::Platform;

const START_WIDTH: u32 = 1024;
const START_HEIGHT: u32 = 768;

/// Deliver queued AS3 calls (purchase results, product info) to the root clip.
pub(crate) fn drain_outbox(player: &mut Player, outbox: &tt_bridge::Outbox) {
    let calls: Vec<_> = {
        let mut guard = outbox.lock().unwrap();
        std::mem::take(&mut *guard)
    };
    for call in calls {
        tracing::debug!(name = call.name, "invoking root AS3 method");
        player.call_root_method(call.name, call.args);
    }
}

/// Real audio via cpal, falling back to silence when no device is available.
fn make_audio() -> Box<dyn AudioBackend> {
    if std::env::var_os("TT_NO_AUDIO").is_some() {
        tracing::info!("audio disabled by TT_NO_AUDIO");
        return Box::new(NullAudioBackend::new());
    }
    match CpalAudioBackend::new(None) {
        Ok(backend) => {
            tracing::info!("cpal audio backend ready");
            Box::new(backend)
        }
        Err(e) => {
            tracing::warn!("audio disabled ({e}); using null backend");
            Box::new(NullAudioBackend::new())
        }
    }
}

/// Wraps a storage backend, collapsing Ruffle's URL-derived keys
/// (`localhost/Volumes/…/TinyThief.swf/save.sol`) into `<movie>/<save>.sol`.
struct NormalizedStorage {
    inner: Box<dyn StorageBackend>,
}

impl NormalizedStorage {
    fn key(name: &str) -> String {
        let mut parts: Vec<&str> = name.rsplit('/').take(2).collect();
        parts.reverse();
        parts.join("/")
    }
}

impl StorageBackend for NormalizedStorage {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.inner.get(&Self::key(name))
    }
    fn put(&mut self, name: &str, value: &[u8]) -> bool {
        self.inner.put(&Self::key(name), value)
    }
    fn get_size(&self, name: &str) -> Option<usize> {
        self.inner.get_size(&Self::key(name))
    }
    fn remove_key(&mut self, name: &str) {
        self.inner.remove_key(&Self::key(name))
    }
}

/// SharedObject store. `TT_SAVE_DIR` overrides the default `./saves`.
fn make_storage() -> Box<dyn StorageBackend> {
    let dir = std::env::var_os("TT_SAVE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("saves"));
    match std::fs::create_dir_all(&dir) {
        Ok(()) => {
            tracing::info!(dir = %dir.display(), "shared objects stored on disk");
            Box::new(NormalizedStorage {
                inner: Box::new(DiskStorageBackend::new(dir)),
            })
        }
        Err(e) => {
            tracing::warn!(
                "cannot create {}: {e}; saves are in-memory only",
                dir.display()
            );
            Box::new(MemoryStorageBackend::new())
        }
    }
}

struct Gfx {
    window: Arc<Window>,
    player: Arc<Mutex<Player>>,
    outbox: tt_bridge::Outbox,
    /// Lifecycle pause: equivalent to the original host calling `_root.onPaused`.
    /// Ruffle has no direct AS3-call API, so we freeze the player instead.
    paused: bool,
    pending: Arc<Mutex<Vec<OwnedFuture<(), Error>>>>,
    last_tick: Instant,
    frames: u64,
}

pub struct Host {
    bundle: AppBundle,
    boot_movie: String,
    quality: StageQuality,
    always_render: bool,
    frame_rate: Option<f64>,
    platform: Platform,
    gfx: Option<Gfx>,
    /// Stage coordinate mapping: (x + offset) * scale.
    stage_scale: f64,
    stage_offset_x: f64,
    stage_offset_y: f64,
    cursor: (f64, f64),
}

impl Host {
    pub fn new(
        bundle: AppBundle,
        platform: Platform,
        boot_movie: String,
        quality: StageQuality,
        always_render: bool,
        frame_rate: Option<f64>,
    ) -> Self {
        Self {
            bundle,
            boot_movie,
            quality,
            always_render,
            frame_rate,
            platform,
            gfx: None,
            stage_scale: 1.0,
            stage_offset_x: 0.0,
            stage_offset_y: 0.0,
            cursor: (0.0, 0.0),
        }
    }

    fn init_gfx(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title("Tiny Thief — Rust host")
            .with_inner_size(winit::dpi::LogicalSize::new(START_WIDTH, START_HEIGHT));
        let window = Arc::new(event_loop.create_window(attrs)?);

        let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        let instance = create_wgpu_instance(backends, wgpu::BackendOptions::default(), None);
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| anyhow!("create surface: {e}"))?;
        let (adapter, device, queue) = futures::executor::block_on(request_adapter_and_device(
            backends,
            &instance,
            Some(&surface),
            wgpu::PowerPreference::HighPerformance,
        ))
        .map_err(|e| anyhow!("request adapter: {e}"))?;

        tracing::info!(
            backend = ?adapter.get_info().backend,
            device = %adapter.get_info().name,
            "wgpu device ready"
        );

        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let descriptors = Arc::new(Descriptors::new(instance, adapter, device, queue));
        let target = SwapChainTarget::new(
            surface,
            &descriptors.adapter,
            (width, height),
            &descriptors.device,
        );
        let renderer = WgpuRenderBackend::new(descriptors.clone(), target)
            .map_err(|e| anyhow!("render backend: {e}"))?;

        let navigator = BundleNavigator::new(AppBundle::open(self.bundle.root())?)
            .context("build navigator")?;
        let pending = navigator.pending();
        let swf = self
            .bundle
            .read(&self.boot_movie)
            .context("read boot movie")?;
        let movie_url = navigator.url_for(&self.boot_movie);
        let movie = SwfMovie::from_data(&swf, movie_url, None, None)
            .map_err(|e| anyhow!("parse SWF: {e}"))?;
        tracing::info!(
            width = movie.width().to_pixels(),
            height = movie.height().to_pixels(),
            frame_rate = ?movie.frame_rate(),
            version = movie.header().version(),
            "movie loaded"
        );

        let ui_scale = window.scale_factor();
        let outbox: tt_bridge::Outbox = Default::default();
        let player = PlayerBuilder::new()
            .with_movie(movie)
            .with_renderer(renderer)
            .with_navigator(navigator)
            .with_boxed_audio(make_audio())
            .with_storage(make_storage())
            .with_ui(NullUiBackend::new())
            .with_external_interface(Box::new(Bridge::with_outbox(
                self.platform.clone(),
                outbox.clone(),
            )))
            .with_fs_commands(Box::new(Bridge::with_outbox(
                self.platform.clone(),
                outbox.clone(),
            )))
            .with_viewport_dimensions(width, height, ui_scale)
            .with_scale_mode(StageScaleMode::ShowAll, true)
            .with_align(StageAlign::TOP | StageAlign::LEFT, true)
            .with_letterbox(Letterbox::On)
            .with_quality(self.quality)
            .with_frame_rate(self.frame_rate)
            .with_autoplay(true)
            .build();

        self.stage_scale = (width as f64 / START_WIDTH as f64)
            .min(height as f64 / START_HEIGHT as f64)
            .max(0.0001);
        self.stage_offset_x = (width as f64 - START_WIDTH as f64 * self.stage_scale) / 2.0;
        self.stage_offset_y = (height as f64 - START_HEIGHT as f64 * self.stage_scale) / 2.0;

        tracing::info!(width, height, ui_scale, "player ready");
        self.gfx = Some(Gfx {
            window,
            player,
            outbox,
            paused: false,
            pending,
            last_tick: Instant::now(),
            frames: 0,
        });
        Ok(())
    }

    fn to_stage(&self, x: f64, y: f64) -> (f64, f64) {
        let s = self.stage_scale;
        ((x - self.stage_offset_x) / s, (y - self.stage_offset_y) / s)
    }

    fn drive_pending(gfx: &Gfx) {
        let futures: Vec<_> = {
            let mut guard = gfx.pending.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for f in futures {
            if let Err(e) = futures::executor::block_on(f) {
                tracing::warn!("spawned future failed: {e}");
            }
        }
    }

    fn redraw(&mut self) {
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        Self::drive_pending(gfx);

        let now = Instant::now();
        let dt = now
            .duration_since(gfx.last_tick)
            .min(Duration::from_millis(100));
        gfx.last_tick = now;

        let mut player = gfx.player.lock().unwrap();
        if !gfx.paused {
            player.tick(FloatDuration::from_std(dt));
        }
        drain_outbox(&mut player, &gfx.outbox);
        // Ruffle flags `needs_render` when a SWF frame actually advanced or input
        // changed the display list; the movie runs at 24 fps, so skipping the
        // other rAF frames avoids ~60% of the render work.
        if self.always_render || player.needs_render() {
            player.render();
        }
        gfx.frames += 1;
        if gfx.frames <= 5 || gfx.frames % 120 == 0 {
            tracing::debug!(
                frame = gfx.frames,
                dt_ms = dt.as_secs_f64() * 1000.0,
                playing = player.is_playing(),
                stage = %player.renderer().debug_info(),
                "frame"
            );
        }
    }
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        if let Err(e) = self.init_gfx(event_loop) {
            tracing::error!("failed to start: {e:#}");
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gfx) = self.gfx.as_ref() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) | WindowEvent::Occluded(true) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    if !gfx.paused {
                        gfx.paused = true;
                        let mut player = gfx.player.lock().unwrap();
                        player.call_root_method("onPaused", Vec::new());
                        tracing::info!("lifecycle pause (AS3 onPaused + frozen)");
                    }
                }
            }
            WindowEvent::Focused(true) | WindowEvent::Occluded(false) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    if gfx.paused {
                        gfx.paused = false;
                        gfx.last_tick = Instant::now();
                        let mut player = gfx.player.lock().unwrap();
                        player.call_root_method("onResumed", Vec::new());
                        tracing::info!("lifecycle resume (AS3 onResumed)");
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(size) => {
                tracing::debug!(width = size.width, height = size.height, "resized");
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = self.to_stage(position.x, position.y);
                self.cursor = (x, y);
                gfx.player
                    .lock()
                    .unwrap()
                    .handle_event(PlayerEvent::MouseMove { x, y });
            }
            WindowEvent::CursorLeft { .. } => {
                gfx.player
                    .lock()
                    .unwrap()
                    .handle_event(PlayerEvent::MouseLeave);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = self.cursor;
                let button = match button {
                    WinitMouseButton::Left => ruffle_core::events::MouseButton::Left,
                    WinitMouseButton::Right => ruffle_core::events::MouseButton::Right,
                    WinitMouseButton::Middle => ruffle_core::events::MouseButton::Middle,
                    _ => return,
                };
                let mut player = gfx.player.lock().unwrap();
                let event = match state {
                    ElementState::Pressed => PlayerEvent::MouseDown {
                        x,
                        y,
                        button,
                        index: None,
                    },
                    ElementState::Released => PlayerEvent::MouseUp { x, y, button },
                };
                player.handle_event(event);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Tiny Thief is touch-driven; only Escape matters for the desktop host.
                if event.state == ElementState::Pressed {
                    if let Key::Named(NamedKey::Escape) = event.logical_key {
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gfx) = self.gfx.as_mut() {
            gfx.paused = true;
            tracing::info!("lifecycle pause (suspended)");
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
    }
}

/// Headless golden-frame capture: renders `frames` ticks offscreen and writes a PNG.
/// Used for CI/screenshot verification so we do not need a window at all.
#[derive(Debug, Clone, Copy)]
pub struct Click {
    pub x: f64,
    pub y: f64,
    pub frame: u32,
}

pub struct CaptureOptions {
    pub frames: u32,
    pub size: (u32, u32),
    pub clicks: Vec<Click>,
    /// Simulate the app lifecycle pause: do not tick between these frames.
    pub pause: Option<(u32, u32)>,
    /// Extra snapshots: (frame, path).
    pub snapshots: Vec<(u32, std::path::PathBuf)>,
}

pub fn run_capture(
    bundle: &AppBundle,
    boot_movie: &str,
    platform: Platform,
    opts: &CaptureOptions,
    out: &std::path::Path,
) -> Result<()> {
    let CaptureOptions {
        frames,
        size: (width, height),
        ref clicks,
        pause,
        ref snapshots,
    } = *opts;
    use std::any::Any;

    let backends = wgpu::Backends::PRIMARY;
    let instance = create_wgpu_instance(backends, wgpu::BackendOptions::default(), None);
    let (adapter, device, queue) = futures::executor::block_on(request_adapter_and_device(
        backends,
        &instance,
        None,
        wgpu::PowerPreference::HighPerformance,
    ))
    .map_err(|e| anyhow!("request adapter: {e}"))?;
    let descriptors = Arc::new(Descriptors::new(instance, adapter, device, queue));
    let target = TextureTarget::new(&descriptors.device, (width, height))
        .map_err(|e| anyhow!("texture target: {e}"))?;
    let renderer = WgpuRenderBackend::new(descriptors.clone(), target)
        .map_err(|e| anyhow!("render backend: {e}"))?;

    let navigator = BundleNavigator::new(AppBundle::open(bundle.root())?)?;
    let pending = navigator.pending();
    let swf = bundle.read(boot_movie)?;
    let movie = SwfMovie::from_data(&swf, navigator.url_for(boot_movie), None, None)
        .map_err(|e| anyhow!("parse SWF: {e}"))?;

    let outbox: tt_bridge::Outbox = Default::default();
    let player = PlayerBuilder::new()
        .with_movie(movie)
        .with_renderer(renderer)
        .with_navigator(navigator)
        .with_boxed_audio(make_audio())
        .with_storage(make_storage())
        .with_ui(NullUiBackend::new())
        .with_external_interface(Box::new(Bridge::with_outbox(
            platform.clone(),
            outbox.clone(),
        )))
        .with_fs_commands(Box::new(Bridge::with_outbox(platform, outbox.clone())))
        .with_viewport_dimensions(width, height, 1.0)
        .with_scale_mode(StageScaleMode::ShowAll, true)
        .with_align(StageAlign::TOP | StageAlign::LEFT, true)
        .with_letterbox(Letterbox::On)
        .with_autoplay(true)
        .build();

    for frame in 0..frames {
        if frame % 200 == 0 {
            let p = player.lock().unwrap();
            tracing::info!(frame, current_frame = ?p.current_frame(), "capture progress");
        }
        for click in clicks {
            let mut p = player.lock().unwrap();
            if frame == click.frame {
                p.handle_event(PlayerEvent::MouseMove {
                    x: click.x,
                    y: click.y,
                });
                p.handle_event(PlayerEvent::MouseDown {
                    x: click.x,
                    y: click.y,
                    button: ruffle_core::events::MouseButton::Left,
                    index: None,
                });
                tracing::info!(x = click.x, y = click.y, frame, "synthetic mouse down");
            } else if frame == click.frame + 4 {
                p.handle_event(PlayerEvent::MouseUp {
                    x: click.x,
                    y: click.y,
                    button: ruffle_core::events::MouseButton::Left,
                });
                tracing::info!(x = click.x, y = click.y, frame, "synthetic mouse up");
            }
        }
        let futs: Vec<_> = {
            let mut guard = pending.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for f in futs {
            if let Err(e) = futures::executor::block_on(f) {
                tracing::warn!("spawned future failed: {e}");
            }
        }
        let paused = matches!(pause, Some((from, to)) if frame >= from && frame < to);
        {
            let mut p = player.lock().unwrap();
            if !paused {
                // Finish any pending movie/sound loading first: keeps captures reproducible.
                p.preload(&mut ruffle_core::limits::ExecutionLimit::none());
                p.tick(FloatDuration::from_secs(1.0 / 30.0));
            }
            drain_outbox(&mut p, &outbox);
            p.render();
        }
        if let Some((_, path)) = snapshots.iter().find(|(f, _)| *f == frame) {
            let image = {
                let p = player.lock().unwrap();
                let any: &dyn Any = p.renderer();
                any.downcast_ref::<WgpuRenderBackend<TextureTarget>>()
                    .and_then(|r| r.capture_frame())
            };
            if let Some(image) = image {
                image
                    .save(path)
                    .map_err(|e| anyhow!("save snapshot: {e}"))?;
                tracing::info!(frame, path = %path.display(), "snapshot");
            }
        }
    }

    let image = {
        let p = player.lock().unwrap();
        let any: &dyn Any = p.renderer();
        any.downcast_ref::<WgpuRenderBackend<TextureTarget>>()
            .and_then(|r| r.capture_frame())
    };
    let image = image.ok_or_else(|| anyhow!("no frame captured"))?;
    image.save(out).map_err(|e| anyhow!("save png: {e}"))?;
    tracing::info!(path = %out.display(), width, height, frames, "captured frame");
    Ok(())
}
