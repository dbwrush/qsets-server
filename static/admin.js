// Page values arrive as data attributes on <body>; the CSP does not allow inline scripts.
window.QSETS_CSRF = document.body.dataset.csrf;

// API helper
async function adminApi(url, options = {}) {
  const headers = Object.assign({}, options.headers || {});
  if (window.QSETS_CSRF) headers["x-csrf-token"] = window.QSETS_CSRF;
  const res = await fetch(url, Object.assign({}, options, { headers }));
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || data.message || "Request failed");
  return data;
}

function escapeHtml(text) {
  const map = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" };
  return String(text).replace(/[&<>"']/g, m => map[m]);
}

// ── Logout ──
document.getElementById("logout-link")?.addEventListener("click", async (e) => {
  e.preventDefault();
  try {
    await adminApi("/api/logout", { method: "POST" });
    window.location.href = "/";
  } catch (err) {
    console.error("Logout failed:", err);
  }
});

// ── Tab switching with auto-load ──
const tabsLoaded = {};
document.querySelectorAll(".admin-tab").forEach(tab => {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".admin-tab").forEach(t => t.classList.remove("active"));
    document.querySelectorAll(".admin-tab-content").forEach(c => c.classList.remove("active"));
    tab.classList.add("active");
    const target = document.getElementById(tab.dataset.tab);
    if (target) target.classList.add("active");

    const tabId = tab.dataset.tab;
    if (tabId === "tab-tiers" && !tabsLoaded.tiers) { tabsLoaded.tiers = true; loadTiers(); }
    if (tabId === "tab-users" && !tabsLoaded.users) { tabsLoaded.users = true; loadUsers(); }
    if (tabId === "tab-audit" && !tabsLoaded.audit) { tabsLoaded.audit = true; loadAudit(); }
  });
});

// ── Tier options (named, loaded from the server) ──
let assignableTiers = [];
let allTiers = [];

function tierOptionsHtml(tiers, selectedId) {
  return tiers
    .map(t => `<option value="${t.id}"${Number(selectedId) === t.id ? " selected" : ""}>${escapeHtml(t.name)}</option>`)
    .join("");
}

function tierNameById(id) {
  const match = (allTiers.length ? allTiers : assignableTiers).find(t => t.id === Number(id));
  return match ? match.name : `Tier ${id}`;
}

async function loadAssignableTiers() {
  const select = document.getElementById("upload-tier");
  try {
    const data = await adminApi("/api/tiers");
    assignableTiers = data.tiers || [];
    if (select) select.innerHTML = tierOptionsHtml(assignableTiers);
  } catch (err) {
    if (select) select.innerHTML = "";
    console.error("Failed to load tiers:", err);
  }
}

async function loadAllTiers() {
  const data = await adminApi("/api/admin/tiers");
  allTiers = data.tiers || [];
  const userTierSelect = document.getElementById("user-tier");
  if (userTierSelect) userTierSelect.innerHTML = tierOptionsHtml(allTiers);
  return allTiers;
}

document.addEventListener("DOMContentLoaded", () => {
  void loadAssignableTiers();
  void loadManagedPools();
});

// ── Pool upload ──
document.getElementById("upload-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(e.target);
  const status = document.getElementById("upload-status");
  try {
    const data = await adminApi("/api/pools", { method: "POST", body: fd });
    status.innerHTML = `<p class="msg-success">${escapeHtml(validationMessage(`Uploaded pool #${data.id}`, data.validation))}</p>`;
    e.target.reset();
    void loadManagedPools();
  } catch (err) {
    status.innerHTML = `<p class="msg-error">${escapeHtml(err.message)}</p>`;
  }
});

function validationMessage(prefix, validation) {
  let msg = `${prefix} — ${validation?.valid_count ?? "?"} valid questions`;
  if (validation?.skipped_count > 0) msg += `, ${validation.skipped_count} rows skipped`;
  return msg;
}

// ── Manage pools (replace / delete) ──
let replaceTarget = null;

async function loadManagedPools() {
  const container = document.getElementById("pools-table");
  if (!container) return;
  container.innerHTML = '<p class="msg-info">Loading…</p>';
  try {
    const data = await adminApi("/api/pools");
    renderPoolsTable((data.pools || []).filter(p => p.can_manage), container);
  } catch (err) {
    container.innerHTML = `<p class="msg-error">${escapeHtml(err.message)}</p>`;
  }
}

