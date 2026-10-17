import { test } from "node:test";
import assert from "node:assert/strict";
import { answersMatch, timing, summarize } from "../static/study-session.mjs";

test("answers normalize punctuation, case, and whitespace, but do not guess", () => {
  assert.equal(answersMatch('  THE   word! ', "The Word."), true);
  assert.equal(answersMatch("God", "The Word"), false);
  assert.equal(answersMatch("!!!", "..."), false);
  assert.equal(answersMatch("John 1:1", "John 1:2"), false);
  assert.equal(answersMatch("JESUS’ words", "Jesus' words"), true);
});

test("submission timing has an exact 20-second boundary", () => {
  assert.deepEqual(timing(100, 20_099), { elapsed: 19_999, onTime: true });
  assert.deepEqual(timing(100, 20_100), { elapsed: 20_000, onTime: true });
  assert.deepEqual(timing(100, 20_101), { elapsed: 20_001, onTime: false });
  assert.equal(timing(0, 120_000).onTime, false);
});

test("feedback separates accuracy, timing, skips, and self-assessment", () => {
  assert.deepEqual(summarize([
    { correct: true, onTime: true },
    { correct: true, onTime: false, selfAssessed: true },
    { correct: false, onTime: true },
    { correct: false, onTime: false, skipped: true },
  ]), {
    answered: 4, correct: 2, onTime: 1, late: 1,
    incorrect: 1, skipped: 1, selfCorrected: 1,
  });
  assert.equal(summarize([]).answered, 0);
});
