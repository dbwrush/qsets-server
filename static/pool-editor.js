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

function addQuestion(question = { question: "", type: "General", reference: "", answer: "" }) {
  const card = document.createElement("fieldset");
  card.className = "editor-question";
  const legend = document.createElement("legend");
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
    wrapper.append(fieldLabel, input);
    fields.append(wrapper);
  }
  card.append(fields);
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "btn-ghost btn-danger";
  remove.textContent = "Remove question";
  remove.addEventListener("click", () => {
    if (!confirm("Remove this question?")) return;
    card.remove();
    dirty = true;
    renumber();
  });
  const actions = document.createElement("div");
  actions.className = "question-actions";
  actions.append(remove);
  card.append(actions);
  $("editorQuestions").append(card);
  renumber();
  return card;
}

function renumber() {
  [...$("editorQuestions").children].forEach((card, i) => {
    card.querySelector("legend").textContent = `Question ${i + 1}`;
  });
}

$("addQuestion").addEventListener("click", () => {
  const card = addQuestion();
  dirty = true;
  card.querySelector("textarea").focus();
});
$("poolEditor").addEventListener("input", () => { dirty = true; });
window.addEventListener("beforeunload", event => {
  if (dirty) {
    event.preventDefault();
    event.returnValue = "";
  }
});
$("poolEditor").addEventListener("submit", async event => {
  event.preventDefault();
  if (saving) return;
  const questions = [...$("editorQuestions").children].map(card =>
    Object.fromEntries([...card.querySelectorAll("[data-field]")].map(input => [input.dataset.field, input.value])));
  saving = true;
  $("editorFields").disabled = true;
  $("editorStatus").textContent = "Saving...";
  $("editorStatus").className = "";
  try {
    const data = await api(id ? `/api/pools/${id}/editor` : "/api/pools/create", {
      method: id ? "PUT" : "POST",
      body: JSON.stringify({
        name: $("poolName").value, tier_id: Number($("poolTier").value), questions, updated_at: version,
      }),
    });
    id = String(data.id);
    version = data.updated_at;
    dirty = false;
    history.replaceState(null, "", `/pools/${id}/edit`);
    $("editorTitle").textContent = "Edit pool";
    $("editorStatus").textContent = "Pool saved.";
    $("editorStatus").className = "success";
  } catch (error) {
    $("editorStatus").textContent = error.message;
    $("editorStatus").className = "error";
  } finally {
    saving = false;
    $("editorFields").disabled = false;
  }
});

try {
  let tiers;
  if (id) {
    const pool = await api(`/api/pools/${id}/editor`);
    $("poolName").value = pool.name;
    version = pool.updated_at;
    tiers = pool.tiers;
    for (const question of pool.questions) addQuestion(question);
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
    addQuestion();
    $("editorTitle").textContent = "Create pool";
  }
  $("poolEditor").hidden = false;
  $("editorStatus").textContent = "";
} catch (error) {
  $("editorStatus").textContent = error.message;
  $("editorStatus").className = "error";
}
