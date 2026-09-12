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

// State
let selectedPoolId = null;
let availablePools = [];
let availableBooks = [];
let generatedSets = [];
let generatedRoundNames = [];
let lastSelectedBookFilters = [];

// Wizard navigation
function showStep(id) {
  document.querySelectorAll(".wizard-step").forEach(el => el.classList.remove("active"));
  const target = document.getElementById(id);
  if (target) target.classList.add("active");
}

// Init
document.addEventListener("DOMContentLoaded", async () => {
  initializeHeader();
  await loadPools();

  document.getElementById("downloadRTF")?.addEventListener("click", downloadRTF);
  document.getElementById("downloadJSON")?.addEventListener("click", downloadJSON);
  document.getElementById("showAnswers")?.addEventListener("change", toggleAnswers);

  document.getElementById("backToStep1")?.addEventListener("click", () => showStep("step-1"));
  document.getElementById("nextToStep3")?.addEventListener("click", () => showStep("step-3"));
  document.getElementById("backToStep2")?.addEventListener("click", () => showStep("step-2"));
  document.getElementById("backToStep3")?.addEventListener("click", () => showStep("step-3"));
});

function initializeHeader() {
  const el = document.getElementById("user-status");
  if (window.QSETS_IS_AUTHENTICATED) {
    el.innerHTML = `
      <span class="user-info">${escapeHtml(window.QSETS_USERNAME)}</span>
      <a href="#" id="logout-link">Logout</a>
      <a href="/admin">Admin</a>
    `;
    document.getElementById("logout-link")?.addEventListener("click", handleLogout);
  } else {
    el.innerHTML = '<a href="/login">Login</a>';
  }
}

async function handleLogout(e) {
  e.preventDefault();
  try {
    await api("/api/logout", { method: "POST" });
    window.location.href = "/";
  } catch (err) {
    console.error("Logout failed:", err);
  }
}

// ── Pool Selection (table) ──
async function loadPools() {
  const section = document.getElementById("pool-section");
  try {
    const data = await api("/api/pools");
    availablePools = data.pools || [];

    if (availablePools.length === 0) {
      section.innerHTML = '<p class="msg-info">No pools available.</p>';
      return;
    }

    let html = `<table class="data-table">
      <thead><tr><th>Pool Name</th><th>Tier</th></tr></thead><tbody>`;
    availablePools.forEach(pool => {
      html += `<tr class="clickable" data-pool-id="${pool.id}">
        <td>${escapeHtml(pool.name)}</td>
        <td>${escapeHtml(pool.tier_name)}</td>
      </tr>`;
    });
    html += "</tbody></table>";
    section.innerHTML = html;

    section.querySelectorAll("tr.clickable").forEach(row => {
      row.addEventListener("click", () => selectPool(row));
    });
  } catch (err) {
    section.innerHTML = `<p class="msg-error">Failed to load pools: ${escapeHtml(err.message)}</p>`;
  }
}

function selectPool(row) {
  document.querySelectorAll("#pool-section tr").forEach(r => r.classList.remove("row-selected"));
  row.classList.add("row-selected");
  selectedPoolId = parseInt(row.dataset.poolId);
  void loadPoolBooks(selectedPoolId);
}

// ── Book Selection (table) ──
async function loadPoolBooks(poolId) {
  const container = document.getElementById("bookSelection");
  container.innerHTML = '<p class="msg-info">Loading books…</p>';
  try {
    const data = await api(`/api/pools/${poolId}/books`);
    availableBooks = data.books || [];

    if (availableBooks.length === 0) {
      container.innerHTML = '<p class="msg-info">No references found in this pool.</p>';
      return;
    }

    renderBookSelection(availableBooks);
    showStep("step-2");
  } catch (err) {
    container.innerHTML = `<p class="msg-error">Failed to load books: ${escapeHtml(err.message)}</p>`;
  }
}

