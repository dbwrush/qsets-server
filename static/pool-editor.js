const $ = id => document.getElementById(id);
const TYPES = ["General", "According-To", "Situation", "In-What-Book-and-Chapter", "Quote", "Reference", "Verse", "Context"];
let id = location.pathname.match(/^\/pools\/(\d+)\/edit$/)?.[1];
let version = null;
let dirty = false;
let saving = false;
let nextQuestionId = 0;

async function api(url, options = {}) {
  const response = await fetch(url, {
    ...options,
    headers: { "content-type": "application/json", "x-csrf-token": document.body.dataset.csrf },
  });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || "Request failed");
  return body;
}

// The questions live in `rows`. The list is windowed: every row has a fixed height, except the one
// open for editing, and only the rows near the screen are in the DOM. A pool of any size scrolls
// like one long page.
const ROW_H = 44;
const OVERSCAN = 10;
const rows = [];
let view = [];            // indexes into `rows` that match the search and type filter
let open = null;          // the row object open for editing, if any
let openExtra = 0;        // how much taller the open row is than ROW_H
const mounted = new Map(); // row index -> element currently in the list

const blankQuestion = () => ({ question: "", type: "General", reference: "", answer: "" });

function setStatus(text, kind = "") {
  $("saveStatus").textContent = text;
  $("saveStatus").className = `editor-save-status ${kind}`.trim();
  $("editorStatus").textContent = text;
  $("editorStatus").className = kind;
}

function matches(row) {
  const type = $("filterType").value;
  if (type && row.type.toLowerCase() !== type.toLowerCase()) return false;
  const text = $("filterText").value.trim().toLowerCase();
  return !text || [row.question, row.reference, row.answer].some(value => value.toLowerCase().includes(text));
}

function offsetOf(position) {
  const openPosition = open ? view.indexOf(rows.indexOf(open)) : -1;
  return position * ROW_H + (openPosition >= 0 && openPosition < position ? openExtra : 0);
}

function totalHeight() {
  const openVisible = open && view.includes(rows.indexOf(open));
  return view.length * ROW_H + (openVisible ? openExtra : 0);
}

function positionAt(y) {
  let low = 0, high = view.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (offsetOf(mid + 1) <= y) low = mid + 1; else high = mid;
  }
  return Math.min(low, Math.max(0, view.length - 1));
}

function refilter() {
  view = [];
  rows.forEach((row, i) => { if (matches(row)) view.push(i); });
  const count = view.length === rows.length ? `${rows.length} questions` : `${view.length} of ${rows.length} questions`;
  $("questionCount").textContent = count;
  renderWindow(true);
}

function renderWindow(reset = false) {
  const list = $("editorQuestions");
  if (reset) {
    for (const el of mounted.values()) el.remove();
    mounted.clear();
  }
  list.style.height = `${totalHeight()}px`;
  if (!view.length) return;
  const bar = $("editorBar").getBoundingClientRect().height;
  const top = Math.max(0, -list.getBoundingClientRect().top + bar);
  const first = Math.max(0, positionAt(top) - OVERSCAN);
  const last = Math.min(view.length - 1, positionAt(top + window.innerHeight) + OVERSCAN);
  const wanted = new Set();
  for (let position = first; position <= last; position++) wanted.add(view[position]);
  for (const [index, el] of mounted) {
    if (!wanted.has(index) && rows[index] !== open) { el.remove(); mounted.delete(index); }
  }
  for (let position = first; position <= last; position++) {
    const index = view[position];
    let el = mounted.get(index);
    if (!el) {
      el = rows[index] === open ? openRow(index) : summaryRow(index);
      mounted.set(index, el);
      list.append(el);
    }
    el.style.top = `${offsetOf(position)}px`;
  }
  // The open row may have scrolled out of range and been kept; place it too.
  if (open) {
    const index = rows.indexOf(open);
    const el = mounted.get(index);
    const position = view.indexOf(index);
    if (el && position >= 0) el.style.top = `${offsetOf(position)}px`;
  }
}

