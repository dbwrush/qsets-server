use std::collections::{HashMap, HashSet};

use qsets_server::services::generator::{
    filter_by_books, filter_by_books_cow, generate_questions, list_books, parse_csv, validate_csv,
    BookFilter, GenerateRequest,
};

// ── Fixture helpers ──────────────────────────────────────────────────

fn full_fixture_csv() -> String {
    let mut rows = vec!["Question,Type,Reference,Answer".to_string()];
    for ch in 1..=8 {
        rows.push(format!("Sit {ch},Situation,John {ch}:1,Ans S{ch}"));
        rows.push(format!(
            "IWBAC {ch},In-What-Book-and-Chapter,John {ch}:2,Ans I{ch}"
        ));
        rows.push(format!("Quote {ch},Quote,John {ch}:3,Ans Q{ch}"));
        rows.push(format!("Ref {ch},Reference,John {ch}:4,Ans R{ch}"));
        rows.push(format!("Verse {ch},Verse,John {ch}:5,Ans V{ch}"));
        rows.push(format!("Ctx {ch},Context,John {ch}:6,Ans X{ch}"));
        rows.push(format!("AT {ch}a,According-To,John {ch}:7,Ans A{ch}a"));
        rows.push(format!("AT {ch}b,According-To,John {ch}:8,Ans A{ch}b"));
        rows.push(format!("Gen {ch}a,General,John {ch}:9,Ans G{ch}a"));
        rows.push(format!("Gen {ch}b,General,John {ch}:10,Ans G{ch}b"));
        rows.push(format!("Gen {ch}c,General,John {ch}:11,Ans G{ch}c"));
    }
    rows.join("\n")
}

fn multi_book_csv() -> String {
    let mut rows = vec!["Question,Type,Reference,Answer".to_string()];
    for ch in 1..=8 {
        for (t, ty) in [
            ("Sit", "Situation"),
            ("IWBAC", "In-What-Book-and-Chapter"),
            ("Quote", "Quote"),
            ("Ref", "Reference"),
            ("Verse", "Verse"),
            ("Ctx", "Context"),
            ("AT-a", "According-To"),
            ("AT-b", "According-To"),
            ("Gen-a", "General"),
            ("Gen-b", "General"),
            ("Gen-c", "General"),
        ] {
            rows.push(format!("{t} Luke {ch},{ty},Luke {ch}:{},Ans", ch + 1));
            rows.push(format!("{t} Acts {ch},{ty},Acts {ch}:{},Ans", ch + 1));
        }
    }
    rows.join("\n")
}

// ── parse_csv tests ──────────────────────────────────────────────────

#[test]
fn parse_csv_empty_input() {
    let pool = parse_csv("");
    assert!(pool.is_empty());
}

#[test]
fn parse_csv_headers_only() {
    let pool = parse_csv("Question,Type,Reference,Answer\n");
    assert!(pool.is_empty());
}

#[test]
fn parse_csv_skips_license_rows() {
    let csv = "Question,Type,Reference,Answer\nLICENSE stuff,General,John 1:1,Ans\nReal Q,General,John 1:2,Real A\n";
    let pool = parse_csv(csv);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].question, "Real Q");
}

#[test]
fn parse_csv_skips_rows_with_empty_fields() {
    let csv = "Question,Type,Reference,Answer\nQ1,,John 1:1,Ans\nQ2,General,,Ans\n,General,John 1:1,Ans\nQ4,General,John 1:1,\n";
    let pool = parse_csv(csv);
    assert!(pool.is_empty());
}

#[test]
fn parse_csv_skips_unparseable_references() {
    let csv = "Question,Type,Reference,Answer\nQ1,General,no_colon,Ans\nQ2,General,John 1:1,Ans\n";
    let pool = parse_csv(csv);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].reference, "John 1:1");
}

#[test]
fn parse_csv_handles_quoted_fields_with_commas() {
    let csv = r#"Question,Type,Reference,Answer
"Who said, ""follow me""?",General,John 1:43,"Jesus said, follow me"
"#;
    let pool = parse_csv(csv);
    assert_eq!(pool.len(), 1);
    assert!(pool[0].question.contains("follow me"));
}