function renderBookSelection(books) {
  const container = document.getElementById("bookSelection");
  let html = `<table class="data-table">
    <thead><tr><th></th><th>Book</th><th>Available</th><th>From</th><th>To</th></tr></thead><tbody>`;

  books.forEach((book, idx) => {
    html += `<tr data-book-index="${idx}">
      <td><input type="checkbox" class="book-enabled" checked /></td>
      <td>${escapeHtml(book.name)}</td>
      <td>${book.min_chapter}–${book.max_chapter}</td>
      <td><input type="number" class="chapter-start" min="${book.min_chapter}" max="${book.max_chapter}" value="${book.min_chapter}" /></td>
      <td><input type="number" class="chapter-end" min="${book.min_chapter}" max="${book.max_chapter}" value="${book.max_chapter}" /></td>
    </tr>`;
  });

  html += "</tbody></table>";
  container.innerHTML = html;

  container.querySelectorAll("tbody tr").forEach(row => {
    const enabled = row.querySelector(".book-enabled");
    const start = row.querySelector(".chapter-start");
    const end = row.querySelector(".chapter-end");
    if (!enabled || !start || !end) return;

    const syncDisabled = () => { start.disabled = end.disabled = !enabled.checked; };
    const syncBounds = () => { if (Number(start.value) > Number(end.value)) end.value = start.value; };

    enabled.addEventListener("change", syncDisabled);
    start.addEventListener("change", syncBounds);
    end.addEventListener("change", syncBounds);
    syncDisabled();
  });
}

function selectedBookFilters() {
  const rows = document.querySelectorAll("#bookSelection tbody tr");
  const filters = [];
  rows.forEach(row => {
    const enabled = row.querySelector(".book-enabled");
    if (!enabled?.checked) return;
    const idx = Number(row.dataset.bookIndex);
    const book = availableBooks[idx];
    if (!book) return;
    const s = Number(row.querySelector(".chapter-start")?.value);
    const e = Number(row.querySelector(".chapter-end")?.value);
    if (!s || !e) return;
    filters.push({ name: book.name, start_chapter: Math.min(s, e), end_chapter: Math.max(s, e) });
  });
  return filters;
}

// ── Generation ──
document.getElementById("generateBtn")?.addEventListener("click", async () => {
  if (!selectedPoolId) { alert("Select a pool first."); return; }

  const books = selectedBookFilters();
  lastSelectedBookFilters = books;
  if (books.length === 0) { alert("Select at least one book."); return; }

  const numSets = parseInt(document.getElementById("numSets").value) || 1;
  const useSituation = document.querySelector('input[name="questionType"]:checked')?.value === "situation";
  const seedInput = document.getElementById("seedInput").value;
  const seed = seedInput ? parseInt(seedInput) : null;

  const btn = document.getElementById("generateBtn");
  const status = document.getElementById("generateStatus");
  btn.disabled = true;
  status.textContent = "Generating…";

  try {
    const sets = [];
    const names = [];
    const prefix = document.getElementById("setNamePrefix")?.value || "SET #";

    for (let i = 0; i < numSets; i++) {
      const data = await api("/api/generate", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          pool_id: selectedPoolId,
          question_type: "standard",
          count: 20,
          situation: useSituation,
          seed: seed == null ? null : seed + i,
          books,
        }),
      });
      sets.push(data.questions || []);
      names.push(`${prefix}${i + 1}`);
    }

    generatedSets = sets;
    generatedRoundNames = names;
    displayResults(sets, prefix);
    status.textContent = "";
  } catch (err) {
    status.textContent = `Error: ${err.message}`;
  } finally {
    btn.disabled = false;
  }
});

