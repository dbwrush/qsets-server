// Registers the service worker that makes the site installable and gives it an offline page.
if ("serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch(() => {
      /* the site works the same without it */
    });
  });
}
