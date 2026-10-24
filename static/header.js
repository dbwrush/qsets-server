// Header behavior shared by every page: the user menu, the light/dark switch, and logout.
// Page values arrive as data attributes on <body>; the CSP does not allow inline scripts.
(function () {
  const theme = window.qsetsTheme;

  // ── Light/dark controls (menu switch for signed-in users, icon button otherwise) ──
  const themeSwitch = document.getElementById("theme-toggle");
  const themeButton = document.getElementById("theme-button");

  function syncThemeControls() {
    const dark = theme && theme.effective() === "dark";
    if (themeSwitch) themeSwitch.checked = dark;
    if (themeButton) themeButton.setAttribute("aria-pressed", String(dark));
  }

  themeSwitch?.addEventListener("change", () => theme?.set(themeSwitch.checked ? "dark" : "light"));
  themeButton?.addEventListener("click", () => theme?.set(theme.effective() === "dark" ? "light" : "dark"));
  document.addEventListener("qsets-theme", syncThemeControls);
  syncThemeControls();

  // ── User menu: opens on click, closes on outside click, Escape, or choosing an item ──
  const menuButton = document.getElementById("user-menu-button");
  const menu = document.getElementById("user-menu");

  function setMenu(open, { restoreFocus = false } = {}) {
    if (!menuButton || !menu) return;
    menu.hidden = !open;
    menuButton.setAttribute("aria-expanded", String(open));
    if (!open && restoreFocus) menuButton.focus();
  }

  menuButton?.addEventListener("click", () => setMenu(menu.hidden));

  document.addEventListener("click", (e) => {
    if (menu && !menu.hidden && !menu.contains(e.target) && !menuButton.contains(e.target)) setMenu(false);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && menu && !menu.hidden) setMenu(false, { restoreFocus: true });
  });

  // Tabbing out of the menu closes it rather than leaving it open behind the page.
  menu?.addEventListener("focusout", (e) => {
    if (e.relatedTarget && !menu.contains(e.relatedTarget) && e.relatedTarget !== menuButton) setMenu(false);
  });

  // ── Offline copy of question pools (installed app only) ──
  // The copy belongs to the signed-in user: it is wiped on logout and whenever a page shows
  // nobody signed in, and its lease is renewed from /api/me. The study page does this itself.
  const PENDING_LOGOUT = "qsets-pending-logout";
  const installed = window.matchMedia?.("(display-mode: standalone)").matches || navigator.standalone === true;

  function pendingLogout() {
    try { return localStorage.getItem(PENDING_LOGOUT) === "1"; } catch (e) { return false; }
  }
  function setPendingLogout(value) {
    try { value ? localStorage.setItem(PENDING_LOGOUT, "1") : localStorage.removeItem(PENDING_LOGOUT); } catch (e) { /* nothing to do */ }
  }
  const logoutRequest = () => fetch("/api/logout", {
    method: "POST",
    headers: { "x-csrf-token": document.body.dataset.csrf || "" },
  });

  if (pendingLogout()) {
    // A logout that could not reach the server earlier: finish it as soon as there is a connection.
    logoutRequest().then((res) => {
      if (res.ok) { setPendingLogout(false); window.location.reload(); }
    }).catch(() => {});
  } else if (installed && !document.body.dataset.offlineShell && window.location.pathname !== "/study") {
    import("/static/vault.mjs?v=offline-1").then(({ syncDeviceVault, wipeDeviceVault }) => {
      if (!document.getElementById("user-menu-button")) return wipeDeviceVault();
      return syncDeviceVault(async () => {
        const res = await fetch("/api/me");
        if (res.status === 401) return null;
        if (!res.ok) throw new Error(`Session check failed (${res.status})`);
        return res.json();
      });
    }).catch((err) => console.error("Offline data sync failed:", err));
  }

  // ── Logout ──
  document.getElementById("logout-link")?.addEventListener("click", async (e) => {
    e.preventDefault();
    // Offline copies go first, so they are gone even if the server cannot be reached.
    try {
      const { wipeDeviceVault } = await import("/static/vault.mjs?v=offline-1");
      await wipeDeviceVault();
    } catch (err) {
      console.error("Could not clear offline data:", err);
    }
    try {
      const res = await logoutRequest();
      if (!res.ok) throw new Error(`Logout failed (${res.status})`);
      window.location.href = "/";
    } catch (err) {
      console.error("Logout failed:", err);
      if (err instanceof TypeError) {
        // Offline: sign out on the server as soon as the app is online again.
        setPendingLogout(true);
        window.location.href = "/";
      }
    }
  });
})();
