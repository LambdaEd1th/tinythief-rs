//! Tiny Thief — cross-platform Rust/wgpu host (Plan B: Ruffle as the AS3 engine).
//!
//! Milestones:
//!   M0  decrypting asset provider        (done, `tt-assets`)
//!   M1  window + player + boot to menu   (done)
//!   M2  native interfaces / save / input (done, platform backends are no-ops)
//!
//! Usage:
//!   cargo run -p tt-host -- ../TinyThief.app

mod host;
mod navigator;

use anyhow::{Context, Result};
use tt_assets::AppBundle;
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tt_host=debug,tt_assets=debug,ruffle_core=warn".into()),
        )
        .init();

    let app = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "../TinyThief.app".to_string());
    let bundle = AppBundle::open(&app).with_context(|| format!("open bundle {app}"))?;
    tracing::info!(root = %bundle.root().display(), "bundle opened");

    let boot_movie = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "TinyThief.swf".to_string());
    tracing::info!(%boot_movie, "boot movie");

    let quality = std::env::args()
        .position(|a| a == "--quality")
        .and_then(|i| std::env::args().nth(i + 1))
        .map(|q| parse_quality(&q))
        .unwrap_or_default();
    let always_render = std::env::args().any(|a| a == "--always-render");
    let frame_rate = std::env::args()
        .position(|a| a == "--frame-rate")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse::<f64>().ok());

    // Platform services are deliberately no-ops: the host implements the 29
    // native functions but every backend call is a logged stub, exactly like
    // Ruffle's `--dummy-external-interface` mode. Real backends (StoreKit,
    // Game Center, Play Billing, Steamworks) plug into `tt_platform::Platform`.
    let platform = tt_platform::Platform::null();

    // Headless mode: --capture out.png --frames N --size WxH [movie]
    if let Some(idx) = std::env::args().position(|a| a == "--capture") {
        let out = std::env::args().nth(idx + 1).context("--capture <png>")?;
        let frames = std::env::args()
            .position(|a| a == "--frames")
            .and_then(|i| std::env::args().nth(i + 1))
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(300);
        // --click "x,y@frame" (repeatable) or legacy --click x,y --click-frame N
        let mut clicks = Vec::new();
        let default_frame = std::env::args()
            .position(|a| a == "--click-frame")
            .and_then(|i| std::env::args().nth(i + 1))
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(240);
        let args: Vec<String> = std::env::args().collect();
        for (i, a) in args.iter().enumerate() {
            if a != "--click" {
                continue;
            }
            let Some(spec) = args.get(i + 1) else {
                continue;
            };
            let (xy, frame) = match spec.split_once('@') {
                Some((xy, f)) => (xy, f.parse::<u32>().unwrap_or(default_frame)),
                None => (spec.as_str(), default_frame),
            };
            let Some((x, y)) = xy.split_once(',') else {
                continue;
            };
            let (Ok(x), Ok(y)) = (x.trim().parse::<f64>(), y.trim().parse::<f64>()) else {
                continue;
            };
            clicks.push(host::Click { x, y, frame });
        }
        clicks.sort_by_key(|c| c.frame);
        let pause = {
            let get = |flag: &str| -> Option<u32> {
                std::env::args()
                    .position(|a| a == flag)
                    .and_then(|i| std::env::args().nth(i + 1))
                    .and_then(|v| v.parse().ok())
            };
            match (get("--pause-after"), get("--pause-frames")) {
                (Some(from), Some(len)) => Some((from, from + len)),
                _ => None,
            }
        };
        let mut snapshots = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if a != "--snapshot" {
                continue;
            }
            let Some(spec) = args.get(i + 1) else {
                continue;
            };
            if let Some((f, path)) = spec.split_once(':') {
                if let Ok(frame) = f.trim().parse::<u32>() {
                    snapshots.push((frame, std::path::PathBuf::from(path)));
                }
            }
        }
        let opts = host::CaptureOptions {
            frames,
            size: (1024, 768),
            clicks,
            pause,
            snapshots,
        };
        return host::run_capture(
            &bundle,
            &boot_movie,
            platform,
            &opts,
            std::path::Path::new(&out),
        );
    }

    let event_loop = EventLoop::new().context("create event loop")?;
    let mut host = host::Host::new(
        bundle,
        platform,
        boot_movie,
        quality,
        always_render,
        frame_rate,
    );
    event_loop.run_app(&mut host).context("run event loop")?;
    Ok(())
}

fn parse_quality(value: &str) -> ruffle_render::quality::StageQuality {
    use ruffle_render::quality::StageQuality;
    match value {
        "low" => StageQuality::Low,
        "medium" => StageQuality::Medium,
        "high" => StageQuality::High,
        "best" => StageQuality::Best,
        "high8x8" => StageQuality::High8x8,
        "high16x16" => StageQuality::High16x16,
        other => {
            tracing::warn!("unknown --quality {other}; using high");
            StageQuality::High
        }
    }
}
