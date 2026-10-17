import { QUESTION_MS, answersMatch, timing, summarize } from "./study-session.mjs?v=study-30s-1";

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

async function api(url, body) {
  const response = await fetch(url, body === undefined ? {} : {
    method: "POST",
    headers: { "content-type": "application/json", "x-csrf-token": document.body.dataset.csrf },
    body: JSON.stringify(body),
  });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || "Request failed");
  return data;
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
    const data = await api(`/api/pools/${$("studyPool").value}/books`);
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
    $("studySetupStatus").textContent = data.books.length ? "" : "No passages available in this pool.";
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
    const data = await api("/api/study/questions", { pool_id: Number($("studyPool").value), books, types, count });
    begin(data.questions);
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

try {
  const data = await api("/api/pools");
  for (const pool of data.pools) {
    const option = document.createElement("option");
    option.value = pool.id;
    option.textContent = `${pool.name} (${pool.tier_name})`;
    $("studyPool").append(option);
  }
  if (data.pools.length) {
    $("studyPool").disabled = false;
    await loadPassages();
  } else {
    $("studySetupStatus").textContent = "No pools available.";
  }
} catch (error) {
  $("studySetupStatus").textContent = `Could not load pools: ${error.message}`;
}
