use std::collections::HashMap;

use qsets_server::services::generator::{generate_questions, parse_csv, GenerateRequest};

fn build_fixture_csv() -> String {
    let mut rows = vec!["Question,Type,Reference,Answer".to_string()];

    for ch in 1..=8 {
        rows.push(format!("Sit {ch},Situation,John {ch}:1,Ans"));
        rows.push(format!(
            "BookChap {ch},In-What-Book-and-Chapter,John {ch}:2,Ans"
        ));
        rows.push(format!("Quote {ch},Quote,John {ch}:3,Ans"));
        rows.push(format!("Ref {ch},Reference,John {ch}:4,Ans"));
        rows.push(format!("Verse {ch},Verse,John {ch}:5,Ans"));
        rows.push(format!("Context {ch},Context,John {ch}:6,Ans"));
        rows.push(format!("AT {ch}a,According-To,John {ch}:7,Ans"));
        rows.push(format!("AT {ch}b,According-To,John {ch}:8,Ans"));
        rows.push(format!("Gen {ch}a,General,John {ch}:9,Ans"));
        rows.push(format!("Gen {ch}b,General,John {ch}:10,Ans"));
        rows.push(format!("Gen {ch}c,General,John {ch}:11,Ans"));
    }

    rows.join("\n")
}

#[test]
fn seeded_generation_is_deterministic() {
    let csv = build_fixture_csv();

    let parsed = parse_csv(&csv);
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(true),
        seed: Some(12345),
    };

    let a = generate_questions(&parsed, &req);
    let b = generate_questions(&parsed, &req);

    assert_eq!(a.len(), 20);
    assert_eq!(a, b);
}

#[test]
fn standard_set_matches_required_type_counts() {
    let csv = build_fixture_csv();
    let parsed = parse_csv(&csv);
    let req = GenerateRequest {
        question_type: "standard".to_string(),
        count: 20,
        situation: Some(false),
        seed: Some(99),
    };

    let set = generate_questions(&parsed, &req);
    assert_eq!(set.len(), 20);

    let mut counts: HashMap<String, usize> = HashMap::new();
    for q in set {
        *counts.entry(q.qtype).or_insert(0) += 1;
    }

    assert_eq!(
        counts.get("In-What-Book-and-Chapter").copied().unwrap_or(0),
        1
    );
    assert_eq!(counts.get("Quote").copied().unwrap_or(0), 1);
    assert_eq!(counts.get("Reference").copied().unwrap_or(0), 1);
    assert_eq!(counts.get("Verse").copied().unwrap_or(0), 1);
    assert_eq!(counts.get("Context").copied().unwrap_or(0), 1);
    assert_eq!(counts.get("According-To").copied().unwrap_or(0), 4);
    assert_eq!(counts.get("General").copied().unwrap_or(0), 11);
}
