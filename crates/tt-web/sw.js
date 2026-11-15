// Service worker: cache the app shell and game assets.
//
// Cache names are derived from the content hash of the wasm bundle in
// index.html, so a new deployment automatically installs a new cache and the
// previous one is dropped on activate.

const CACHE_PREFIX = "tinythief-";
let cacheName = `${CACHE_PREFIX}dev`;

const WASM_RE = /tt-web-[a-z0-9]+_bg\.wasm/g;
const JS_RE = /tt-web-[a-z0-9]+\.js/g;

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      self.skipWaiting();
      const index = await fetch("index.html", { cache: "reload" });
      const html = await index.clone().text();

      const wasm = html.match(WASM_RE);
      if (wasm) cacheName = CACHE_PREFIX + wasm[0].match(/tt-web-([a-z0-9]+)_bg/)[1];

      const cache = await caches.open(cacheName);
      await cache.put("index.html", index);

      const scope = self.registration.scope;
      const files = new Set(["main.js"]);
      for (const match of html.matchAll(JS_RE)) files.add(match[0]);
      for (const match of html.matchAll(WASM_RE)) files.add(match[0]);

      await Promise.all(
        [...files].map(async (file) => {
          try {
            const url = new URL(file, scope);
            const response = await fetch(url, { cache: "reload" });
            if (response.ok) await cache.put(url, response);
          } catch (_) {
            // Offline during install: keep whatever HTTP cache provides later.
          }
        }),
      );
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const keys = await caches.keys();
      await Promise.all(
        keys
          .filter((key) => key.startsWith(CACHE_PREFIX) && key !== cacheName)
          .map((key) => caches.delete(key)),
      );
      await self.clients.claim();
    })(),
  );
});

async function cacheFirst(request) {
  const cache = await caches.open(cacheName);
  const hit = await cache.match(request);
  if (hit) return hit;

  const response = await fetch(request);
  if (response.ok) await cache.put(request, response.clone());
  return response;
}

async function staleWhileRevalidate(request) {
  const cache = await caches.open(cacheName);
  const cached = await cache.match(request);
  const network = fetch(request)
    .then(async (response) => {
      if (response.ok) await cache.put(request, response.clone());
      return response;
    })
    .catch(() => null);
  return cached || network || Response.error();
}

async function networkFirst(request) {
  const cache = await caches.open(cacheName);
  try {
    const response = await fetch(request);
    if (response.ok) await cache.put("index.html", response.clone());
    return response;
  } catch (_) {
    return (
      (await cache.match(request, { ignoreSearch: true })) ||
      (await cache.match("index.html")) ||
      Response.error()
    );
  }
}

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  if (request.mode === "navigate") {
    event.respondWith(networkFirst(request));
    return;
  }

  if (url.pathname.endsWith("/main.js") || url.pathname.endsWith("/sw.js")) {
    event.respondWith(staleWhileRevalidate(request));
    return;
  }

  if (url.pathname.includes("/assets/") || /tt-web-[a-z0-9]+(\.js|_bg\.wasm)$/.test(url.pathname)) {
    event.respondWith(cacheFirst(request));
  }
});
