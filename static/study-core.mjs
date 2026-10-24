// Study question selection that runs on the device, for pools kept in the offline vault.
// Mirrors the server's study_questions handler (generator::list_books, filter_by_books_cow
// and generate_questions) so a session behaves the same with or without a connection.

// "John 3:16" -> { book: "John", chapter: 3 }; null when the reference has no chapter number.
export function parseReference(reference) {
  const space = reference.lastIndexOf(" ");
  if (space === -1) return null;
  const chapter = reference.slice(space + 1).split(":")[0].trim();
  if (!/^[+-]?\d+$/u.test(chapter)) return null;
  return { book: reference.slice(0, space).trim(), chapter: Number(chapter) };
}

// Books in the pool with the first and last chapter that has a question, sorted by name.
export function listBooks(questions) {
  const bounds = new Map();
  for (const question of questions) {
    const parsed = parseReference(question.reference);
    if (!parsed) continue;
    const found = bounds.get(parsed.book);
    if (!found) bounds.set(parsed.book, [parsed.chapter, parsed.chapter]);
    else {
      found[0] = Math.min(found[0], parsed.chapter);
      found[1] = Math.max(found[1], parsed.chapter);
    }
  }
  return [...bounds]
    .map(([name, [min_chapter, max_chapter]]) => ({ name, min_chapter, max_chapter }))
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

// Questions inside the chosen chapter ranges and of the chosen types.
export function filterQuestions(questions, books, types) {
  const ranges = new Map(books.map(book => [
    book.name,
    [Math.min(book.start_chapter, book.end_chapter), Math.max(book.start_chapter, book.end_chapter)],
  ]));
  const wanted = new Set(types.map(type => type.toLowerCase()));
  return questions.filter(question => {
    const parsed = parseReference(question.reference);
    const range = parsed && ranges.get(parsed.book);
    return range && parsed.chapter >= range[0] && parsed.chapter <= range[1]
      && wanted.has(question.type.toLowerCase());
  });
}

export function shuffle(items, random = Math.random) {
  const result = [...items];
  for (let i = result.length - 1; i > 0; i--) {
    const j = Math.floor(random() * (i + 1));
    [result[i], result[j]] = [result[j], result[i]];
  }
  return result;
}

// A shuffled session of `count` questions (0 = all). Throws the same messages the server returns.
export function selectQuestions(questions, { books, types, count }, random = Math.random) {
  if (!books.length || !types.length) throw new Error("Select passages and at least one question type");
  const matching = filterQuestions(questions, books, types);
  if (!matching.length) throw new Error("No questions match the selected passages and types");
  const size = count === 0 ? matching.length : Math.min(count, matching.length);
  return shuffle(matching, random).slice(0, size);
}
