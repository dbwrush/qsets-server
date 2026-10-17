export const QUESTION_MS = 20_000;

export function normalizeAnswer(answer) {
  return answer.normalize("NFKC").toLowerCase().replace(/\p{P}/gu, "").replace(/\s+/gu, " ").trim();
}

export function answersMatch(answer, expected) {
  const normalized = normalizeAnswer(answer);
  return normalized.length > 0 && normalized === normalizeAnswer(expected);
}

export function timing(started, submitted) {
  const elapsed = Math.max(0, submitted - started);
  return { elapsed, onTime: elapsed <= QUESTION_MS };
}

export function summarize(results) {
  const correct = results.filter(r => r.correct);
  return {
    answered: results.length,
    correct: correct.length,
    onTime: correct.filter(r => r.onTime).length,
    late: correct.filter(r => !r.onTime).length,
    incorrect: results.filter(r => !r.correct && !r.skipped).length,
    skipped: results.filter(r => r.skipped).length,
    selfCorrected: correct.filter(r => r.selfAssessed).length,
  };
}