#[test]
fn parse_csv_trims_whitespace() {
    let csv = "Question,Type,Reference,Answer\n  Q1  , General , John 1:1 , Ans \n";
    let pool = parse_csv(csv);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].question, "Q1");
    assert_eq!(pool[0].qtype, "General");
    assert_eq!(pool[0].reference, "John 1:1");
    assert_eq!(pool[0].answer, "Ans");
}

#[test]
fn parse_csv_uses_named_columns_in_any_order() {
    let csv = "Answer,Reference,Question,Type\nAns,John 1:1,Question text,General\n";
    let pool = parse_csv(csv);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].question, "Question text");
    assert_eq!(pool[0].qtype, "General");
    assert_eq!(pool[0].reference, "John 1:1");
    assert_eq!(pool[0].answer, "Ans");
}

#[test]
fn validate_csv_accepts_utf8_bom_and_reordered_columns() {
    let csv = "\u{feff}Answer,Reference,Question,Type\nAns,John 1:1,Question text,General\n";
    let result = validate_csv(csv);
    assert_eq!(result.valid_count, 1);
    assert!(
        result.errors.is_empty(),
        "Unexpected errors: {:?}",
        result.errors
    );
}

#[test]
fn parse_csv_multi_word_book_names() {
    let csv = "Question,Type,Reference,Answer\nQ1,General,1 John 3:16,Ans\nQ2,General,Song of Solomon 1:1,Ans\n";
    let pool = parse_csv(csv);
    assert_eq!(pool.len(), 2);
}

// ── validate_csv tests ──────────────────────────────────────────────

#[test]
fn validate_csv_valid_input_no_warnings() {
    let result = validate_csv(&full_fixture_csv());
    assert!(
        result.errors.is_empty(),
        "Expected no errors: {:?}",
        result.errors
    );
    assert_eq!(result.valid_count, 88);
}

#[test]
fn validate_csv_empty_input_error() {
    let result = validate_csv("");
    assert!(!result.errors.is_empty());
}

#[test]
fn validate_csv_detects_invalid_types() {
    let csv = "Question,Type,Reference,Answer\nQ1,FakeType,John 1:1,Ans\n";
    let result = validate_csv(csv);
    assert!(
        result.warnings.iter().any(|w| w.contains("FakeType")),
        "Expected warning about FakeType: {:?}",
        result.warnings
    );
}

#[test]
fn validate_csv_detects_missing_required_types() {
    // Only General questions — missing Situation, Quote, Reference, etc.
    let csv = "Question,Type,Reference,Answer\nQ1,General,John 1:1,Ans\nQ2,General,John 1:2,Ans\n";
    let result = validate_csv(csv);
    assert!(
        result.warnings.iter().any(|w| w.contains("missing")),
        "Expected warning about missing types: {:?}",
        result.warnings
    );
}

#[test]
fn validate_csv_reports_skipped_rows() {
    let csv = "Question,Type,Reference,Answer\n,General,John 1:1,Ans\nQ2,,John 1:2,Ans\nQ3,General,John 1:3,Valid\n";
    let result = validate_csv(csv);
    assert_eq!(result.skipped_count, 2);
    assert_eq!(result.valid_count, 1);
}

// ── list_books tests ─────────────────────────────────────────────────

#[test]
fn list_books_single_book() {
    let pool = parse_csv(&full_fixture_csv());
    let books = list_books(&pool);
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].name, "John");
    assert_eq!(books[0].min_chapter, 1);
    assert_eq!(books[0].max_chapter, 8);
}

#[test]
fn list_books_multiple_books_sorted() {
    let pool = parse_csv(&multi_book_csv());
    let books = list_books(&pool);
    assert_eq!(books.len(), 2);
    assert_eq!(books[0].name, "Acts");
    assert_eq!(books[1].name, "Luke");
}

#[test]
fn list_books_empty_pool() {
    let books = list_books(&[]);
    assert!(books.is_empty());
}

// ── filter_by_books tests ────────────────────────────────────────────

#[test]
fn filter_by_books_empty_filters_returns_all() {
    let pool = parse_csv(&full_fixture_csv());
    let filtered = filter_by_books(&pool, &[]);
    assert_eq!(filtered.len(), pool.len());
}

