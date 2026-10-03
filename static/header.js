// Logout lives in the header on every page, so it is wired up once here.
// Page values arrive as data attributes on <body>; the CSP does not allow inline scripts.
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