function displayResults(sets, prefix) {
  const container = document.getElementById("setResults");
  const html = sets.map((set, si) => {
    const label = `${prefix}${si + 1}`;
    const qs = set.map((q, qi) => `
      <div class="question-item">
        <span class="question-num">${qi + 1}.</span>
        <div class="question-text">
          ${escapeHtml(q.question)}
          <div class="question-answer">${escapeHtml(q.answer || "")} <span class="question-ref">(${escapeHtml(q.reference || "")})</span></div>
        </div>
        <span class="question-type">${escapeHtml(q.type || q.qtype || "")}</span>
      </div>
    `).join("");
    return `<div class="set-heading"><h3>${escapeHtml(label)}</h3><span class="set-count">${set.length} questions</span></div>
      <div class="question-set">${qs}</div>`;
  }).join("");

  container.innerHTML = html;
  showStep("results");

  const cb = document.getElementById("showAnswers");
  if (cb) cb.checked = false;
}

function toggleAnswers() {
  const show = document.getElementById("showAnswers")?.checked;
  document.querySelectorAll(".question-answer").forEach(el => el.classList.toggle("visible", show));
}

// ── Downloads ──
function escapeRtf(text) {
  return String(text).replace(/\\/g, "\\\\").replace(/{/g, "\\{").replace(/}/g, "\\}").replace(/\n/g, "\\line ");
}

function triggerDownload(filename, content, mimeType) {
  const blob = new Blob([content], { type: mimeType });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

function generateSectionTag(filters) {
  if (!filters || filters.length === 0) return "Generated Set";
  return filters.map(f => f.start_chapter === f.end_chapter ? `${f.name} ${f.start_chapter}` : `${f.name} ${f.start_chapter}-${f.end_chapter}`).join(", ");
}

function buildRtf(roundNames, sets, sectionTag) {
  const abbrev = { "According-To": "A", General: "G", "In-What-Book-and-Chapter": "I", Quote: "Q", Reference: "R", Situation: "S", Context: "X", Verse: "V" };
  const header = `{\\rtf1\\ansi\\ansicpg1252\\deff0{\\fonttbl{\\f0\\fswiss Arial;}{\\f1\\froman Times New Roman;}}{\\header \\pard\\plain\\qr \\fs16 ${escapeRtf(sectionTag)}\\par}{\\footer \\pard\\plain\\ql \\fs14 ${escapeRtf(sectionTag)}\\par}\\viewkind4\\uc1\\pard\\f0\\fs22 `;
  const body = sets.map((set, si) => {
    const name = roundNames[si] || `SET #${si + 1}`;
    const h = `\\pard\\qc\\b\\fs28 ${escapeRtf(name)}\\b0\\fs22\\par\\par`;
    const qs = set.map((q, qi) => {
      const t = abbrev[q.type || q.qtype || "General"] || "G";
      return `\\pard\\fi-360\\li360\\fs22 ${t}\\tab ${qi + 1}. ${escapeRtf(q.question || "")}\\par\\tab A. ${escapeRtf(q.answer || "")} (${escapeRtf(q.reference || "")})\\par\\par`;
    }).join("");
    return `${h}${qs}`;
  }).join("\\par\\page\\par");
  return `${header}${body}}`;
}

function buildQset(roundNames, sets) {
  const out = {};
  sets.forEach((set, i) => {
    const name = roundNames[i] || `SET #${i + 1}`;
    out[name] = { round: name, questions: set.map((q, qi) => ({ number: qi + 1, type: q.type, reference: q.reference })) };
  });
  return JSON.stringify(out, null, 2);
}

function downloadRTF() {
  if (!generatedSets.length) { alert("Generate sets first."); return; }
  triggerDownload("qsets_generated.rtf", buildRtf(generatedRoundNames, generatedSets, generateSectionTag(lastSelectedBookFilters)), "application/rtf");
}

function downloadJSON() {
  if (!generatedSets.length) { alert("Generate sets first."); return; }
  triggerDownload("qsets_generated.qset", buildQset(generatedRoundNames, generatedSets), "application/json");
}

function escapeHtml(text) {
  const map = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" };
  return String(text).replace(/[&<>"']/g, m => map[m]);
}

