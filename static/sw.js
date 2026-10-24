// Served at /sw.js (see src/lib.rs) so its scope covers the whole site.
//
// Pages are never cached: each one embeds a per-session CSRF token and the signed-in user's
// header, so a stored copy would be stale or belong to someone else. Navigations go to the
// network and fall back to a static offline page. /api/ is never touched. Only /static/ files,
// which hold no per-user data, are cached.
const CACHE = "qsets-static-v1";
const OFFLINE_URL = "/static/offline.html";
const PRECACHE = [
  OFFLINE_URL,
  "/static/style.css",
  "/static/theme.js",
  "/static/favicon.svg",
  "/static/icons/icon-192.png",
];

self.addEventListener("install", event => {
  event.waitUntil(
    caches.open(CACHE).then(cache => cache.addAll(PRECACHE)).then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", event => {
  event.waitUntil(
    caches.keys()
      .then(keys => Promise.all(keys.filter(key => key !== CACHE).map(key => caches.delete(key))))
      .then(() => self.clients.claim()),
  );
});

// Stale-while-revalidate. Files are versioned with ?v=, so a new version is a different URL;
// when one is stored, older copies of the same path are dropped.
async function staticFile(request) {
  const cache = await caches.open(CACHE);
  const cached = await cache.match(request);
  const refresh = fetch(request).then(async response => {
    if (response.ok) {
      const path = new URL(request.url).pathname;
      for (const old of await cache.keys()) {
        if (new URL(old.url).pathname === path && old.url !== request.url) await cache.delete(old);
      }
      await cache.put(request, response.clone());
    }
    return response;
  });
  if (cached) {
    refresh.catch(() => {});
    return cached;
  }
  try {
    return await refresh;
  } catch (error) {
    // Offline with no copy of this exact version: any version of the file is better than none.
    const other = await cache.match(request, { ignoreSearch: true });
    if (other) return other;
    throw error;
  }
}

self.addEventListener("fetch", event => {
  const { request } = event;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request).catch(() => caches.match(OFFLINE_URL).then(page => page || Response.error())),
    );
  } else if (url.pathname.startsWith("/static/")) {
    event.respondWith(staticFile(request));
  }
});