function renderPoolsTable(pools, container) {
  if (!pools.length) { container.innerHTML = '<p class="msg-info">No pools you can manage.</p>'; return; }
  let html = `<table class="data-table">
    <thead><tr><th>Name</th><th>Tier</th><th>Last updated</th><th></th></tr></thead><tbody>`;
  pools.forEach(p => {
    html += `<tr data-id="${p.id}" data-name="${escapeHtml(p.name)}">
      <td>${escapeHtml(p.name)}</td>
      <td>${escapeHtml(p.tier_name)}</td>
      <td>${escapeHtml(new Date(p.updated_at).toLocaleString())}</td>
      <td class="admin-actions">
        <button class="btn-icon replace-pool" title="Replace CSV">&#8635;</button>
        <button class="btn-icon danger delete-pool" title="Delete">&#128465;</button>
      </td>
    </tr>`;
  });
  html += "</tbody></table>";
  container.innerHTML = html;

  container.querySelectorAll(".replace-pool").forEach(btn => {
    btn.addEventListener("click", () => {
      const row = btn.closest("tr");
      replaceTarget = { id: row.dataset.id, name: row.dataset.name };
      document.getElementById("replace-file").click();
    });
  });
  container.querySelectorAll(".delete-pool").forEach(btn => {
    btn.addEventListener("click", () => deletePool(btn.closest("tr")));
  });
}

document.getElementById("replace-file")?.addEventListener("change", async (e) => {
  const input = e.target;
  const file = input.files?.[0];
  const target = replaceTarget;
  input.value = "";
  replaceTarget = null;
  if (!file || !target) return;
  if (!confirm(`Replace the questions in "${target.name}" with ${file.name}?`)) return;

  const status = document.getElementById("replace-status");
  const fd = new FormData();
  fd.append("file", file);
  try {
    const data = await adminApi(`/api/pools/${target.id}`, { method: "PUT", body: fd });
    status.innerHTML = `<p class="msg-success">${escapeHtml(validationMessage(`Replaced "${target.name}"`, data.validation))}</p>`;
    void loadManagedPools();
  } catch (err) {
    status.innerHTML = `<p class="msg-error">${escapeHtml(err.message)}</p>`;
  }
});

async function deletePool(row) {
  const name = row.dataset.name;
  if (!confirm(`Delete pool "${name}"? This cannot be undone.`)) return;
  try {
    await adminApi(`/api/pools/${row.dataset.id}`, { method: "DELETE" });
    void loadManagedPools();
  } catch (err) {
    alert(err.message);
  }
}

// ── Tiers ──
async function loadTiers() {
  const container = document.getElementById("tiers-table");
  container.innerHTML = '<p class="msg-info">Loading…</p>';
  try {
    const tiers = await loadAllTiers();
    renderTiersTable(tiers, container);
  } catch (err) {
    container.innerHTML = `<p class="msg-error">${escapeHtml(err.message)}</p>`;
  }
}

function renderTiersTable(tiers, container) {
  if (!tiers.length) { container.innerHTML = '<p class="msg-info">No tiers found.</p>'; return; }
  let html = `<table class="data-table">
    <thead><tr><th>ID</th><th>Name</th><th>Rank</th><th></th></tr></thead><tbody>`;
  tiers.forEach(t => {
    html += `<tr data-id="${t.id}">
      <td>${t.id}</td>
      <td class="cell-name">${escapeHtml(t.name)}</td>
      <td class="cell-rank">${t.rank}</td>
      <td class="admin-actions">
        <button class="btn-icon edit-tier" title="Edit">&#9998;</button>
        <button class="btn-icon danger delete-tier" title="Delete">&#128465;</button>
      </td>
    </tr>`;
  });
  html += "</tbody></table>";
  container.innerHTML = html;

  container.querySelectorAll(".edit-tier").forEach(btn => {
    btn.addEventListener("click", () => startEditTier(btn.closest("tr")));
  });
  container.querySelectorAll(".delete-tier").forEach(btn => {
    btn.addEventListener("click", () => deleteTier(btn.closest("tr")));
  });
}

