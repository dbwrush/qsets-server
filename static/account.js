// Page values arrive as data attributes on <body>; the CSP does not allow inline scripts.
window.QSETS_CSRF = document.body.dataset.csrf;

async function api(url, options = {}) {
  const headers = Object.assign({}, options.headers || {});
  if (window.QSETS_CSRF) headers["x-csrf-token"] = window.QSETS_CSRF;
  const res = await fetch(url, Object.assign({}, options, { headers }));
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || data.message || "Request failed");
  return data;
}

document.getElementById("password-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = e.target;
  const status = document.getElementById("password-status");
  const button = form.querySelector('button[type="submit"]');
  const current = form.current_password.value;
  const next = form.new_password.value;

  if (next !== form.confirm_password.value) {
    status.textContent = "The new passwords do not match.";
    status.className = "login-status error";
    return;
  }

  button.disabled = true;
  status.textContent = "Saving…";
  status.className = "login-status";
  try {
    await api("/api/me/password", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ current_password: current, new_password: next }),
    });
    form.reset();
    status.textContent = "Password changed. Other sessions have been signed out.";
    status.className = "login-status success";
  } catch (err) {
    status.textContent = `Error: ${err.message}`;
    status.className = "login-status error";
  } finally {
    button.disabled = false;
  }
});
