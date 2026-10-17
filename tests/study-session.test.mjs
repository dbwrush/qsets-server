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

test("parenthetical answer text is optional, but required text must match", () => {
  const expected = "John (the man sent from God)";
  for (const answer of ["John", "John, the man sent from God", expected]) {
    assert.equal(answersMatch(answer, expected), true, answer);
  }
  for (const answer of ["", "the man sent from God", "John the man", "John someone else"]) {
    assert.equal(answersMatch(answer, expected), false, answer);
  }
  assert.equal(answersMatch("John preached", "John (the Baptist) preached"), true);
  assert.equal(answersMatch("John the Baptist preached", "John (the Baptist) preached"), true);
  assert.equal(answersMatch("Johnpreached", "John (the Baptist) preached"), false);
  assert.equal(answersMatch("John from God", "John (the Baptist) from God (in Judea)"), true);
  assert.equal(answersMatch("John in Judea", "John (the Baptist) from God (in Judea)"), false);
  assert.equal(answersMatch("John the man", "John (the man (sent from God))"), true);
  assert.equal(answersMatch("John the man sent from God", "John (the man (sent from God))"), true);
  assert.equal(answersMatch("John", "John (the man (sent from God))"), true);
  assert.equal(answersMatch("John the man", "John (the man"), true);
});

test("parenthesized OR alternatives require at least one complete option", () => {
  const expected = "(John) OR (The man sent from God)";
  for (const answer of [
    "John", "The man sent from God", "John, the man sent from God",
    "The man sent from God, John",
  ]) {
    assert.equal(answersMatch(answer, expected), true, answer);
  }
  for (const answer of ["", "!!!", "Peter", "The man", "John Peter", "John John"]) {
    assert.equal(answersMatch(answer, expected), false, answer);
  }
  assert.equal(answersMatch("the man sent from God", "(John) or (The man sent from God)"), true);
  assert.equal(answersMatch("John preached", "(John) OR (The Baptist) preached"), true);
  assert.equal(answersMatch("preached", "(John) OR (The Baptist) preached"), false);
  assert.equal(answersMatch("John James", "(John) OR (James) OR (Peter)"), true);
  assert.equal(answersMatch("Peter", "(John) OR (James) OR (Peter)"), true);
  assert.equal(answersMatch("John", "John or James"), false);
  assert.equal(answersMatch("the Baptist", "(John (the Baptist)) OR (James)"), false);
  assert.equal(answersMatch("John the Baptist", "(John (the Baptist)) OR (James)"), true);
  assert.equal(answersMatch("John", "John ((the Baptist) OR (the man sent from God))"), true);
  assert.equal(answersMatch("John the Baptist", "John ((the Baptist) OR (the man sent from God))"), true);
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
