import { test } from "node:test";
import assert from "node:assert/strict";
import { parseReference, listBooks, filterQuestions, selectQuestions } from "../static/study-core.mjs";

const q = (reference, type = "General") => ({ question: `Q ${reference}`, type, reference, answer: "A" });
const POOL = [
  q("John 1:1"), q("John 1:2"), q("John 3:16", "Quote"), q("John 5:1"),
  q("1 John 2:1"), q("Song of Songs 4:3", "Verse"), q("Nowhere"),
];

test("references split into book and chapter like the server does", () => {
  assert.deepEqual(parseReference("John 3:16"), { book: "John", chapter: 3 });
  assert.deepEqual(parseReference("1 John 2:1"), { book: "1 John", chapter: 2 });
  assert.deepEqual(parseReference("Song of Songs 4"), { book: "Song of Songs", chapter: 4 });
  assert.equal(parseReference("Nowhere"), null);
  assert.equal(parseReference("John abc:1"), null);
});

test("books list the chapter span with questions, sorted by name", () => {
  assert.deepEqual(listBooks(POOL), [
    { name: "1 John", min_chapter: 2, max_chapter: 2 },
    { name: "John", min_chapter: 1, max_chapter: 5 },
    { name: "Song of Songs", min_chapter: 4, max_chapter: 4 },
  ]);
});

test("filtering honors chapter ranges (either order) and type case", () => {
  const books = [{ name: "John", start_chapter: 3, end_chapter: 1 }];
  assert.deepEqual(
    filterQuestions(POOL, books, ["general", "QUOTE"]).map(x => x.reference),
    ["John 1:1", "John 1:2", "John 3:16"],
  );
  assert.deepEqual(filterQuestions(POOL, books, ["Verse"]), []);
});

test("selection shuffles, caps at the count, and 0 means everything", () => {
  const request = { books: [{ name: "John", start_chapter: 1, end_chapter: 5 }], types: ["General", "Quote"] };
  const all = selectQuestions(POOL, { ...request, count: 0 });
  assert.equal(all.length, 4);
  assert.equal(selectQuestions(POOL, { ...request, count: 2 }).length, 2);
  assert.equal(selectQuestions(POOL, { ...request, count: 99 }).length, 4);
  const reversed = selectQuestions(POOL, { ...request, count: 0 }, () => 0);
  assert.notDeepEqual(reversed.map(x => x.reference), POOL.slice(0, 4).map(x => x.reference));
});

test("selection reports the same problems the server does", () => {
  const books = [{ name: "John", start_chapter: 1, end_chapter: 1 }];
  assert.throws(() => selectQuestions(POOL, { books: [], types: ["General"], count: 5 }), /Select passages/);
  assert.throws(() => selectQuestions(POOL, { books, types: [], count: 5 }), /Select passages/);
  assert.throws(() => selectQuestions(POOL, { books, types: ["Verse"], count: 5 }), /No questions match/);
});
