import { QUESTION_MS, answersMatch, timing, summarize } from "./study-session.mjs?v=study-30s-1";
import { selectQuestions } from "./study-core.mjs?v=offline-1";
import { syncDeviceVault, wipeDeviceVault } from "./vault.mjs?v=offline-1";

const $ = id => document.getElementById(id);
const TYPES = ["General", "According-To", "Situation", "In-What-Book-and-Chapter", "Quote", "Reference", "Verse", "Context"];
let questions = [];
let results = [];
let index = 0;
let started = 0;
let interval;
let pending = null;
let reviewQuestions = [];
let passageVersion = 0;
let active = false;

// Installed app only: an encrypted copy of the pools in use, so sessions also work offline.
// `vault` is null in a browser tab, where questions always come from the server.
let vault = null;
let offline = document.body.dataset.offlineShell === "true";
let pools = new Map();
let localPool = null;

async function api(url, body) {
  const response = await fetch(url, body === undefined ? {} : {
    method: "POST",
    headers: { "content-type": "application/json", "x-csrf-token": document.body.dataset.csrf },
    body: JSON.stringify(body),
  });
  const data = await response.json();
  if (!response.ok) {
    // The session is gone, so the copy on this device no longer belongs to anyone.
    if (response.status === 401) await wipeDeviceVault();
    const error = new Error(data.error || "Request failed");
    error.status = response.status;
    throw error;
  }
  return data;
}

// The device copy of a pool, downloading or refreshing it first when online. Null when the
// pool cannot be kept on this device, in which case the server is used instead.
async function localCopy(summary) {
  if (!vault) return null;
  try {
    const copy = await vault.loadPool(summary.id);
    if (offline || (copy && copy.updated_at === summary.updated_at)) return copy;
    $("studySetupStatus").textContent = "Saving for offline use...";
    const data = await api(`/api/pools/${summary.id}/questions`);
    await vault.savePool({ ...data.pool, books: data.books, questions: data.questions });
    return await vault.loadPool(summary.id);
  } catch (error) {
    if (error instanceof TypeError) {
      offline = true;
      return vault.loadPool(summary.id);
    }
    if (error.status) throw error;
    console.error("Could not keep this pool on the device:", error);
    return null;
  }
}

// Pools the user can no longer reach are removed from the device.
async function dropRevokedCopies(available) {
  const ids = new Set(available.map(pool => String(pool.id)));
  for (const meta of await vault.pools()) {
    if (!ids.has(String(meta.id))) await vault.dropPool(meta.id);
  }
}

function show(section) {
  for (const id of ["studySetup", "studyQuestion", "studySummary"]) $(id).hidden = id !== section;
  $("studyIntro").hidden = section !== "studySetup";
}

async function loadPassages() {
  const version = ++passageVersion;
  $("studyStart").disabled = true;
  $("studyPassages").replaceChildren();
  $("studySetupStatus").textContent = "Loading passages...";
  try {
    const summary = pools.get(Number($("studyPool").value));
    localPool = summary ? await localCopy(summary) : null;
    if (offline && !localPool) throw new Error("This pool is not saved on this device.");
    const data = localPool ? { books: localPool.books } : await api(`/api/pools/${$("studyPool").value}/books`);
    if (version !== passageVersion) return;
    for (const book of data.books) {
      const row = document.createElement("div");
      row.className = "study-passage";
      row.dataset.book = book.name;
      const label = document.createElement("label");
      const enabled = document.createElement("input");
      enabled.type = "checkbox";
      enabled.checked = true;
      label.append(enabled, ` ${book.name}`);
      row.append(label);
      for (const [name, value] of [["From", book.min_chapter], ["To", book.max_chapter]]) {
        const boundLabel = document.createElement("label");
        boundLabel.textContent = `${name} `;
        const input = document.createElement("input");
        input.type = "number";
        input.min = book.min_chapter;
        input.max = book.max_chapter;
        input.value = value;
        input.required = true;
        boundLabel.append(input);
        row.append(boundLabel);
      }
      enabled.addEventListener("change", () => {
        row.querySelectorAll('input[type="number"]').forEach(input => { input.disabled = !enabled.checked; });
      });
      $("studyPassages").append(row);
    }
    $("studySetupStatus").textContent = data.books.length ? (offline ? "Offline: using pools saved on this device." : "") : "No passages available in this pool.";
    $("studyStart").disabled = !data.books.length;
  } catch (error) {
    if (version === passageVersion) $("studySetupStatus").textContent = error.message;
  }
}

