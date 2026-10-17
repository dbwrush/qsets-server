export const QUESTION_MS = 20_000;

function normalizeText(answer) {
  return answer.normalize("NFKC").toLowerCase().replace(/\p{P}/gu, "").replace(/\s+/gu, " ");
}

export function normalizeAnswer(answer) {
  return normalizeText(answer).trim();
}

function closingParenthesis(text, start) {
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    if (text[i] === "(") depth++;
    if (text[i] === ")" && --depth === 0) return i;
  }
  return -1;
}

export function answersMatch(answer, expected) {
  const normalized = normalizeAnswer(answer);
  if (!normalized) return false;

  function append(prefixes, text) {
    return new Set([...prefixes]
      .map(prefix => normalizeText(prefix + text).trimStart())
      .filter(prefix => normalized.startsWith(prefix.trimEnd())));
  }

  function matchParts(text, prefixes) {
    let offset = 0;
    while (offset < text.length) {
      const start = text.indexOf("(", offset);
      if (start === -1) break;
      let end = closingParenthesis(text, start);
      if (end === -1) break;
      prefixes = append(prefixes, text.slice(offset, start));
      const values = [text.slice(start + 1, end)];
      for (;;) {
        const separator = /^\s+OR\s+\(/iu.exec(text.slice(end + 1));
        if (!separator) break;
        const nextStart = end + separator[0].length;
        const nextEnd = closingParenthesis(text, nextStart);
        if (nextEnd === -1) break;
        values.push(text.slice(nextStart + 1, nextEnd));
        end = nextEnd;
      }
      if (values.length === 1) {
        prefixes = new Set([...prefixes, ...matchParts(values[0], prefixes)]);
      } else {
        const matches = new Set();
        function matchOptions(remaining, starts) {
          for (const [index, value] of remaining.entries()) {
            const ends = matchParts(value, starts);
            for (const end of ends) matches.add(end);
            if (ends.size && remaining.length > 1) {
              matchOptions(remaining.filter((_, i) => i !== index), append(ends, " "));
            }
          }
        }
        matchOptions(values, prefixes);
        prefixes = matches;
      }
      offset = end + 1;
      if (!prefixes.size) return prefixes;
    }
    return append(prefixes, text.slice(offset));
  }

  return [...matchParts(expected, new Set([""]))]
    .some(candidate => candidate.trimEnd() === normalized);
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
