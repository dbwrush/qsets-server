// Served at /sw.js (see src/lib.rs) so its scope covers the whole site.
//
// Pages are never cached: each one embeds a per-session CSRF token and the signed-in user's
// header, so a stored copy would be stale or belong to someone else. Navigations go to the
// network and fall back to a static offline page. /api/ is never touched. Only /static/ files,
// which hold no per-user data, are cached.
//
// The one page kept is /study/offline, an anonymous copy of the study page (no session, no
// CSRF token). It is what /study shows without a connection; the page itself then reads the
// encrypted pools saved on the device. See static/vault.mjs.
const CACHE = "qsets-static-v2";
const OFFLINE_URL = "/static/offline.html";
const STUDY_URL = "/study";
const STUDY_SHELL_URL = "/study/offline";
const PRECACHE = [
  OFFLINE_URL,
  STUDY_SHELL_URL,
  "/static/style.css",
  "/static/theme.js",
  "/static/header.js",
  "/static/study.js",
  "/static/study-core.mjs",
  "/static/study-session.mjs",
  "/static/vault.mjs",
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

async function refreshStudyShell() {
  try {
    const response = await fetch(STUDY_SHELL_URL, { cache: "no-cache" });
    if (response.ok) await (await caches.open(CACHE)).put(STUDY_SHELL_URL, response);
  } catch {
    /* keep the copy already stored */
  }
}

self.addEventListener("fetch", event => {
  const { request } = event;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  if (request.mode === "navigate") {
    const study = url.pathname === STUDY_URL;
    event.respondWith(
      fetch(request).then(response => {
        // Online: keep the offline study page as current as the deployed one.
        if (study) event.waitUntil(refreshStudyShell());
        return response;
      }).catch(async () => {
        const page = await caches.match(study ? STUDY_SHELL_URL : OFFLINE_URL)
          || await caches.match(OFFLINE_URL);
        return page || Response.error();
      }),
    );
  } else if (url.pathname.startsWith("/static/")) {
    event.respondWith(staticFile(request));
  }
});