for (const type of TYPES) {
  const label = document.createElement("label");
  const input = document.createElement("input");
  input.type = "checkbox";
  input.value = type;
  input.checked = true;
  label.append(input, ` ${type.replaceAll("-", " ")}`);
  $("studyTypes").append(label);
}

$("studyPool").addEventListener("change", loadPassages);
$("studySetupForm").addEventListener("submit", async event => {
  event.preventDefault();
  const books = [...$("studyPassages").children].filter(row => row.querySelector('input[type="checkbox"]').checked).map(row => {
    const [from, to] = row.querySelectorAll('input[type="number"]');
    return { name: row.dataset.book, start_chapter: Number(from.value), end_chapter: Number(to.value) };
  });
  const types = [...$("studyTypes").querySelectorAll("input:checked")].map(input => input.value);
  const count = Number($("studyCount").value);
  if (!books.length || !types.length || !Number.isSafeInteger(count) || count < 0) {
    $("studySetupStatus").textContent = "Select passages, at least one type, and a nonnegative whole-number session length.";
    return;
  }
  $("studyStart").disabled = true;
  $("studySetupStatus").textContent = "Loading questions...";
  try {
    const selected = localPool
      ? selectQuestions(localPool.questions, { books, types, count })
      : (await api("/api/study/questions", { pool_id: Number($("studyPool").value), books, types, count })).questions;
    begin(selected);
    $("studySetupStatus").textContent = "";
  } catch (error) {
    $("studySetupStatus").textContent = error.message;
  } finally {
    $("studyStart").disabled = false;
  }
});

function begin(selected) {
  questions = selected;
  results = [];
  reviewQuestions = [];
  index = 0;
  active = true;
  show("studyQuestion");
  nextQuestion();
}

function tick() {
  const remaining = Math.max(0, QUESTION_MS - (performance.now() - started));
  $("studyTimer").textContent = `${Math.ceil(remaining / 1000)}s`;
  $("studyTimer").classList.toggle("study-time-expired", remaining === 0);
  if (remaining === 0) {
    $("studyExpired").textContent = "Time’s up";
    clearInterval(interval);
  }
}

function nextQuestion() {
  clearInterval(interval);
  pending = null;
  $("studyProgress").textContent = `Question ${index + 1} of ${questions.length}`;
  $("studyPrompt").textContent = questions[index].question;
  $("studyType").textContent = questions[index].type.replaceAll("-", " ");
  $("studyTimer").hidden = false;
  $("studyAnswer").value = "";
  $("studyAnswerForm").hidden = false;
  $("studyFeedback").hidden = true;
  $("studySelfGrade").hidden = true;
  $("studyNext").hidden = true;
  $("studyExpired").textContent = "";
  $("studyPrompt").focus();
  $("studyAnswer").focus();
  started = performance.now();
  tick();
  interval = setInterval(tick, 100);
}

function submit(skipped) {
  if (pending || !active) return;
  const submitted = performance.now();
  clearInterval(interval);
  const answer = $("studyAnswer").value;
  pending = {
    question: questions[index], answer, ...timing(started, submitted),
    correct: !skipped && answersMatch(answer, questions[index].answer),
    skipped, selfAssessed: false,
  };
  $("studyAnswerForm").hidden = true;
  $("studyFeedback").hidden = false;
  $("studyTimer").hidden = true;
  $("studyExpired").textContent = "";
  $("studySubmittedCard").hidden = skipped;
  $("studySubmitted").textContent = answer;
  $("studyTiming").textContent = skipped ? "" : `${(pending.elapsed / 1000).toFixed(1)}s · ${pending.onTime ? "On time" : "Late"}`;
  $("studyTiming").classList.toggle("study-time-expired", !pending.onTime && !skipped);
  $("studyExpected").textContent = questions[index].answer;
  $("studyReference").textContent = questions[index].reference;
  if (pending.correct || skipped) {
    $("studyAssessment").textContent = pending.correct ? "Correct" : "Skipped";
    record();
  } else {
    $("studyAssessment").textContent = "Check your answer";
    $("studySelfGrade").hidden = false;
    $("studyCorrect").focus();
  }
}