function summaryRow(index) {
  const row = rows[index];
  const el = document.createElement("div");
  el.className = "editor-row";
  el.style.height = `${ROW_H}px`;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "editor-row-summary";
  button.setAttribute("aria-label", `Edit question ${index + 1}`);
  for (const [cls, text] of [
    ["editor-row-num", String(index + 1)],
    ["editor-row-type", row.type.replaceAll("-", " ")],
    ["editor-row-text", row.question],
    ["editor-row-ref", row.reference],
  ]) {
    const part = document.createElement("span");
    part.className = cls;
    part.textContent = text;
    button.append(part);
  }
  button.addEventListener("click", () => openQuestion(index));
  el.append(button);
  return el;
}

function openRow(index) {
  const el = document.createElement("div");
  el.className = "editor-row editor-row-open";
  el.append(questionCard(index));
  return el;
}

function openQuestion(index, { focus = true } = {}) {
  closeQuestion();
  open = rows[index];
  openExtra = 0;
  mounted.get(index)?.remove();
  mounted.delete(index);
  renderWindow();
  const el = mounted.get(index);
  if (!el) return;
  measureOpen();
  if (focus) el.querySelector("textarea")?.focus();
}

function closeQuestion() {
  if (!open) return;
  const index = rows.indexOf(open);
  open = null;
  openExtra = 0;
  mounted.get(index)?.remove();
  mounted.delete(index);
  renderWindow();
}

// The open row's height is only known once it is laid out, so measure and make room for it.
function measureOpen() {
  if (!open) return;
  const el = mounted.get(rows.indexOf(open));
  if (!el) return;
  el.style.height = "auto";
  const extra = Math.max(0, el.getBoundingClientRect().height - ROW_H);
  if (Math.abs(extra - openExtra) > 1) { openExtra = extra; renderWindow(); }
}

function questionCard(index) {
  const question = rows[index];
  const card = document.createElement("fieldset");
  card.className = "editor-question";
  const legend = document.createElement("legend");
  legend.textContent = `Question ${index + 1}`;
  card.append(legend);
  const fields = document.createElement("div");
  fields.className = "editor-question-fields";
  const questionId = ++nextQuestionId;
  for (const [key, label] of [["question", "Question"], ["type", "Type"], ["reference", "Reference"], ["answer", "Answer"]]) {
    const wrapper = document.createElement("div");
    wrapper.className = `form-group editor-field-${key}`;
    const fieldLabel = document.createElement("label");
    fieldLabel.textContent = label;
    fieldLabel.htmlFor = `question-${questionId}-${key}`;
    const input = document.createElement(key === "type" ? "select" : key === "reference" ? "input" : "textarea");
    input.id = fieldLabel.htmlFor;
    input.dataset.field = key;
    input.required = true;
    if (key === "type") {
      const types = [...TYPES];
      if (!types.some(t => t.toLowerCase() === question.type.toLowerCase())) types.push(question.type);
      for (const type of types) {
        const option = document.createElement("option");
        option.value = type;
        option.textContent = type.replaceAll("-", " ");
        option.selected = type.toLowerCase() === question.type.toLowerCase();
        input.append(option);
      }
    } else {
      input.value = question[key];
      if (key === "reference") {
        input.type = "text";
        input.placeholder = "John 1:1";
      }
      else input.rows = 2;
    }
    input.addEventListener("input", () => { question[key] = input.value; if (key !== "type") requestAnimationFrame(measureOpen); });
    wrapper.append(fieldLabel, input);
    fields.append(wrapper);
  }
  card.append(fields);
  const actions = document.createElement("div");
  actions.className = "question-actions";
  const done = document.createElement("button");
  done.type = "button";
  done.className = "btn btn-ghost";
  done.textContent = "Done";
  done.addEventListener("click", () => { closeQuestion(); refilter(); });
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "btn-ghost btn-danger";
  remove.textContent = "Remove question";
  remove.addEventListener("click", () => {
    if (!confirm("Remove this question?")) return;
    open = null;
    openExtra = 0;
    rows.splice(rows.indexOf(question), 1);
    dirty = true;
    refilter();
  });
  actions.append(done, remove);
  card.append(actions);
  return card;
}

function firstIncompleteRow() {
  return rows.findIndex(row => !row.question.trim() || !row.type.trim() || !row.reference.trim() || !row.answer.trim());
}

