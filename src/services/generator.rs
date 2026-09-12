use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

use csv::StringRecord;
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct QuestionRecord {
    pub question: String,
    #[serde(rename = "type")]
    pub qtype: String,
    pub reference: String,
    pub answer: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GenerateRequest {
    pub question_type: String,
    pub count: usize,
    pub situation: Option<bool>,
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BookFilter {
    pub name: String,
    pub start_chapter: i32,
    pub end_chapter: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BookMeta {
    pub name: String,
    pub min_chapter: i32,
    pub max_chapter: i32,
}

fn parse_book_and_chapter(reference: &str) -> Option<(String, i32)> {
    let last_space = reference.rfind(' ')?;
    let book = reference[..last_space].trim().to_string();
    let chapter_part = reference[last_space + 1..].split(':').next()?.trim();
    let chapter = chapter_part.parse::<i32>().ok()?;
    Some((book, chapter))
}

fn chapter_key(reference: &str) -> Option<String> {
    let (book, chapter) = parse_book_and_chapter(reference)?;
    Some(format!("{book} {chapter}"))
}

pub fn list_books(pool: &[QuestionRecord]) -> Vec<BookMeta> {
    let mut bounds: HashMap<String, (i32, i32)> = HashMap::new();
    for q in pool {
        let Some((book, chapter)) = parse_book_and_chapter(&q.reference) else {
            continue;
        };
        bounds
            .entry(book)
            .and_modify(|(min_ch, max_ch)| {
                if chapter < *min_ch {
                    *min_ch = chapter;
                }
                if chapter > *max_ch {
                    *max_ch = chapter;
                }
            })
            .or_insert((chapter, chapter));
    }

    let mut books: Vec<BookMeta> = bounds
        .into_iter()
        .map(|(name, (min_chapter, max_chapter))| BookMeta {
            name,
            min_chapter,
            max_chapter,
        })
        .collect();

    books.sort_by(|a, b| a.name.cmp(&b.name));
    books
}

#[allow(dead_code)]
pub fn filter_by_books(pool: &[QuestionRecord], filters: &[BookFilter]) -> Vec<QuestionRecord> {
    filter_by_books_cow(pool, filters).into_owned()
}

pub fn filter_by_books_cow<'a>(
    pool: &'a [QuestionRecord],
    filters: &[BookFilter],
) -> Cow<'a, [QuestionRecord]> {
    if filters.is_empty() {
        return Cow::Borrowed(pool);
    }

    let mut by_book: HashMap<&str, (i32, i32)> = HashMap::new();
    for filter in filters {
        let start = filter.start_chapter.min(filter.end_chapter);
        let end = filter.start_chapter.max(filter.end_chapter);
        by_book.insert(filter.name.as_str(), (start, end));
    }

    let all_questions_match = pool.iter().all(|q| {
        let Some((book, chapter)) = parse_book_and_chapter(&q.reference) else {
            return false;
        };
        let Some((start, end)) = by_book.get(book.as_str()) else {
            return false;
        };
        chapter >= *start && chapter <= *end
    });
    if all_questions_match {
        return Cow::Borrowed(pool);
    }

    let filtered: Vec<QuestionRecord> = pool
        .iter()
        .filter(|q| {
            let Some((book, chapter)) = parse_book_and_chapter(&q.reference) else {
                return false;
            };
            let Some((start, end)) = by_book.get(book.as_str()) else {
                return false;
            };
            chapter >= *start && chapter <= *end
        })
        .cloned()
        .collect();

    if filtered.len() == pool.len() {
        Cow::Borrowed(pool)
    } else {
        Cow::Owned(filtered)
    }
}

const VALID_TYPES: &[&str] = &[
    "Situation",
    "In-What-Book-and-Chapter",
    "Quote",
    "Reference",
    "Verse",
    "Context",
    "According-To",
    "General",
];

const REQUIRED_TYPES_FOR_STANDARD: &[&str] = &[
    "Situation",
    "Quote",
    "Reference",
    "Verse",
    "Context",
    "According-To",
    "General",
];

#[derive(Debug, Clone, Serialize)]
pub struct CsvValidation {
    pub valid_count: usize,
    pub skipped_count: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn required_column_indexes(headers: &StringRecord) -> Result<[usize; 4], Vec<String>> {
    let normalized: Vec<String> = headers
        .iter()
        .map(|s| s.trim().trim_start_matches('\u{feff}').to_lowercase())
        .collect();
    let names = ["question", "type", "reference", "answer"];
    let mut missing = Vec::new();
    let mut indexes = [0; 4];

    for (slot, name) in names.iter().enumerate() {
        match normalized.iter().position(|column| column == name) {
            Some(index) => indexes[slot] = index,
            None => missing.push(format!("Missing required column: {name}")),
        }
    }

    if missing.is_empty() {
        Ok(indexes)
    } else {
        Err(missing)
    }
}

pub fn validate_csv(csv_text: &str) -> CsvValidation {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if csv_text.trim().is_empty() {
        return CsvValidation {
            valid_count: 0,
            skipped_count: 0,
            errors: vec!["CSV is empty".to_string()],
            warnings: vec![],
        };
    }

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(csv_text.as_bytes());

    let indexes = match rdr.headers() {
        Ok(headers) => match required_column_indexes(headers) {
            Ok(indexes) => indexes,
            Err(missing) => {
                errors.extend(missing);
                return CsvValidation {
                    valid_count: 0,
                    skipped_count: 0,
                    errors,
                    warnings,
                };
            }
        },
        Err(error) => {
            return CsvValidation {
                valid_count: 0,
                skipped_count: 0,
                errors: vec![format!("Could not read CSV headers: {error}")],
                warnings,
            };
        }
    };

    if !errors.is_empty() {
        return CsvValidation {
            valid_count: 0,
            skipped_count: 0,
            errors,
            warnings,
        };
    }

    let mut valid = 0usize;
    let mut skipped = 0usize;
    let mut seen_types: HashSet<String> = HashSet::new();
    let mut unknown_types: HashSet<String> = HashSet::new();

    for (row_num, rec) in rdr.records().enumerate() {
        let Ok(rec) = rec else {
            skipped += 1;
            warnings.push(format!("Row {}: could not parse CSV record", row_num + 2));
            continue;
        };

        let question = rec.get(indexes[0]).unwrap_or_default().trim().to_string();
        let qtype = rec.get(indexes[1]).unwrap_or_default().trim().to_string();
        let reference = rec.get(indexes[2]).unwrap_or_default().trim().to_string();
        let answer = rec.get(indexes[3]).unwrap_or_default().trim().to_string();

        if question.starts_with("LICENSE") {
            skipped += 1;
            continue;
        }

        if question.is_empty() || qtype.is_empty() || reference.is_empty() || answer.is_empty() {
            skipped += 1;
            continue;
        }

        if parse_book_and_chapter(&reference).is_none() {
            skipped += 1;
            warnings.push(format!(
                "Row {}: unparseable reference \"{}\"",
                row_num + 2,
                reference
            ));
            continue;
        }

        if !VALID_TYPES.iter().any(|t| qtype.eq_ignore_ascii_case(t)) {
            unknown_types.insert(qtype.clone());
        }

        seen_types.insert(qtype);
        valid += 1;
    }

    for ut in &unknown_types {
        warnings.push(format!(
            "Unknown question type \"{ut}\" — expected one of: {}",
            VALID_TYPES.join(", ")
        ));
    }

    let missing: Vec<&str> = REQUIRED_TYPES_FOR_STANDARD
        .iter()
        .filter(|t| !seen_types.iter().any(|s| s.eq_ignore_ascii_case(t)))
        .copied()
        .collect();

    if !missing.is_empty() {
        warnings.push(format!(
            "Pool is missing required types for standard set generation: {}",
            missing.join(", ")
        ));
    }

    if valid == 0 {
        errors.push("No valid questions found after parsing".to_string());
    }

    CsvValidation {
        valid_count: valid,
        skipped_count: skipped,
        errors,
        warnings,
    }
}

pub fn parse_csv(csv_text: &str) -> Vec<QuestionRecord> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(csv_text.as_bytes());

    let indexes = match rdr
        .headers()
        .ok()
        .and_then(|headers| required_column_indexes(headers).ok())
    {
        Some(indexes) => indexes,
        None => return Vec::new(),
    };

    let mut out = Vec::new();
    for rec in rdr.records().flatten() {
        let question = rec.get(indexes[0]).unwrap_or_default().trim().to_string();
        let qtype = rec.get(indexes[1]).unwrap_or_default().trim().to_string();
        let reference = rec.get(indexes[2]).unwrap_or_default().trim().to_string();
        let answer = rec.get(indexes[3]).unwrap_or_default().trim().to_string();

        if question.starts_with("LICENSE") {
            continue;
        }

        if question.is_empty() || qtype.is_empty() || reference.is_empty() || answer.is_empty() {
            continue;
        }

        if parse_book_and_chapter(&reference).is_none() {
            continue;
        }

        out.push(QuestionRecord {
            question,
            qtype,
            reference,
            answer,
        });
    }
    out
}

pub fn generate_questions(pool: &[QuestionRecord], req: &GenerateRequest) -> Vec<QuestionRecord> {
    if req.question_type.eq_ignore_ascii_case("standard") {
        return generate_standard_set(pool, req.situation.unwrap_or(true), req.seed);
    }

    let mut filtered: Vec<QuestionRecord> = pool
        .iter()
        .filter(|q| req.question_type == "all" || q.qtype.eq_ignore_ascii_case(&req.question_type))
        .cloned()
        .collect();

    if filtered.is_empty() {
        return vec![];
    }

    let mut rng: StdRng = match req.seed {
        Some(seed) => SeedableRng::seed_from_u64(seed),
        None => SeedableRng::from_entropy(),
    };

    filtered.shuffle(&mut rng);
    filtered.truncate(req.count.min(filtered.len()));
    filtered
}

pub fn generate_standard_set(
    pool: &[QuestionRecord],
    situation: bool,
    seed: Option<u64>,
) -> Vec<QuestionRecord> {
    let mut rng: StdRng = match seed {
        Some(v) => SeedableRng::seed_from_u64(v),
        None => SeedableRng::from_entropy(),
    };

    let source: Vec<(usize, &QuestionRecord)> = pool.iter().enumerate().collect();
    let mut by_type: HashMap<String, Vec<(usize, &QuestionRecord)>> = HashMap::new();
    for pair in &source {
        by_type
            .entry(pair.1.qtype.to_ascii_lowercase())
            .or_default()
            .push(*pair);
    }

    let chapter_keys: HashSet<String> = source
        .iter()
        .filter_map(|(_, q)| chapter_key(&q.reference))
        .collect();

    let chapter_count = chapter_keys.len().max(1);
    let max_appearances = usize::max(2, 20_usize.div_ceil(chapter_count));

    let mut chapter_usage: HashMap<String, usize> =
        chapter_keys.iter().map(|k| (k.clone(), 0usize)).collect();
    let mut used_chapters = HashSet::new();
    let mut used_indices = HashSet::new();
    let mut set: Vec<(usize, QuestionRecord)> = Vec::new();

    let pick = |wanted_type: &str,
                prefer_new_chapter: bool,
                by_type_ref: &HashMap<String, Vec<(usize, &QuestionRecord)>>,
                used_idx: &mut HashSet<usize>,
                ch_usage: &mut HashMap<String, usize>,
                used_ch: &mut HashSet<String>,
                rng_ref: &mut StdRng|
     -> Option<(usize, QuestionRecord)> {
        let source_ref = by_type_ref
            .get(&wanted_type.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let matches = |pair: &&(usize, &QuestionRecord), require_new: bool| {
            let (idx, q) = pair;
            if !q.qtype.eq_ignore_ascii_case(wanted_type) || used_idx.contains(idx) {
                return false;
            }
            let Some(key) = chapter_key(&q.reference) else {
                return false;
            };
            let under_cap = ch_usage.get(&key).copied().unwrap_or(0) < max_appearances;
            if !under_cap {
                return false;
            }
            if require_new && used_ch.contains(&key) {
                return false;
            }
            true
        };

        let mut candidates: Vec<&(usize, &QuestionRecord)> = source_ref
            .iter()
            .filter(|p| matches(p, prefer_new_chapter))
            .collect();
        if candidates.is_empty() {
            candidates = source_ref.iter().filter(|p| matches(p, false)).collect();
        }

        let chosen = candidates.choose(rng_ref)?;
        let (idx, question) = **chosen;
        let key = chapter_key(&question.reference)?;
        used_idx.insert(idx);
        *ch_usage.entry(key.clone()).or_insert(0) += 1;
        used_ch.insert(key);
        Some((idx, question.clone()))
    };

    let first_type = if situation {
        "Situation"
    } else {
        "In-What-Book-and-Chapter"
    };
    if let Some(q) = pick(
        first_type,
        true,
        &by_type,
        &mut used_indices,
        &mut chapter_usage,
        &mut used_chapters,
        &mut rng,
    ) {
        set.push(q);
    } else {
        return Vec::new();
    }

    for t in ["Quote", "Reference", "Verse", "Context"] {
        if let Some(q) = pick(
            t,
            true,
            &by_type,
            &mut used_indices,
            &mut chapter_usage,
            &mut used_chapters,
            &mut rng,
        ) {
            set.push(q);
        } else {
            return Vec::new();
        }
    }

    for _ in 0..4 {
        if let Some(q) = pick(
            "According-To",
            true,
            &by_type,
            &mut used_indices,
            &mut chapter_usage,
            &mut used_chapters,
            &mut rng,
        ) {
            set.push(q);
        } else {
            return Vec::new();
        }
    }

    while set.len() < 20 {
        if let Some(q) = pick(
            "General",
            true,
            &by_type,
            &mut used_indices,
            &mut chapter_usage,
            &mut used_chapters,
            &mut rng,
        ) {
            set.push(q);
        } else {
            return Vec::new();
        }
    }

    set.shuffle(&mut rng);

    for _ in 0..4 {
        let fifth_indices: Vec<usize> = (4..set.len()).step_by(5).collect();
        if fifth_indices.is_empty() {
            continue;
        }
        let slot = fifth_indices[rng.gen_range(0..fifth_indices.len())];
        let old = &set[slot].1;
        let wanted_type = old.qtype.clone();
        let old_key = chapter_key(&old.reference);

        let replacement_source = by_type
            .get(&wanted_type.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let replacements: Vec<&(usize, &QuestionRecord)> = replacement_source
            .iter()
            .filter(|(idx, q)| {
                if used_indices.contains(idx) {
                    return false;
                }
                if !q.qtype.eq_ignore_ascii_case(&wanted_type) {
                    return false;
                }
                let Some(key) = chapter_key(&q.reference) else {
                    return false;
                };
                let mut count = set
                    .iter()
                    .filter_map(|(_, s)| chapter_key(&s.reference))
                    .filter(|k| *k == key)
                    .count();
                if let Some(ok) = &old_key {
                    if ok == &key {
                        count = count.saturating_sub(1);
                    }
                }
                count < max_appearances
            })
            .collect();

        if let Some(candidate) = replacements.choose(&mut rng) {
            let (new_idx, new_q) = **candidate;
            let old_idx = set[slot].0;
            used_indices.remove(&old_idx);
            used_indices.insert(new_idx);
            set[slot] = (new_idx, new_q.clone());
        }
    }

    set.shuffle(&mut rng);
    set.into_iter().map(|(_, q)| q).collect()
}