function startEditTier(row) {
  const id = row.dataset.id;
  const nameCell = row.querySelector(".cell-name");
  const rankCell = row.querySelector(".cell-rank");
  const actionsCell = row.querySelector(".admin-actions");

  const oldName = nameCell.textContent;
  const oldRank = rankCell.textContent;

  nameCell.innerHTML = `<input type="text" class="edit-name" value="${escapeHtml(oldName)}" />`;
  rankCell.innerHTML = `<input type="number" class="edit-rank" value="${oldRank}" min="0" />`;
  actionsCell.innerHTML = `
    <button class="btn-icon save-tier" title="Save">&#10003;</button>
    <button class="btn-icon danger cancel-tier" title="Cancel">&#10005;</button>
  `;

  actionsCell.querySelector(".save-tier").addEventListener("click", async () => {
    const name = row.querySelector(".edit-name").value.trim();
    const rank = Number(row.querySelector(".edit-rank").value);
    try {
      await adminApi(`/api/admin/tiers/${id}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, rank }),
      });
      tabsLoaded.tiers = false;
      loadTiers();
    } catch (err) {
      alert(err.message);
    }
  });

  actionsCell.querySelector(".cancel-tier").addEventListener("click", () => {
    nameCell.textContent = oldName;
    rankCell.textContent = oldRank;
    actionsCell.innerHTML = `
      <button class="btn-icon edit-tier" title="Edit">&#9998;</button>
      <button class="btn-icon danger delete-tier" title="Delete">&#128465;</button>
    `;
    actionsCell.querySelector(".edit-tier").addEventListener("click", () => startEditTier(row));
    actionsCell.querySelector(".delete-tier").addEventListener("click", () => deleteTier(row));
  });
}

async function deleteTier(row) {
  const id = row.dataset.id;
  const name = row.querySelector(".cell-name")?.textContent || id;
  if (!confirm(`Delete tier "${name}"?`)) return;
  try {
    await adminApi(`/api/admin/tiers/${id}`, { method: "DELETE" });
    tabsLoaded.tiers = false;
    loadTiers();
  } catch (err) {
    alert(err.message);
  }
}

// Create tier
document.getElementById("tier-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(e.target);
  try {
    await adminApi("/api/admin/tiers", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: fd.get("name").trim(), rank: Number(fd.get("rank")) }),
    });
    e.target.reset();
    tabsLoaded.tiers = false;
    loadTiers();
    void loadAssignableTiers();
  } catch (err) {
    alert(err.message);
  }
});

// ── Users ──
async function loadUsers() {
  const container = document.getElementById("users-table");
  container.innerHTML = '<p class="msg-info">Loading…</p>';
  try {
    if (!allTiers.length) await loadAllTiers();
    const data = await adminApi("/api/admin/users");
    renderUsersTable(data.users || [], container);
  } catch (err) {
    container.innerHTML = `<p class="msg-error">${escapeHtml(err.message)}</p>`;
  }
}

function renderUsersTable(users, container) {
  if (!users.length) { container.innerHTML = '<p class="msg-info">No users found.</p>'; return; }
  let html = `<table class="data-table">
    <thead><tr><th>ID</th><th>Username</th><th>Tier</th><th>Admin</th><th>Can Upload</th><th></th></tr></thead><tbody>`;
  users.forEach(u => {
    html += `<tr data-id="${u.id}" data-tier="${u.tier_id}" data-admin="${u.is_admin}" data-upload="${u.can_upload_pools}">
      <td>${u.id}</td>
      <td class="cell-username">${escapeHtml(u.username)}</td>
      <td class="cell-tier">${escapeHtml(tierNameById(u.tier_id))}</td>
      <td class="cell-admin">${u.is_admin ? "Yes" : "No"}</td>
      <td class="cell-upload">${u.can_upload_pools ? "Yes" : "No"}</td>
      <td class="admin-actions">
        <button class="btn-icon edit-user" title="Edit">&#9998;</button>
        <button class="btn-icon danger delete-user" title="Delete">&#128465;</button>
      </td>
    </tr>`;
  });
  html += "</tbody></table>";
  container.innerHTML = html;

  container.querySelectorAll(".edit-user").forEach(btn => {
    btn.addEventListener("click", () => startEditUser(btn.closest("tr")));
  });
  container.querySelectorAll(".delete-user").forEach(btn => {
    btn.addEventListener("click", () => deleteUser(btn.closest("tr")));
  });
}

function startEditUser(row) {
  const id = row.dataset.id;
  const usernameCell = row.querySelector(".cell-username");
  const tierCell = row.querySelector(".cell-tier");
  const adminCell = row.querySelector(".cell-admin");
  const uploadCell = row.querySelector(".cell-upload");
  const actionsCell = row.querySelector(".admin-actions");

  const oldUsername = usernameCell.textContent;
  const oldTierId = row.dataset.tier;
  const oldTierLabel = tierCell.textContent;
  const oldAdmin = row.dataset.admin === "true";
  const oldUpload = row.dataset.upload === "true";

  usernameCell.innerHTML = `<input type="text" class="edit-username" value="${escapeHtml(oldUsername)}" />`;
  tierCell.innerHTML = `<select class="edit-tier">${tierOptionsHtml(allTiers, oldTierId)}</select>`;
  adminCell.innerHTML = `<input type="checkbox" class="edit-admin" ${oldAdmin ? "checked" : ""} />`;
  uploadCell.innerHTML = `<input type="checkbox" class="edit-upload" ${oldUpload ? "checked" : ""} />`;
  actionsCell.innerHTML = `
    <button class="btn-icon save-user" title="Save">&#10003;</button>
    <button class="btn-icon danger cancel-user" title="Cancel">&#10005;</button>
  `;

  actionsCell.querySelector(".save-user").addEventListener("click", async () => {
    const username = row.querySelector(".edit-username").value.trim();
    const tier_id = Number(row.querySelector(".edit-tier").value);
    const is_admin = row.querySelector(".edit-admin").checked;
    const can_upload_pools = row.querySelector(".edit-upload").checked;
    try {
      await adminApi(`/api/admin/users/${id}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ username, password: null, tier_id, is_admin, can_upload_pools }),
      });
      tabsLoaded.users = false;
      loadUsers();
    } catch (err) {
      alert(err.message);
    }
  });

  actionsCell.querySelector(".cancel-user").addEventListener("click", () => {
    usernameCell.textContent = oldUsername;
    tierCell.textContent = oldTierLabel;
    adminCell.textContent = oldAdmin ? "Yes" : "No";
    uploadCell.textContent = oldUpload ? "Yes" : "No";
    actionsCell.innerHTML = `
      <button class="btn-icon edit-user" title="Edit">&#9998;</button>
      <button class="btn-icon danger delete-user" title="Delete">&#128465;</button>
    `;
    actionsCell.querySelector(".edit-user").addEventListener("click", () => startEditUser(row));
    actionsCell.querySelector(".delete-user").addEventListener("click", () => deleteUser(row));
  });
}

