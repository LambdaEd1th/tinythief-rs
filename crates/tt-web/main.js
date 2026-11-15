// Trunk injects the generated wasm-bindgen module and exposes it as
// `window.wasmBindings` once the module has been initialised.
function waitForBindings() {
  if (window.wasmBindings) {
    return Promise.resolve(window.wasmBindings);
  }
  return new Promise((resolve) => {
    window.addEventListener(
      "TrunkApplicationStarted",
      () => resolve(window.wasmBindings),
      { once: true },
    );
  });
}

const canvas = document.getElementById("tt");
const loading = document.getElementById("loading");

/** Map a browser event to canvas (stage) coordinates. */
function stageCoords(event) {
  const rect = canvas.getBoundingClientRect();
  return {
    x: ((event.clientX - rect.left) / rect.width) * canvas.width,
    y: ((event.clientY - rect.top) / rect.height) * canvas.height,
  };
}

async function main() {
  const bindings = await waitForBindings();
  // `?assets=<base-url>` lets the bundle live somewhere else (must allow CORS).
  const params = new URLSearchParams(location.search);
  const assets = params.get("assets") || "assets/";
  const quality = params.get("quality") || null;
  const alwaysRender = params.get("alwaysRender") === "1";
  const frameRate = params.get("frameRate") ? Number(params.get("frameRate")) : null;
  const mute = params.get("mute") === "1";
  const host = await bindings.TinyThiefHost.create(canvas, assets, quality, alwaysRender, frameRate, mute);
  loading.remove();

  // `?warmup=N` advances N frames before the first paint (used for screenshots
  // and smoke tests, mirrors the native host's --frames).
  const warmup = Number(new URLSearchParams(location.search).get("warmup") || 0);
  for (let i = 0; i < warmup; i += 1) {
    host.tick(1000 / 30);
    // Yield so Ruffle's asset-loading futures can run between frames.
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  // Browsers keep the AudioContext suspended until a user gesture.
  const unlockAudio = () => host.resume_audio();
  canvas.addEventListener("pointerdown", unlockAudio, { once: true });
  window.addEventListener("keydown", unlockAudio, { once: true });

  // Input: pointer events cover mouse + touch.
  canvas.addEventListener("pointermove", (e) => {
    const { x, y } = stageCoords(e);
    host.mouse_move(x, y);
  });
  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    const { x, y } = stageCoords(e);
    host.mouse_down(x, y);
  });
  canvas.addEventListener("pointerup", (e) => {
    const { x, y } = stageCoords(e);
    host.mouse_up(x, y);
  });
  canvas.addEventListener("pointercancel", (e) => {
    const { x, y } = stageCoords(e);
    host.mouse_up(x, y);
  });
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());

  // Lifecycle: pause the game (and AS3) when the tab is hidden.
  document.addEventListener("visibilitychange", () => {
    const hidden = document.visibilityState === "hidden";
    host.set_paused(hidden);
    if (!hidden) {
      last = performance.now();
      host.resume_audio();
    }
  });

  // Frame-time statistics (p50/p95 + effective fps), readable from the console
  // or via CDP: `__ttStats()`.
  const frameTimes = [];
  window.__ttStats = () => {
    const sorted = [...frameTimes].sort((a, b) => a - b);
    const pick = (q) => (sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * q))] : 0);
    const avg = sorted.reduce((a, b) => a + b, 0) / (sorted.length || 1);
    return {
      frames: sorted.length,
      p50: +pick(0.5).toFixed(2),
      p95: +pick(0.95).toFixed(2),
      avg: +avg.toFixed(2),
      fps: +(1000 / (avg || 1)).toFixed(1),
    };
  };

  window.__ttCanvas = () => canvas.toDataURL("image/png");
  window.__ttAudio = () => ({ state: host.audio_state(), time: host.audio_time(), peak: host.audio_peak() });

  window.__ttCounts = () => ({
    ticks: host.tick_count(),
    renders: host.render_count(),
  });

  window.__ttReset = () => {
    frameTimes.length = 0;
  };

  let last = performance.now();
  function frame(now) {
    const dt = now - last;
    last = now;
    if (frameTimes.length > 600) frameTimes.shift();
    frameTimes.push(dt);
    host.tick(dt);
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

main().catch((err) => {
  console.error(err);
  if (loading) loading.textContent = "Failed to start: " + err;
});
