use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CustomCounts {
    #[serde(rename = "Situation")]
    pub situation: usize,
    #[serde(rename = "In-What-Book-and-Chapter")]
    pub in_what_book_and_chapter: usize,
    #[serde(rename = "Quote")]
    pub quote: usize,
    #[serde(rename = "Reference")]
    pub reference: usize,
    #[serde(rename = "Verse")]
    pub verse: usize,
    #[serde(rename = "Context")]
    pub context: usize,
    #[serde(rename = "According-To")]
    pub according_to: usize,
    #[serde(rename = "General")]
    pub general: usize,
}

impl CustomCounts {
    fn requirements(&self) -> [(&'static str, usize); 8] {
        [
            ("Situation", self.situation),
            ("In-What-Book-and-Chapter", self.in_what_book_and_chapter),
            ("Quote", self.quote),
            ("Reference", self.reference),
            ("Verse", self.verse),
            ("Context", self.context),
            ("According-To", self.according_to),
            ("General", self.general),
        ]
    }
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

pub fn available_type_counts(pool: &[QuestionRecord]) -> HashMap<&'static str, usize> {
    let mut counts: HashMap<_, _> = VALID_TYPES.iter().map(|qtype| (*qtype, 0)).collect();
    for q in pool {
        if let Some(qtype) = VALID_TYPES
            .iter()
            .find(|qtype| q.qtype.eq_ignore_ascii_case(qtype))
        {
            *counts.get_mut(qtype).unwrap() += 1;
        }
    }
    counts
}

pub fn questions_to_csv(questions: &[QuestionRecord]) -> Result<String, String> {
    if questions.is_empty() {
        return Err("Add at least one question".to_string());
    }
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer
        .write_record(["Question", "Type", "Reference", "Answer"])
        .map_err(|error| format!("Could not write CSV headers: {error}"))?;
    for (index, q) in questions.iter().enumerate() {
        let row = index + 1;
        if [&q.question, &q.qtype, &q.reference, &q.answer]
            .iter()
            .any(|v| v.trim().is_empty())
        {
            return Err(format!("Question {row}: fill in all four fields"));
        }
        if q.question.trim().starts_with("LICENSE") {
            return Err(format!(
                "Question {row}: question text cannot start with LICENSE"
            ));
        }
        if !VALID_TYPES
            .iter()
            .any(|t| q.qtype.trim().eq_ignore_ascii_case(t))
        {
            return Err(format!("Question {row}: unknown question type"));
        }
        if parse_book_and_chapter(q.reference.trim()).is_none() {
            return Err(format!("Question {row}: use a reference such as John 1:1"));
        }
        writer
            .write_record([
                q.question.trim(),
                q.qtype.trim(),
                q.reference.trim(),
                q.answer.trim(),
            ])
            .map_err(|error| format!("Could not write question {row}: {error}"))?;
    }
    let bytes = writer
        .into_inner()
        .map_err(|error| format!("Could not finish CSV: {error}"))?;
    String::from_utf8(bytes).map_err(|error| format!("Could not encode CSV: {error}"))
}

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

pub fn generate_questions(
    pool: &[QuestionRecord],
    req: &GenerateRequest,
) -> Result<Vec<QuestionRecord>, String> {
    if req.question_type.eq_ignore_ascii_case("custom") {
        return Err("Custom generation requires custom_counts".to_string());
    }
    if req.question_type.eq_ignore_ascii_case("standard") {
        return generate_standard_set(pool, req.situation.unwrap_or(true), req.seed);
    }

    let mut filtered: Vec<QuestionRecord> = pool
        .iter()
        .filter(|q| req.question_type == "all" || q.qtype.eq_ignore_ascii_case(&req.question_type))
        .cloned()
        .collect();

    if filtered.is_empty() {
        return Ok(vec![]);
    }

    let mut rng: StdRng = match req.seed {
        Some(seed) => SeedableRng::seed_from_u64(seed),
        None => SeedableRng::from_entropy(),
    };

    filtered.shuffle(&mut rng);
    filtered.truncate(req.count.min(filtered.len()));
    Ok(filtered)
}

fn question_shortages(
    requirements: &[(&str, usize)],
    available: impl Fn(&str) -> usize,
) -> Vec<String> {
    requirements
        .iter()
        .filter_map(|(qtype, required)| {
            let count = available(qtype);
            (count < *required).then(|| format!("{qtype} (need {required}, available {count})"))
        })
        .collect()
}

#[derive(Clone)]
struct FlowEdge {
    to: usize,
    reverse: usize,
    remaining: usize,
}

fn add_flow_edge(graph: &mut [Vec<FlowEdge>], from: usize, to: usize, capacity: usize) {
    let reverse = graph[to].len();
    let forward = graph[from].len();
    graph[from].push(FlowEdge {
        to,
        reverse,
        remaining: capacity,
    });
    graph[to].push(FlowEdge {
        to: from,
        reverse: forward,
        remaining: 0,
    });
}

pub fn generate_custom_set(
    pool: &[QuestionRecord],
    counts: &CustomCounts,
    seed: Option<u64>,
) -> Result<Vec<QuestionRecord>, String> {
    let requirements = counts.requirements();
    let total = requirements
        .iter()
        .try_fold(0usize, |total, (_, count)| total.checked_add(*count))
        .ok_or("Custom question total is too large")?;
    if total == 0 {
        return Err("Custom mode requires at least one question".to_string());
    }

    let mut chapters = Vec::new();
    let mut chapter_indexes = HashMap::new();
    let mut buckets = vec![Vec::<Vec<&QuestionRecord>>::new(); requirements.len()];
    for q in pool {
        let Some(key) = chapter_key(&q.reference) else {
            continue;
        };
        let chapter = *chapter_indexes.entry(key.clone()).or_insert_with(|| {
            chapters.push(key);
            chapters.len() - 1
        });
        if let Some(qtype) = requirements
            .iter()
            .position(|(qtype, _)| q.qtype.eq_ignore_ascii_case(qtype))
        {
            buckets[qtype].resize_with(chapters.len(), Vec::new);
            buckets[qtype][chapter].push(q);
        }
    }
    let shortages = question_shortages(&requirements, |qtype| {
        let index = requirements
            .iter()
            .position(|(name, _)| *name == qtype)
            .unwrap();
        buckets[index].iter().map(Vec::len).sum()
    });
    if !shortages.is_empty() {
        return Err(format!(
            "Cannot generate a custom {total}-question set from the selected books/chapters. \
             Insufficient question types: {}. Select more chapters, a different pool, \
             or reduce the requested counts.",
            shortages.join("; ")
        ));
    }

    let max_appearances = usize::max(2, total.div_ceil(chapters.len().max(1)));
    let mut rng = match seed {
        Some(seed) => StdRng::seed_from_u64(seed),
        None => StdRng::from_entropy(),
    };
    let source = 0;
    let chapter_start = 1 + requirements.len();
    let sink = chapter_start + chapters.len();
    let mut graph = vec![Vec::new(); sink + 1];
    let mut bucket_edges = Vec::new();
    let mut chapter_order: Vec<usize> = (0..chapters.len()).collect();
    chapter_order.shuffle(&mut rng);
    for (index, (_, count)) in requirements.iter().enumerate() {
        add_flow_edge(&mut graph, source, index + 1, *count);
        for &chapter in &chapter_order {
            let capacity = buckets[index].get(chapter).map_or(0, Vec::len);
            if capacity > 0 && *count > 0 {
                bucket_edges.push((index, chapter, graph[index + 1].len()));
                add_flow_edge(&mut graph, index + 1, chapter_start + chapter, capacity);
            }
        }
    }
    for chapter in 0..chapters.len() {
        add_flow_edge(&mut graph, chapter_start + chapter, sink, max_appearances);
    }

    // Residual paths can move earlier allocations so a feasible distribution is not
    // rejected just because a randomly chosen chapter filled up first.
    let mut selected = 0;
    while selected < total {
        let mut parents = vec![None; graph.len()];
        let mut queue = VecDeque::from([source]);
        parents[source] = Some((source, 0));
        while let Some(node) = queue.pop_front() {
            if node == sink {
                break;
            }
            for (edge_index, edge) in graph[node].iter().enumerate() {
                if edge.remaining > 0 && parents[edge.to].is_none() {
                    parents[edge.to] = Some((node, edge_index));
                    queue.push_back(edge.to);
                }
            }
        }
        if parents[sink].is_none() {
            return Err(format!(
                "Cannot generate a custom {total}-question set from the selected books/chapters. \
                 The requested type counts cannot be met while respecting the limit of \
                 {max_appearances} questions per chapter. Select more chapters, a different pool, \
                 or change the requested counts."
            ));
        }
        let mut amount = total - selected;
        let mut node = sink;
        while node != source {
            let (from, edge) = parents[node].unwrap();
            amount = amount.min(graph[from][edge].remaining);
            node = from;
        }
        node = sink;
        while node != source {
            let (from, edge) = parents[node].unwrap();
            let reverse = graph[from][edge].reverse;
            graph[from][edge].remaining -= amount;
            graph[node][reverse].remaining += amount;
            node = from;
        }
        selected += amount;
    }

    let mut set = Vec::with_capacity(total);
    for (qtype, chapter, edge) in bucket_edges {
        let questions = &mut buckets[qtype][chapter];
        let count = questions.len() - graph[qtype + 1][edge].remaining;
        questions.shuffle(&mut rng);
        set.extend(questions.iter().take(count).map(|q| (*q).clone()));
    }
    set.shuffle(&mut rng);
    Ok(set)
}

pub fn generate_standard_set(
    pool: &[QuestionRecord],
    situation: bool,
    seed: Option<u64>,
) -> Result<Vec<QuestionRecord>, String> {
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

    let first_type = if situation {
        "Situation"
    } else {
        "In-What-Book-and-Chapter"
    };
    let requirements = [
        (first_type, 1),
        ("Quote", 1),
        ("Reference", 1),
        ("Verse", 1),
        ("Context", 1),
        ("According-To", 4),
        ("General", 11),
    ];
    let shortages = question_shortages(&requirements, |qtype| {
        by_type.get(&qtype.to_ascii_lowercase()).map_or(0, Vec::len)
    });
    if !shortages.is_empty() {
        return Err(format!(
            "Cannot generate a standard 20-question set from the selected books/chapters. \
             Insufficient question types: {}. Select more chapters or a different pool.",
            shortages.join("; ")
        ));
    }

    let chapter_keys: HashSet<String> = source
        .iter()
        .filter_map(|(_, q)| chapter_key(&q.reference))
        .collect();

    let chapter_count = chapter_keys.len().max(1);
    let max_appearances = usize::max(2, 20_usize.div_ceil(chapter_count));
    let chapter_limit_error = |qtype: &str| {
        format!(
            "Cannot generate a standard 20-question set from the selected books/chapters. \
             Could not select enough {qtype} questions while respecting the limit of \
             {max_appearances} questions per chapter. Select more chapters or a different pool."
        )
    };

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
        return Err(chapter_limit_error(first_type));
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
            return Err(chapter_limit_error(t));
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
            return Err(chapter_limit_error("According-To"));
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
            return Err(chapter_limit_error("General"));
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
    Ok(set.into_iter().map(|(_, q)| q).collect())
}
