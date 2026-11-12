// Question sets kept on this device. A dataset is a named group of imported files; its pool is
// every question from its files with repeats removed. Nothing here touches the network:
// datasets live in this browser's IndexedDB (or in memory, for tests).

import { dedupeQuestions } from "./rtf-import.mjs";
import { listBooks, availableTypeCounts } from "./generator-core.mjs";

const DB_NAME = "qsets-local";
const STORE = "datasets";

export function memoryBackend() {
  const rows = new Map();
  return {
    async all() { return [...rows.values()].map(row => structuredClone(row)); },
    async put(row) { rows.set(row.id, structuredClone(row)); },
    async remove(id) { rows.delete(id); },
  };
}

export function indexedDbBackend(factory = globalThis.indexedDB) {
  let opened;
  const open = () => opened ||= new Promise((resolve, reject) => {
    const request = factory.open(DB_NAME, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE, { keyPath: "id" });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  const run = async (mode, action) => {
    const db = await open();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(STORE, mode);
      const request = action(tx.objectStore(STORE));
      tx.oncomplete = () => resolve(request.result);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  };
  return {
    all: () => run("readonly", store => store.getAll()),
    put: row => run("readwrite", store => store.put(row)),
    remove: id => run("readwrite", store => store.delete(id)),
  };
}

export function createStore(backend = indexedDbBackend()) {
  const newId = () => (globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`);

  async function load(id) {
    const row = (await backend.all()).find(item => item.id === id);
    if (!row) throw new Error("That question set is no longer on this device.");
    return row;
  }

  return {
    async list() {
      const rows = await backend.all();
      return rows.sort((a, b) => a.created - b.created).map(summary);
    },
    async create(name) {
      const row = { id: newId(), name: cleanName(name), created: Date.now(), files: [] };
      await backend.put(row);
      return summary(row);
    },
    async rename(id, name) {
      const row = await load(id);
      row.name = cleanName(name);
      await backend.put(row);
    },
    async remove(id) { await backend.remove(id); },
    async addFile(id, fileName, questions) {
      const row = await load(id);
      row.files = row.files.filter(file => file.name !== fileName);
      row.files.push({ name: fileName, added: Date.now(), questions });
      await backend.put(row);
      return summary(row);
    },
    async removeFile(id, fileName) {
      const row = await load(id);
      row.files = row.files.filter(file => file.name !== fileName);
      await backend.put(row);
    },
    async files(id) {
      const row = await load(id);
      return row.files.map(file => ({ name: file.name, added: file.added, count: file.questions.length }));
    },
    // The dataset's combined pool: { id, name, questions, books, type_counts }.
    async pool(id) {
      const row = await load(id);
      const questions = dedupeQuestions(row.files.flatMap(file => file.questions));
      return { id: row.id, name: row.name, questions, books: listBooks(questions), type_counts: availableTypeCounts(questions) };
    },
  };
}

function cleanName(name) {
  const cleaned = String(name ?? "").replace(/\s+/gu, " ").trim();
  if (!cleaned) throw new Error("Give the set a name.");
  return cleaned.slice(0, 80);
}

function summary(row) {
  const questions = dedupeQuestions(row.files.flatMap(file => file.questions));
  return { id: row.id, name: row.name, created: row.created, files: row.files.length, questions: questions.length };
}
