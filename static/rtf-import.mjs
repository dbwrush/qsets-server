// Reads question sets from the text of a user's own RTF file (see rtf-text.mjs) and turns them
// into pool records: { question, type, reference, answer }. Runs in the browser only.
//
// Layout understood: a heading line ("Set #3 ..."), then for each question a line
// "<letter><tab><number>. question text" followed by "A. answer (Book 1:2)". Blank lines separate
// questions; text that continues onto further lines is joined to the field it belongs to.

export const TYPE_BY_LETTER = {
  A: "According-To", G: "General", I: "In-What-Book-and-Chapter", Q: "Quote",
  R: "Reference", S: "Situation", X: "Context", V: "Verse",
};

const HEADING = /^\s*(?:tie[\s-]*breaker\s+)?(?:replacement\s+)?set\s*#?\s*\w+/iu;
const QUESTION = /^\s*([A-Za-z])[\t ]+(\d+)\s*[.)]\s*(.*)$/u;
const ANSWER = /^\s*A\s*[.:]\s*(.*)$/u;
const TRAILING_REFERENCE = /\s*\(([^()]*\d[^()]*)\)\s*$/u;

const clean = text => text.replace(/\s+/gu, " ").trim();

// "[OR Jesus]" is how these files mark alternatives; the app writes them as "(OR Jesus)".
export function convertAlternatives(answer) {
  return answer.replace(/\[\s*(OR\b[^\[\]]*)\]/giu, (_, inner) => `(${clean(inner)})`);
}

// Splits "The Word [OR Jesus] (John 1:1)" into the answer and its reference.
export function splitAnswer(text) {
  const match = TRAILING_REFERENCE.exec(text);
  if (!match) return null;
  // "John 4:5, 8" -> "John 4:5,8": the chapter is read from after the last space.
  const reference = clean(match[1]).replace(/\s*([,–—-])\s*(?=\d)/gu, "$1");
  const answer = convertAlternatives(clean(text.slice(0, match.index)));
  return answer && reference ? { answer, reference } : null;
}

// Returns { questions, rounds, warnings } for the text of one file.
export function parseQuestionSets(text) {
  const questions = [];
  const warnings = [];
  const rounds = [];
  let round = "";
  let current = null; // { letter, number, question, answer, stage: "question" | "answer", round }

  const finish = () => {
    if (!current) return;
    const { letter, number, round: setName } = current;
    const type = TYPE_BY_LETTER[letter.toUpperCase()];
    const split = splitAnswer(current.answer);
    if (!type) warnings.push(`Question ${number}${setName ? ` (${setName})` : ""}: unknown type letter "${letter}"`);
    else if (!clean(current.question)) warnings.push(`Question ${number}${setName ? ` (${setName})` : ""}: no question text`);
    else if (!split) warnings.push(`Question ${number}${setName ? ` (${setName})` : ""}: no answer with a reference in parentheses`);
    else questions.push({ question: clean(current.question), type, reference: split.reference, answer: split.answer, round: setName });
    current = null;
  };

  for (const rawLine of String(text).split("\n")) {
    const line = rawLine.replace(/\s+$/u, "");
    if (!line.trim()) continue;
    const question = QUESTION.exec(line);
    if (question && !ANSWER.test(line.trim())) {
      finish();
      current = { letter: question[1], number: question[2], question: question[3], answer: "", stage: "question", round };
      continue;
    }
    // A heading only counts between questions, not inside a question that is still being read.
    if (HEADING.test(line) && (!current || (current.stage === "answer" && TRAILING_REFERENCE.test(current.answer)))) {
      finish();
      round = clean(line);
      rounds.push(round);
      continue;
    }
    if (!current) continue;
    const answer = current.stage === "question" ? ANSWER.exec(line) : null;
    if (answer) {
      current.answer = answer[1];
      current.stage = "answer";
    } else if (current.stage === "question") {
      current.question += ` ${line.trim()}`;
    } else if (!TRAILING_REFERENCE.test(current.answer)) {
      current.answer += ` ${line.trim()}`;
    }
  }
  finish();
  return { questions, rounds, warnings };
}

// Parts of a reference such as "John 3:16-18" -> "John 3", used to list memory passages.
export function chapterOf(reference) {
  const space = reference.lastIndexOf(" ");
  if (space === -1) return null;
  const chapter = reference.slice(space + 1).split(":")[0].trim();
  return /^\d+$/u.test(chapter) ? `${reference.slice(0, space).trim()} ${chapter}` : null;
}

// Removes repeats (the same question, answer and reference in several sets or files).
export function dedupeQuestions(questions) {
  const seen = new Set();
  const result = [];
  for (const q of questions) {
    const key = [q.question, q.answer, q.reference, q.type].join("\u0000").toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    result.push({ question: q.question, type: q.type, reference: q.reference, answer: q.answer });
  }
  return result;
}
