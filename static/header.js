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

  // ── Logout ──
  document.getElementById("logout-link")?.addEventListener("click", async (e) => {
    e.preventDefault();
    try {
      const res = await fetch("/api/logout", {
        method: "POST",
        headers: { "x-csrf-token": document.body.dataset.csrf || "" },
      });
      if (!res.ok) throw new Error(`Logout failed (${res.status})`);
      window.location.href = "/";
    } catch (err) {
      console.error("Logout failed:", err);
    }
  });
})();
