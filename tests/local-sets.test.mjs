import { test } from "node:test";
import assert from "node:assert/strict";
import { rtfToText } from "../static/rtf-text.mjs";
import { parseQuestionSets, splitAnswer, dedupeQuestions } from "../static/rtf-import.mjs";
import {
  generateStandardSet, generateCustomSet, generateSets, validateCustom, availableTypeCounts, makeRandom,
} from "../static/generator-core.mjs";
import { createStore, memoryBackend } from "../static/local-store.mjs";

// A small made-up question file in the plain style: one paragraph per line.
const SIMPLE = String.raw`{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fswiss Arial;}}{\colortbl;\red0\green0\blue0;}
\viewkind4\uc1\pard\f0\fs22 Set #1 Sample\par\par
G\tab 1. Who was ready to go@u8217?s way?\par
\tab A. The man [OR the guide] (Test 1:1)\par\par
V\tab 2. Finish this verse: \'93Say it\'94\par
\tab A. plainly (Test 1:2)\par\par
Tie Breaker Set #1 Extra\par
Q\tab 1. Quote one\par
\tab A. quoted (Test 2:3)\par
}`;

// The same idea as Word writes it: styles, a header, field codes and runs split across groups.
const WORD = String.raw`{\rtf1\ansi\ansicpg1252\uc1{\fonttbl{\f0\fnil Calibri;}}{\stylesheet{\s0 Normal;}}
{\header{\pard header text\par}}
{\pard\plain\f0 Set #2 Word\par}
{\pard\plain X{\tab}1. Context question\line runs on\par}
{\pard\plain{\tab}A. {\b bold} answer {\*\generator hidden}(Test 3:4)\par}
{\pard\plain Z\tab 2. Unknown type\par}
{\pard\plain\tab A. nope (Test 3:5)\par}
}`;

const SIMPLE_RTF = SIMPLE.replaceAll("@u", "\\u");

test("RTF text keeps paragraphs, tabs, escapes and drops everything else", () => {
  const text = rtfToText(SIMPLE_RTF);
  assert.match(text, /G\t1\. Who was ready to go’s way\?/u);
  assert.match(text, /Finish this verse: “Say it”/u);
  assert.doesNotMatch(text, /Arial|colortbl|redgreen/u);
  const word = rtfToText(WORD);
  assert.doesNotMatch(word, /header text|hidden|Calibri/u);
  assert.match(word, /A\. bold answer \(Test 3:4\)/u);
});

test("answers split off their reference and convert alternatives", () => {
  assert.deepEqual(splitAnswer("The Word [OR Jesus] (Jesus) (John 1:1)"), {
    answer: "The Word (OR Jesus) (Jesus)", reference: "John 1:1",
  });
  assert.equal(splitAnswer("Both (John 4:5, 8)").reference, "John 4:5,8");
  assert.equal(splitAnswer("Range (John 4:5 – 8)").reference, "John 4:5–8");
  assert.equal(splitAnswer("no reference here"), null);
});

test("question files parse into typed records, with problems reported", () => {
  const simple = parseQuestionSets(rtfToText(SIMPLE_RTF));
  assert.deepEqual(simple.rounds, ["Set #1 Sample", "Tie Breaker Set #1 Extra"]);
  assert.deepEqual(simple.questions.map(q => [q.type, q.reference, q.answer]), [
    ["General", "Test 1:1", "The man (OR the guide)"],
    ["Verse", "Test 1:2", "plainly"],
    ["Quote", "Test 2:3", "quoted"],
  ]);
  const word = parseQuestionSets(rtfToText(WORD));
  assert.equal(word.questions.length, 1);
  assert.equal(word.questions[0].question, "Context question runs on");
  assert.equal(word.questions[0].type, "Context");
  assert.equal(word.warnings.length, 1);
  assert.match(word.warnings[0], /unknown type letter "Z"/u);
});

test("repeated questions are counted once", () => {
  const q = { question: "Q", type: "General", reference: "Test 1:1", answer: "A" };
  assert.equal(dedupeQuestions([q, { ...q }, { ...q, answer: "B" }]).length, 2);
});