#[test]
fn filter_by_books_single_book() {
    let pool = parse_csv(&multi_book_csv());
    let filters = vec![BookFilter {
        name: "Luke".to_string(),
        start_chapter: 1,
        end_chapter: 4,
    }];
    let filtered = filter_by_books(&pool, &filters);
    assert!(filtered.iter().all(|q| q.reference.starts_with("Luke")));
    assert!(!filtered.is_empty());
}

#[test]
fn filter_by_books_cow_borrows_when_range_covers_pool() {
    let pool = parse_csv(&full_fixture_csv());
    let filters = vec![BookFilter {
        name: "John".to_string(),
        start_chapter: 1,
        end_chapter: 8,
    }];
    assert!(matches!(
        filter_by_books_cow(&pool, &filters),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn filter_by_books_chapter_range() {
    let pool = parse_csv(&full_fixture_csv());
    let filters = vec![BookFilter {
        name: "John".to_string(),
        start_chapter: 3,
        end_chapter: 5,
    }];
    let filtered = filter_by_books(&pool, &filters);
    // Should only contain chapters 3, 4, 5
    for q in &filtered {
        let ch: i32 = q
            .reference
            .split(' ')
            .next_back()
            .unwrap()
            .split(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            (3..=5).contains(&ch),
            "Unexpected chapter {} in {}",
            ch,
            q.reference
        );
    }
    // 3 chapters × 11 questions each
    assert_eq!(filtered.len(), 33);
}

#[test]
fn filter_by_books_swapped_start_end_still_works() {
    let pool = parse_csv(&full_fixture_csv());
    let filters = vec![BookFilter {
        name: "John".to_string(),
        start_chapter: 5,
        end_chapter: 3,
    }];
    let filtered = filter_by_books(&pool, &filters);
    assert_eq!(filtered.len(), 33);
}

#[test]
fn filter_by_books_nonexistent_book_returns_empty() {
    let pool = parse_csv(&full_fixture_csv());
    let filters = vec![BookFilter {
        name: "Revelation".to_string(),
        start_chapter: 1,
        end_chapter: 22,
    }];
    let filtered = filter_by_books(&pool, &filters);
    assert!(filtered.is_empty());
}

// ── generate_questions tests ─────────────────────────────────────────

#[test]
fn standard_set_with_situation() {
    let pool = parse_csv(&full_fixture_csv());
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(true),
        seed: Some(42),
    };
    let set = generate_questions(&pool, &req);
    assert_eq!(set.len(), 20);

    let mut counts: HashMap<String, usize> = HashMap::new();
    for q in &set {
        *counts.entry(q.qtype.clone()).or_insert(0) += 1;
    }
    assert_eq!(counts.get("Situation"), Some(&1));
    assert_eq!(counts.get("Quote"), Some(&1));
    assert_eq!(counts.get("Reference"), Some(&1));
    assert_eq!(counts.get("Verse"), Some(&1));
    assert_eq!(counts.get("Context"), Some(&1));
    assert_eq!(counts.get("According-To"), Some(&4));
    assert_eq!(counts.get("General"), Some(&11));
}

#[test]
fn standard_set_with_iwbac() {
    let pool = parse_csv(&full_fixture_csv());
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(false),
        seed: Some(42),
    };
    let set = generate_questions(&pool, &req);
    assert_eq!(set.len(), 20);

    let mut counts: HashMap<String, usize> = HashMap::new();
    for q in &set {
        *counts.entry(q.qtype.clone()).or_insert(0) += 1;
    }
    assert_eq!(counts.get("In-What-Book-and-Chapter"), Some(&1));
    assert!(!counts.contains_key("Situation"));
}

#[test]
fn standard_set_no_duplicate_questions() {
    let pool = parse_csv(&full_fixture_csv());
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(true),
        seed: Some(777),
    };
    let set = generate_questions(&pool, &req);
    let questions: HashSet<&str> = set.iter().map(|q| q.question.as_str()).collect();
    assert_eq!(
        questions.len(),
        set.len(),
        "Duplicate questions found in set"
    );
}

