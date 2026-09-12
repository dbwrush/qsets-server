// API helper with CSRF support
async function api(url, options = {}) {
  const headers = Object.assign({}, options.headers || {});
  if (window.QSETS_CSRF) {
    headers["x-csrf-token"] = window.QSETS_CSRF;
  }
  const res = await fetch(url, Object.assign({}, options, { headers }));
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || data.message || "Request failed");
  return data;
}

// Handle login form submission
document.getElementById("login-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  
  const username = document.getElementById("username").value;
  const password = document.getElementById("password").value;
  const statusDiv = document.getElementById("login-status");
  const button = e.target.querySelector('button[type="submit"]');
  
  button.disabled = true;
  statusDiv.textContent = "Signing in...";
  statusDiv.className = "login-status";
  
  try {
    const body = { username, password };
    await api("/api/login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    
    statusDiv.textContent = "Success! Redirecting...";
    statusDiv.className = "login-status success";
    
    // Redirect to main page after brief delay
    setTimeout(() => {
      window.location.href = "/";
    }, 500);
    
  } catch (err) {
    statusDiv.textContent = `Error: ${err.message}`;
    statusDiv.className = "login-status error";
    button.disabled = false;
  }
});