// Scroll so the row at `index` is just under the toolbar, wherever it is in the list.
function revealRow(index) {
  const position = view.indexOf(index);
  if (position < 0) return;
  const bar = $("editorBar").getBoundingClientRect().height;
  const listTop = $("editorQuestions").getBoundingClientRect().top + window.scrollY;
  window.scrollTo({ top: listTop + offsetOf(position) - bar - 8 });
  renderWindow();
}

$("addQuestion").addEventListener("click", () => {
  $("filterText").value = "";
  $("filterType").value = "";
  rows.push(blankQuestion());
  dirty = true;
  refilter();
  const index = rows.length - 1;
  revealRow(index);
  openQuestion(index);
  revealRow(index);
});
let filterTimer = 0;
for (const filter of ["filterText", "filterType"]) {
  $(filter).addEventListener("input", () => {
    clearTimeout(filterTimer);
    filterTimer = setTimeout(() => {
      closeQuestion();
      refilter();
      const list = $("editorQuestions").getBoundingClientRect().top + window.scrollY;
      if (window.scrollY > list) window.scrollTo({ top: list - $("editorBar").getBoundingClientRect().height });
    }, 120);
  });
}
let scrollQueued = false;
window.addEventListener("scroll", () => {
  if (scrollQueued) return;
  scrollQueued = true;
  requestAnimationFrame(() => { scrollQueued = false; renderWindow(); });
}, { passive: true });
window.addEventListener("resize", () => { measureOpen(); renderWindow(); });
$("poolEditor").addEventListener("input", event => {
  if (!event.target.closest("#editorBar")) dirty = true;
});
window.addEventListener("beforeunload", event => {
  if (dirty) {
    event.preventDefault();
    event.returnValue = "";
  }
});
$("poolEditor").addEventListener("submit", async event => {
  event.preventDefault();
  if (saving) return;
  const incomplete = firstIncompleteRow();
  if (incomplete >= 0) {
    $("filterText").value = "";
    $("filterType").value = "";
    refilter();
    openQuestion(incomplete, { focus: false });
    revealRow(incomplete);
    mounted.get(incomplete)?.querySelector("[data-field]:invalid")?.reportValidity();
    setStatus(`Question ${incomplete + 1} has an empty field.`, "error");
    return;
  }
  const questions = rows.map(row => ({ ...row }));
  saving = true;
  $("poolEditor").inert = true;
  setStatus("Saving...");
  try {
    const data = await api(id ? `/api/pools/${id}/editor` : "/api/pools/create", {
      method: id ? "PUT" : "POST",
      body: JSON.stringify({
        name: $("poolName").value, tier_id: Number($("poolTier").value), questions, updated_at: version,
        popup_message: $("poolPopup").value, print_message: $("poolPrint").value,
      }),
    });
    id = String(data.id);
    version = data.updated_at;
    dirty = false;
    history.replaceState(null, "", `/pools/${id}/edit`);
    $("editorTitle").textContent = "Edit pool";
    setStatus("Pool saved.", "success");
  } catch (error) {
    setStatus(error.message, "error");
  } finally {
    saving = false;
    $("poolEditor").inert = false;
  }
});

try {
  let tiers;
  if (id) {
    const pool = await api(`/api/pools/${id}/editor`);
    $("poolName").value = pool.name;
    $("poolPopup").value = pool.popup_message || "";
    $("poolPrint").value = pool.print_message || "";
    version = pool.updated_at;
    tiers = pool.tiers;
    rows.push(...pool.questions);
    $("editorTitle").textContent = "Edit pool";
    for (const tier of tiers) {
      const option = new Option(tier.name, tier.id, false, tier.id === pool.tier_id);
      $("poolTier").append(option);
    }
  } else {
    const data = await api("/api/tiers");
    if (!data.can_upload_pools) throw new Error("You do not have permission to create question pools.");
    tiers = data.tiers;
    for (const tier of tiers) $("poolTier").append(new Option(tier.name, tier.id));
    rows.push(blankQuestion());
    $("editorTitle").textContent = "Create pool";
  }
  for (const type of TYPES) $("filterType").append(new Option(type.replaceAll("-", " "), type));
  $("poolEditor").hidden = false;
  refilter();
  $("editorStatus").textContent = "";
} catch (error) {
  $("editorStatus").textContent = error.message;
  $("editorStatus").className = "error";
}
