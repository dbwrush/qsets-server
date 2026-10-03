// Loaded from <head> on every page, before first paint, so a saved choice never flashes the
// other theme. With no saved choice the page follows the device setting (see style.css).
// The CSP does not allow inline scripts, which is why this is a separate file.
(function () {
  var KEY = "qsets-theme";
  var root = document.documentElement;
  var query = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;

  function stored() {
    try {
      var value = localStorage.getItem(KEY);
      return value === "light" || value === "dark" ? value : null;
    } catch (e) {
      return null; // storage can be blocked; the page then just follows the device
    }
  }

  function effective() {
    return stored() || root.dataset.theme || (query && query.matches ? "dark" : "light");
  }

  function set(theme) {
    root.dataset.theme = theme;
    try { localStorage.setItem(KEY, theme); } catch (e) { /* choice lasts for this page only */ }
    document.dispatchEvent(new CustomEvent("qsets-theme", { detail: theme }));
  }

  var saved = stored();
  if (saved) root.dataset.theme = saved;

  // Without a saved choice, keep controls in step when the device switches themes.
  if (query && query.addEventListener) {
    query.addEventListener("change", function () {
      if (!stored()) document.dispatchEvent(new CustomEvent("qsets-theme", { detail: effective() }));
    });
  }

  window.qsetsTheme = { effective: effective, set: set };
})();