#[test]
fn standard_set_chapter_spread() {
    let pool = parse_csv(&full_fixture_csv());
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(true),
        seed: Some(123),
    };
    let set = generate_questions(&pool, &req);

    // With 8 chapters and 20 questions, max per chapter = ceil(20/8) = 3
    let mut ch_counts: HashMap<String, usize> = HashMap::new();
    for q in &set {
        let key = q.reference.split(':').next().unwrap().to_string();
        *ch_counts.entry(key).or_insert(0) += 1;
    }
    let max_per_ch = *ch_counts.values().max().unwrap();
    assert!(
        max_per_ch <= 3,
        "Chapter usage cap violated: max {} per chapter, found {}",
        3,
        max_per_ch
    );
}

#[test]
fn different_seeds_produce_different_sets() {
    let pool = parse_csv(&full_fixture_csv());
    let a = generate_questions(
        &pool,
        &GenerateRequest {
            question_type: "standard".to_string(),
            count: 20,
            situation: Some(true),
            seed: Some(1),
        },
    );
    let b = generate_questions(
        &pool,
        &GenerateRequest {
            question_type: "standard".to_string(),
            count: 20,
            situation: Some(true),
            seed: Some(2),
        },
    );
    assert_ne!(a, b, "Different seeds should produce different sets");
}

#[test]
fn generate_non_standard_type_filter() {
    let pool = parse_csv(&full_fixture_csv());
    let req = GenerateRequest {
        question_type: "General".to_string(),
        count: 5,
        situation: None,
        seed: Some(10),
    };
    let set = generate_questions(&pool, &req);
    assert_eq!(set.len(), 5);
    assert!(set.iter().all(|q| q.qtype == "General"));
}

#[test]
fn generate_all_type_returns_mixed() {
    let pool = parse_csv(&full_fixture_csv());
    let req = GenerateRequest {
        question_type: "all".to_string(),
        count: 30,
        situation: None,
        seed: Some(10),
    };
    let set = generate_questions(&pool, &req);
    assert_eq!(set.len(), 30);
    let types: HashSet<&str> = set.iter().map(|q| q.qtype.as_str()).collect();
    assert!(types.len() > 1, "Expected multiple types in 'all' mode");
}

#[test]
fn generate_count_capped_to_available() {
    let csv = "Question,Type,Reference,Answer\nQ1,General,John 1:1,Ans\nQ2,General,John 1:2,Ans\n";
    let pool = parse_csv(csv);
    let req = GenerateRequest {
        question_type: "all".to_string(),
        count: 100,
        situation: None,
        seed: Some(1),
    };
    let set = generate_questions(&pool, &req);
    assert_eq!(set.len(), 2);
}

#[test]
fn generate_empty_pool_returns_empty() {
    let pool = parse_csv("Question,Type,Reference,Answer\n");
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(true),
        seed: Some(1),
    };
    let set = generate_questions(&pool, &req);
    assert!(set.is_empty());
}

// ── Multi-book generation ────────────────────────────────────────────

#[test]
fn standard_set_from_filtered_multi_book_pool() {
    let pool = parse_csv(&multi_book_csv());
    let filters = vec![BookFilter {
        name: "Luke".to_string(),
        start_chapter: 1,
        end_chapter: 8,
    }];
    let filtered = filter_by_books(&pool, &filters);
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(true),
        seed: Some(55),
    };
    let set = generate_questions(&filtered, &req);
    assert_eq!(set.len(), 20);
    assert!(set.iter().all(|q| q.reference.starts_with("Luke")));
}

// ── Stress: many seeds produce valid sets ────────────────────────────

#[test]
fn hundred_seeds_all_produce_valid_20_question_sets() {
    let pool = parse_csv(&full_fixture_csv());
    for seed in 0..100 {
        let req = GenerateRequest {
            question_type: "standard".to_string(),
            count: 20,
            situation: Some(seed % 2 == 0),
            seed: Some(seed),
        };
        let set = generate_questions(&pool, &req);
        assert_eq!(
            set.len(),
            20,
            "Seed {} produced {} questions instead of 20",
            seed,
            set.len()
        );
    }
}