// A pool big enough for standard and custom sets, spread over several chapters.
function pool(chapters = 8) {
  const make = (type, count) => Array.from({ length: count }, (_, i) => ({
    question: `${type} ${i}`, type, reference: `Test ${(i % chapters) + 1}:${i + 1}`, answer: `a${i}`,
  }));
  return [
    ...make("Situation", 4), ...make("In-What-Book-and-Chapter", 4), ...make("Quote", 4),
    ...make("Reference", 4), ...make("Verse", 4), ...make("Context", 6),
    ...make("According-To", 12), ...make("General", 40),
  ];
}

const countBy = (items, key) => items.reduce((acc, item) => ({ ...acc, [key(item)]: (acc[key(item)] || 0) + 1 }), {});
const chapterOf = q => q.reference.split(":")[0];

test("standard sets have the usual shape and respect the chapter limit", () => {
  for (const situation of [true, false]) {
    const set = generateStandardSet(pool(), situation, 7);
    assert.equal(set.length, 20);
    assert.equal(new Set(set.map(q => q.question)).size, 20);
    const types = countBy(set, q => q.type);
    assert.equal(types[situation ? "Situation" : "In-What-Book-and-Chapter"], 1);
    assert.equal(types["According-To"], 4);
    assert.equal(types.General, 11);
    assert.ok(Math.max(...Object.values(countBy(set, chapterOf))) <= Math.max(2, Math.ceil(20 / 8)));
  }
});

test("a seed repeats a set and seeds differ between sets", () => {
  assert.deepEqual(generateStandardSet(pool(), true, 3), generateStandardSet(pool(), true, 3));
  const sets = generateSets(pool(), { mode: "standard", seed: 5, sets: 3 });
  assert.equal(sets.length, 3);
  assert.deepEqual(sets[1], generateStandardSet(pool(), true, 6));
  assert.equal(makeRandom(1)(), makeRandom(1)());
});

test("custom sets meet the requested counts or explain the shortage", () => {
  const counts = { General: 5, "According-To": 3, Quote: 2 };
  const set = generateCustomSet(pool(), counts, 1);
  assert.deepEqual(countBy(set, q => q.type), counts);
  assert.throws(() => generateCustomSet(pool(), { Quote: 9 }), /Quote \(need 9, available 4\)/u);
  assert.throws(() => generateCustomSet(pool(), {}), /at least one question/u);
  assert.throws(() => generateStandardSet(pool().filter(q => q.type !== "Verse")), /Verse \(need 1, available 0\)/u);
});

test("validation reports availability, chapter limits and book filters", () => {
  const books = [{ name: "Test", start_chapter: 1, end_chapter: 2 }];
  const ok = validateCustom(pool(), { General: 4 }, books);
  assert.equal(ok.valid, true);
  assert.equal(ok.available.General, 10);
  const tooMany = validateCustom(pool(), { General: 30 }, books);
  assert.equal(tooMany.valid, false);
  assert.match(tooMany.error, /per chapter|Insufficient/u);
  assert.equal(availableTypeCounts(pool()).Verse, 4);
  assert.throws(() => generateSets(pool(), { mode: "standard", books: [{ name: "Nope", start_chapter: 1, end_chapter: 2 }] }), /No questions match/u);
});

test("datasets keep files, combine them without repeats and can be renamed or deleted", async () => {
  const store = createStore(memoryBackend());
  const set = await store.create("  My   set ");
  assert.equal(set.name, "My set");
  const a = { question: "Q1", type: "General", reference: "Test 1:1", answer: "A" };
  const b = { question: "Q2", type: "Quote", reference: "Test 2:1", answer: "B" };
  await store.addFile(set.id, "one.rtf", [a, b]);
  await store.addFile(set.id, "two.rtf", [a]);
  const combined = await store.pool(set.id);
  assert.equal(combined.questions.length, 2);
  assert.deepEqual(combined.books, [{ name: "Test", min_chapter: 1, max_chapter: 2 }]);
  assert.equal(combined.type_counts.Quote, 1);
  await store.addFile(set.id, "one.rtf", [a]);
  assert.equal((await store.files(set.id)).find(file => file.name === "one.rtf").count, 1);
  await store.removeFile(set.id, "two.rtf");
  assert.equal((await store.list())[0].questions, 1);
  await store.rename(set.id, "Renamed");
  assert.equal((await store.list())[0].name, "Renamed");
  await assert.rejects(store.rename(set.id, "  "), /Give the set a name/u);
  await store.remove(set.id);
  assert.deepEqual(await store.list(), []);
  await assert.rejects(store.pool(set.id), /no longer on this device/u);
});
