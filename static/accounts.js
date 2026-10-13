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

function postJson(url, body) {
  return api(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

function setStatus(el, text, kind = "") {
  el.textContent = text;
  el.className = kind ? `login-status ${kind}` : "login-status";
}

// Wires a form to an API call: disables the button while it runs and reports the outcome.
function handleForm(formId, statusId, { validate, request, done }) {
  const form = document.getElementById(formId);
  if (!form) return;
  const status = document.getElementById(statusId);
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const problem = validate?.(form);
    if (problem) return setStatus(status, problem, "error");

    const button = form.querySelector('button[type="submit"]');
    button.disabled = true;
    setStatus(status, "Working…");
    try {
      const data = await request(form);
      setStatus(status, data.message, "success");
      done?.(form);
    } catch (err) {
      setStatus(status, `Error: ${err.message}`, "error");
      button.disabled = false;
    }
  });
}

const passwordsMatch = (form) =>
  form.new_password.value === form.confirm_password.value ? null : "The passwords do not match.";

handleForm("register-form", "register-status", {
  validate: passwordsMatch,
  request: (form) =>
    postJson("/api/register", {
      username: form.username.value,
      email: form.email.value,
      password: form.new_password.value,
    }),
  done: (form) => form.querySelectorAll("input").forEach((input) => (input.disabled = true)),
});

handleForm("forgot-form", "forgot-status", {
  request: (form) => postJson("/api/password-reset/request", { email: form.email.value }),
});

// The link carries the token in the query string; move it out of the address bar and history.
const linkToken = new URLSearchParams(window.location.search).get("token");
if (linkToken) history.replaceState(null, "", window.location.pathname);

handleForm("reset-form", "reset-status", {
  validate: (form) =>
    !linkToken
      ? "This reset link is incomplete. Request a new one."
      : passwordsMatch(form),
  request: (form) =>
    postJson("/api/password-reset/confirm", {
      token: linkToken,
      new_password: form.new_password.value,
    }),
  done: () => setTimeout(() => (window.location.href = "/login"), 1500),
});

handleForm("resend-form", "resend-status", {
  request: (form) => postJson("/api/email/resend", { email: form.email.value }),
});

// Opening the verification link confirms the address. It is done by script rather than on page
// load of a GET so mail scanners that merely fetch links cannot spend the one-time token.
const verifyStatus = document.getElementById("verify-status");
if (verifyStatus) {
  const resend = document.getElementById("resend-form");
  if (!linkToken) {
    setStatus(verifyStatus, "This link is incomplete.", "error");
    resend.hidden = false;
  } else {
    postJson("/api/email/verify", { token: linkToken })
      .then((data) => {
        setStatus(verifyStatus, data.message, "success");
      })
      .catch((err) => {
        setStatus(verifyStatus, `Error: ${err.message}`, "error");
        resend.hidden = false;
      });
  }
}