async function deleteUser(row) {
  const id = row.dataset.id;
  const name = row.querySelector(".cell-username")?.textContent || id;
  if (!confirm(`Delete user "${name}"?`)) return;
  try {
    await adminApi(`/api/admin/users/${id}`, { method: "DELETE" });
    tabsLoaded.users = false;
    loadUsers();
  } catch (err) {
    alert(err.message);
  }
}

// Create user
document.getElementById("user-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(e.target);
  try {
    await adminApi("/api/admin/users", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        username: fd.get("username").trim(),
        password: fd.get("password"),
        tier_id: Number(fd.get("tier_id")),
        is_admin: fd.get("is_admin") === "on",
        can_upload_pools: fd.get("can_upload_pools") === "on",
      }),
    });
    e.target.reset();
    tabsLoaded.users = false;
    loadUsers();
  } catch (err) {
    alert(err.message);
  }
});

// ── Audit log ──
let auditNextBefore = null;

function auditUrl(before) {
  const params = new URLSearchParams();
  const action = document.getElementById("audit-action")?.value;
  if (action) params.set("action", action);
  if (before != null) params.set("before", before);
  const query = params.toString();
  return query ? `/api/admin/audit?${query}` : "/api/admin/audit";
}

async function loadAudit(append = false) {
  const container = document.getElementById("audit-table");
  const more = document.getElementById("audit-more");
  if (!append) container.innerHTML = '<p class="msg-info">Loading…</p>';
  try {
    const data = await adminApi(auditUrl(append ? auditNextBefore : null));
    auditNextBefore = data.next_before;
    renderAudit(data.entries || [], container, append);
    more.hidden = auditNextBefore == null;
  } catch (err) {
    container.innerHTML = `<p class="msg-error">${escapeHtml(err.message)}</p>`;
    more.hidden = true;
  }
}

function auditDetails(metadata) {
  if (!metadata || typeof metadata !== "object" || !Object.keys(metadata).length) return "";
  return Object.entries(metadata).map(([k, v]) => `${k}: ${typeof v === "object" ? JSON.stringify(v) : v}`).join(", ");
}

function renderAudit(entries, container, append) {
  const rows = entries.map(e => `<tr>
      <td>${escapeHtml(new Date(e.created_at).toLocaleString())}</td>
      <td>${escapeHtml(e.actor_username ?? (e.actor_user_id != null ? `#${e.actor_user_id}` : "—"))}</td>
      <td>${escapeHtml(e.action)}</td>
      <td>${escapeHtml(`${e.target_type}${e.target_id ? ` ${e.target_id}` : ""}`)}</td>
      <td title="${escapeHtml(e.user_agent || "")}">${escapeHtml(e.ip_address || "")}</td>
      <td>${escapeHtml(auditDetails(e.metadata))}</td>
    </tr>`).join("");

  const tbody = container.querySelector("tbody");
  if (append && tbody) { tbody.insertAdjacentHTML("beforeend", rows); return; }
  if (!entries.length) { container.innerHTML = '<p class="msg-info">No audit entries.</p>'; return; }
  container.innerHTML = `<table class="data-table">
    <thead><tr><th>Time</th><th>Actor</th><th>Action</th><th>Target</th><th>IP</th><th>Details</th></tr></thead>
    <tbody>${rows}</tbody></table>`;
}

document.getElementById("audit-action")?.addEventListener("change", () => loadAudit());
document.getElementById("audit-more")?.addEventListener("click", () => loadAudit(true));