function record() {
  results.push(pending);
  $("studySelfGrade").hidden = true;
  $("studyNext").hidden = false;
  $("studyNext").textContent = index + 1 === questions.length ? "See results" : "Next question";
  $("studyNext").focus();
}

$("studyAnswerForm").addEventListener("submit", event => { event.preventDefault(); submit(false); });
$("studyReveal").addEventListener("click", () => submit(true));
for (const [id, correct] of [["studyCorrect", true], ["studyIncorrect", false]]) {
  $(id).addEventListener("click", () => {
    if (!pending || $("studySelfGrade").hidden) return;
    pending.correct = correct;
    pending.selfAssessed = true;
    $("studyAssessment").textContent = correct ? "Correct" : "Needs practice";
    record();
  });
}
$("studyNext").addEventListener("click", () => {
  if (++index === questions.length) finish();
  else nextQuestion();
});

function finish() {
  clearInterval(interval);
  active = false;
  const stats = summarize(results);
  reviewQuestions = results.filter(r => !r.correct || !r.onTime).map(r => r.question);
  $("studySummaryText").textContent = `${stats.answered} of ${questions.length} completed`;
  $("studyStats").replaceChildren();
  for (const [label, value] of [
    ["Correct", `${stats.correct} / ${stats.answered}`],
    [`Correct in ${QUESTION_MS / 1000}s`, `${stats.onTime} / ${stats.answered}`],
    ["Correct, late", stats.late], ["Incorrect", stats.incorrect],
    ["Skipped", stats.skipped], ["Self-marked correct", stats.selfCorrected],
  ]) {
    const term = document.createElement("dt");
    const description = document.createElement("dd");
    term.textContent = label;
    description.textContent = value;
    const item = document.createElement("div");
    item.append(term, description);
    $("studyStats").append(item);
  }
  $("studyReview").disabled = reviewQuestions.length === 0;
  show("studySummary");
  $("studySummaryTitle").focus();
}

$("studyFinish").addEventListener("click", () => {
  if (!confirm("End session?")) return;
  finish();
});
$("studyReview").addEventListener("click", () => {
  const review = [...reviewQuestions];
  for (let i = review.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [review[i], review[j]] = [review[j], review[i]];
  }
  begin(review);
});
$("studyAgain").addEventListener("click", () => {
  questions = [];
  results = [];
  pending = null;
  reviewQuestions = [];
  show("studySetup");
});

async function init() {
  if (offline) {
    // Opened from the offline copy of this page: nothing can be fetched, so go straight to the device.
    ({ vault } = await syncDeviceVault(() => Promise.reject(new TypeError("offline"))));
  } else {
    const synced = await syncDeviceVault(async () => {
      try {
        return await api("/api/me");
      } catch (error) {
        if (error.status === 401) return null;
        throw error;
      }
    });
    // Signed out: no copy. Unreachable (offline) keeps it, and the stored lease decides.
    vault = synced.me || synced.offline ? synced.vault : null;
    offline = synced.offline;
  }

  let list;
  if (!offline) {
    try {
      list = (await api("/api/pools")).pools;
      if (vault) await dropRevokedCopies(list);
    } catch (error) {
      if (!(error instanceof TypeError && vault)) throw error;
      offline = true;
    }
  }
  if (offline) {
    list = vault ? await vault.pools() : [];
    if (!list.length) {
      $("studySetupStatus").textContent = vault
        ? "You're offline and no pools are saved on this device yet. Open the pools you want while online first."
        : "You're offline. Offline study is available in the installed app once you've signed in.";
      return;
    }
  }

  pools = new Map(list.map(pool => [pool.id, pool]));
  for (const pool of list) {
    const option = document.createElement("option");
    option.value = pool.id;
    option.textContent = `${pool.name} (${pool.tier_name})`;
    $("studyPool").append(option);
  }
  if (list.length) {
    $("studyPool").disabled = false;
    await loadPassages();
  } else {
    $("studySetupStatus").textContent = "No pools available.";
  }
}

try {
  await init();
} catch (error) {
  $("studySetupStatus").textContent = `Could not load pools: ${error.message}`;
}
