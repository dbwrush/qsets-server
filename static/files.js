// The "Your files" page: question set files are read and stored in this browser only.
import { rtfToText } from "./rtf-text.mjs?v=local-1";
import { parseQuestionSets, dedupeQuestions } from "./rtf-import.mjs?v=local-1";
import { createStore } from "./local-store.mjs?v=local-1";

const $ = id => document.getElementById(id);
const store = createStore();

function say(message, isError = false) {
  const status = $("filesStatus");
  status.textContent = message;
  status.className = isError ? "msg-error" : "";
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value);
  }
  node.append(...children);
  return node;
}

// RTF is 7-bit text, so Latin-1 keeps every byte intact for the reader.
const readFile = file => file.arrayBuffer().then(buffer => new TextDecoder("latin1").decode(buffer));

async function importFiles(set, files) {
  const notes = [];
  for (const file of files) {
    try {
      const text = await readFile(file);
      if (!text.trimStart().startsWith("{\\rtf")) {
        notes.push(`${file.name}: not an RTF file.`);
        continue;
      }
      const { questions, warnings } = parseQuestionSets(rtfToText(text));
      if (!questions.length) {
        notes.push(`${file.name}: no questions found.`);
        continue;
      }
      await store.addFile(set.id, file.name, dedupeQuestions(questions));
      notes.push(`${file.name}: added ${dedupeQuestions(questions).length} questions${warnings.length ? `, ${warnings.length} skipped` : ""}.`);
    } catch (error) {
      notes.push(`${file.name}: ${error.message}`);
    }
  }
  say(notes.join(" "), notes.some(note => /not an RTF|no questions|Error/u.test(note)));
  await render();
}

async function card(set) {
  const files = await store.files(set.id);
  const list = el("ul", { class: "local-files" }, ...files.map(file => el("li", {},
    `${file.name} — ${file.count} questions `,
    el("button", {
      type: "button", class: "btn-ghost",
      onclick: async () => { await store.removeFile(set.id, file.name); await render(); },
    }, "Remove"))));
  const input = el("input", { type: "file", accept: ".rtf,application/rtf,text/rtf", multiple: "", id: `file-${set.id}` });
  input.addEventListener("change", () => importFiles(set, [...input.files]));
  const zone = el("div", { class: "local-drop", tabindex: "-1" },
    el("label", { for: `file-${set.id}` }, "Choose .rtf files or drop them here"), input);
  zone.addEventListener("dragover", event => { event.preventDefault(); zone.classList.add("dragging"); });
  zone.addEventListener("dragleave", () => zone.classList.remove("dragging"));
  zone.addEventListener("drop", event => {
    event.preventDefault();
    zone.classList.remove("dragging");
    importFiles(set, [...event.dataTransfer.files]);
  });
  return el("section", { class: "panel" },
    el("h2", {}, set.name),
    el("p", { class: "msg-info" }, `${set.questions} questions from ${set.files} file${set.files === 1 ? "" : "s"}. Repeated questions are counted once.`),
    list.children.length ? list : el("p", { class: "msg-info" }, "No files yet."),
    zone,
    el("div", { class: "study-actions" },
      el("a", { class: "btn-primary", href: "/" }, "Generate sets"),
      el("a", { class: "btn-ghost", href: "/study" }, "Study"),
      el("button", {
        type: "button", class: "btn-ghost",
        onclick: async () => {
          const name = prompt("New name", set.name);
          if (name === null) return;
          try { await store.rename(set.id, name); await render(); } catch (error) { say(error.message, true); }
        },
      }, "Rename"),
      el("button", {
        type: "button", class: "btn-ghost",
        onclick: async () => {
          if (!confirm(`Delete "${set.name}" from this device?`)) return;
          await store.remove(set.id);
          await render();
        },
      }, "Delete")));
}

async function render() {
  const container = $("localSets");
  try {
    const sets = await store.list();
    container.replaceChildren(...await Promise.all(sets.map(card)));
  } catch (error) {
    say(`This browser cannot keep files on the device: ${error.message}`, true);
  }
}

$("newSetForm").addEventListener("submit", async event => {
  event.preventDefault();
  try {
    await store.create($("newSetName").value);
    $("newSetName").value = "";
    say("");
    await render();
  } catch (error) {
    say(error.message, true);
  }
});

// Shown once per browser session before anything can be added. Cancelling leaves the page.
const NOTICE = {
  name: "Your files stay on your device",
  popup_message: [
    "Your files never leave this device. They are processed entirely in your browser and are never uploaded to, stored by, or seen by the QSets server.",
    "Only use files you have permission to use.",
  ].join("\n\n"),
};
let acknowledged = false;
try { acknowledged = sessionStorage.getItem("qsets-files-notice") === "1"; } catch { /* ask again */ }
if (!acknowledged) {
  if (!(await window.qsetsPoolNotice(NOTICE))) {
    location.href = "/";
  } else {
    try { sessionStorage.setItem("qsets-files-notice", "1"); } catch { /* ask again next time */ }
  }
}

await render();
