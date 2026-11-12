// Set generation that runs in the browser, for question sets that never leave the device.
// Follows the server's generator (src/services/generator.rs): same set shapes, same limits on
// questions per chapter and the same error messages. Random choices come from a small seeded
// generator, so a seed repeats a set here, though not the same set the server would pick.

import { parseReference, listBooks, filterQuestions, shuffle } from "./study-core.mjs";

export { listBooks };

export const VALID_TYPES = [
  "Situation", "In-What-Book-and-Chapter", "Quote", "Reference", "Verse", "Context", "According-To", "General",
];

// Small seeded generator (mulberry32). With no seed it falls back to Math.random.
export function makeRandom(seed) {
  if (seed === null || seed === undefined) return Math.random;
  let state = (Number(seed) >>> 0) ^ Math.floor(Number(seed) / 4294967296);
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const sameType = (a, b) => a.toLowerCase() === b.toLowerCase();

function chapterKey(reference) {
  const parsed = parseReference(reference);
  return parsed ? `${parsed.book} ${parsed.chapter}` : null;
}

const pickOne = (items, random) => items[Math.floor(random() * items.length)];

// Questions inside the chosen chapter ranges; no filters means the whole pool.
export function filterByBooks(pool, filters) {
  if (!filters || !filters.length) return pool;
  return filterQuestions(pool, filters, VALID_TYPES.concat(pool.map(q => q.type)));
}

export function availableTypeCounts(pool) {
  const counts = Object.fromEntries(VALID_TYPES.map(type => [type, 0]));
  for (const q of pool) {
    const type = VALID_TYPES.find(valid => sameType(valid, q.type));
    if (type) counts[type]++;
  }
  return counts;
}

function shortages(requirements, available) {
  return requirements
    .filter(([type, required]) => available(type) < required)
    .map(([type, required]) => `${type} (need ${required}, available ${available(type)})`);
}

export function generateCustomSet(pool, counts, seed = null) {
  const requirements = VALID_TYPES.map(type => [type, Number(counts[type] || 0)]);
  const total = requirements.reduce((sum, [, count]) => sum + count, 0);
  if (!Number.isSafeInteger(total)) throw new Error("Custom question total is too large");
  if (total === 0) throw new Error("Custom mode requires at least one question");
  const random = makeRandom(seed);

  const chapters = [];
  const chapterIndex = new Map();
  const buckets = requirements.map(() => []);
  for (const q of pool) {
    const key = chapterKey(q.reference);
    if (key === null) continue;
    if (!chapterIndex.has(key)) { chapterIndex.set(key, chapters.length); chapters.push(key); }
    const type = requirements.findIndex(([name]) => sameType(name, q.type));
    if (type === -1) continue;
    const chapter = chapterIndex.get(key);
    (buckets[type][chapter] ||= []).push(q);
  }
  const size = type => buckets[type].reduce((sum, list) => sum + (list ? list.length : 0), 0);
  const short = shortages(requirements, name => size(requirements.findIndex(([n]) => n === name)));
  if (short.length) {
    throw new Error(
      `Cannot generate a custom ${total}-question set from the selected books/chapters. `
      + `Insufficient question types: ${short.join("; ")}. Select more chapters, a different pool, `
      + "or reduce the requested counts.");
  }

  const maxAppearances = Math.max(2, Math.ceil(total / Math.max(chapters.length, 1)));
  const source = 0;
  const chapterStart = 1 + requirements.length;
  const sink = chapterStart + chapters.length;
  const graph = Array.from({ length: sink + 1 }, () => []);
  const addEdge = (from, to, capacity) => {
    graph[from].push({ to, reverse: graph[to].length, remaining: capacity });
    graph[to].push({ to: from, reverse: graph[from].length - 1, remaining: 0 });
  };
  const bucketEdges = [];
  const chapterOrder = shuffle(chapters.map((_, i) => i), random);
  requirements.forEach(([, count], type) => {
    addEdge(source, type + 1, count);
    for (const chapter of chapterOrder) {
      const capacity = buckets[type][chapter] ? buckets[type][chapter].length : 0;
      if (capacity > 0 && count > 0) {
        bucketEdges.push([type, chapter, graph[type + 1].length]);
        addEdge(type + 1, chapterStart + chapter, capacity);
      }
    }
  });
  for (let chapter = 0; chapter < chapters.length; chapter++) addEdge(chapterStart + chapter, sink, maxAppearances);

  // Residual paths can move earlier allocations, so a feasible distribution is not rejected
  // just because a randomly chosen chapter filled up first.
  let selected = 0;
  while (selected < total) {
    const parents = new Array(graph.length).fill(null);
    parents[source] = [source, 0];
    const queue = [source];
    for (let head = 0; head < queue.length && parents[sink] === null; head++) {
      const node = queue[head];
      graph[node].forEach((edge, index) => {
        if (edge.remaining > 0 && parents[edge.to] === null) {
          parents[edge.to] = [node, index];
          queue.push(edge.to);
        }
      });
    }
    if (parents[sink] === null) {
      throw new Error(
        `Cannot generate a custom ${total}-question set from the selected books/chapters. `
        + "The requested type counts cannot be met while respecting the limit of "
        + `${maxAppearances} questions per chapter. Select more chapters, a different pool, `
        + "or change the requested counts.");
    }
    let amount = total - selected;
    for (let node = sink; node !== source;) {
      const [from, edge] = parents[node];
      amount = Math.min(amount, graph[from][edge].remaining);
      node = from;
    }
    for (let node = sink; node !== source;) {
      const [from, edge] = parents[node];
      const reverse = graph[from][edge].reverse;
      graph[from][edge].remaining -= amount;
      graph[node][reverse].remaining += amount;
      node = from;
    }
    selected += amount;
  }

  const set = [];
  for (const [type, chapter, edge] of bucketEdges) {
    const questions = buckets[type][chapter];
    const count = questions.length - graph[type + 1][edge].remaining;
    set.push(...shuffle(questions, random).slice(0, count));
  }
  return shuffle(set, random);
}

export function generateStandardSet(pool, situation = true, seed = null) {
  const random = makeRandom(seed);
  const byType = new Map();
  pool.forEach((q, index) => {
    const key = q.type.toLowerCase();
    if (!byType.has(key)) byType.set(key, []);
    byType.get(key).push([index, q]);
  });
  const ofType = type => byType.get(type.toLowerCase()) || [];

  const firstType = situation ? "Situation" : "In-What-Book-and-Chapter";
  const requirements = [
    [firstType, 1], ["Quote", 1], ["Reference", 1], ["Verse", 1], ["Context", 1],
    ["According-To", 4], ["General", 11],
  ];
  const short = shortages(requirements, type => ofType(type).length);
  if (short.length) {
    throw new Error(
      "Cannot generate a standard 20-question set from the selected books/chapters. "
      + `Insufficient question types: ${short.join("; ")}. Select more chapters or a different pool.`);
  }

  const chapterKeys = new Set(pool.map(q => chapterKey(q.reference)).filter(key => key !== null));
  const maxAppearances = Math.max(2, Math.ceil(20 / Math.max(chapterKeys.size, 1)));
  const limitError = type => new Error(
    "Cannot generate a standard 20-question set from the selected books/chapters. "
    + `Could not select enough ${type} questions while respecting the limit of `
    + `${maxAppearances} questions per chapter. Select more chapters or a different pool.`);

  const usage = new Map([...chapterKeys].map(key => [key, 0]));
  const usedChapters = new Set();
  const usedIndices = new Set();
  let set = [];

  const pick = type => {
    const fits = ([index, q], requireNew) => {
      const key = chapterKey(q.reference);
      return key !== null && !usedIndices.has(index) && (usage.get(key) || 0) < maxAppearances
        && !(requireNew && usedChapters.has(key));
    };
    let candidates = ofType(type).filter(pair => fits(pair, true));
    if (!candidates.length) candidates = ofType(type).filter(pair => fits(pair, false));
    if (!candidates.length) return null;
    const chosen = pickOne(candidates, random);
    const key = chapterKey(chosen[1].reference);
    usedIndices.add(chosen[0]);
    usage.set(key, (usage.get(key) || 0) + 1);
    usedChapters.add(key);
    return chosen;
  };

  for (const type of [firstType, "Quote", "Reference", "Verse", "Context"]) {
    const chosen = pick(type);
    if (!chosen) throw limitError(type);
    set.push(chosen);
  }
  for (let i = 0; i < 4; i++) {
    const chosen = pick("According-To");
    if (!chosen) throw limitError("According-To");
    set.push(chosen);
  }
  while (set.length < 20) {
    const chosen = pick("General");
    if (!chosen) throw limitError("General");
    set.push(chosen);
  }
  set = shuffle(set, random);

  // Every fifth question is swapped for a fresh one of its type where the chapter limit allows.
  for (let round = 0; round < 4; round++) {
    const slots = [];
    for (let i = 4; i < set.length; i += 5) slots.push(i);
    if (!slots.length) continue;
    const slot = pickOne(slots, random);
    const [oldIndex, old] = set[slot];
    const oldKey = chapterKey(old.reference);
    const replacements = ofType(old.type).filter(([index, q]) => {
      if (usedIndices.has(index)) return false;
      const key = chapterKey(q.reference);
      if (key === null) return false;
      let count = set.filter(([, s]) => chapterKey(s.reference) === key).length;
      if (oldKey === key) count = Math.max(0, count - 1);
      return count < maxAppearances;
    });
    if (replacements.length) {
      const chosen = pickOne(replacements, random);
      usedIndices.delete(oldIndex);
      usedIndices.add(chosen[0]);
      set[slot] = chosen;
    }
  }
  return shuffle(set, random).map(([, q]) => q);
}

// The same checks as the server's validate endpoint: what is available, and whether the counts fit.
export function validateCustom(pool, counts, filters) {
  const filtered = filterByBooks(pool, filters);
  const available = availableTypeCounts(filtered);
  let error = null;
  try { generateCustomSet(filtered, counts, 0); } catch (e) { error = e.message; }
  return { valid: error === null, error, available };
}

// `sets` sets for the given mode ("standard" or "custom"); set i uses seed + i, like the server.
export function generateSets(pool, { mode, situation = true, counts, seed = null, sets = 1, books = [] }) {
  const filtered = filterByBooks(pool, books);
  if (!filtered.length) throw new Error("No questions match selected books/chapters");
  return Array.from({ length: sets }, (_, i) => {
    const setSeed = seed === null ? null : seed + i;
    return mode === "custom"
      ? generateCustomSet(filtered, counts, setSeed)
      : generateStandardSet(filtered, situation, setSeed);
  });
}
