// Generated with AI Coding Rules Hub
//! Tests for rerank parser (TS-10) and benchmark profile (TS-11).
//!
//! TS-10: tolerant index parsing across 4 fixture variants.
//! TS-11: BenchmarkProfile defaults — BEAM disables rerank,
//! LoCoMo + LongMemEval enable it.

use memlayer_eval::config::{default_profile, RetrievalMode};
use memlayer_eval::rerank::{build_rerank_prompt, parse_rerank_indices};
use memlayer_eval::runner::BenchmarkKind;

// ---------- TS-10 ----------

#[test]
fn ts10_parses_clean_csv() {
    let out = parse_rerank_indices("3,1,7,2,5", 30);
    assert_eq!(out, vec![2, 0, 6, 1, 4], "clean CSV → 0-indexed");
}

#[test]
fn ts10_parses_numbered_list() {
    let raw = "1.\n2.\n3.\n4.\n5.";
    let out = parse_rerank_indices(raw, 30);
    assert_eq!(out, vec![0, 1, 2, 3, 4]);
}

#[test]
fn ts10_parses_prose_prefix() {
    let raw = "Top picks: 3, 1, 7, then 2 and 5.";
    let out = parse_rerank_indices(raw, 30);
    assert_eq!(out, vec![2, 0, 6, 1, 4]);
}

#[test]
fn ts10_garbage_returns_empty() {
    let out = parse_rerank_indices("I don't know", 30);
    assert_eq!(out, Vec::<usize>::new());
}

#[test]
fn ts10_dedupes_preserving_order() {
    let out = parse_rerank_indices("1, 2, 1, 3, 2, 4", 30);
    assert_eq!(out, vec![0, 1, 2, 3], "dedupe by first occurrence");
}

#[test]
fn ts10_filters_out_of_range() {
    let out = parse_rerank_indices("1, 99, 2, 100, 3", 30);
    assert_eq!(out, vec![0, 1, 2], "drops indices > max");
}

#[test]
fn ts10_zero_filtered() {
    // "0" is out of range (1-indexed) so should be skipped.
    let out = parse_rerank_indices("0, 1, 2, 3", 30);
    assert_eq!(out, vec![0, 1, 2]);
}

#[test]
fn ts10_multi_digit_handled() {
    let out = parse_rerank_indices("12, 3, 25, 1", 30);
    assert_eq!(out, vec![11, 2, 24, 0]);
}

#[test]
fn ts10_prompt_includes_question_and_count() {
    let candidates: Vec<String> = (1..=15).map(|i| format!("memory {i}")).collect();
    let prompt = build_rerank_prompt(&candidates, "What did Caroline say?", 10);
    assert!(prompt.contains("What did Caroline say?"));
    assert!(prompt.contains("top 10"));
    assert!(prompt.contains("[1] memory 1"));
    assert!(prompt.contains("[15] memory 15"));
}

// ---------- TS-11 ----------

#[test]
fn ts11_locomo_enables_rerank() {
    let p = default_profile(BenchmarkKind::Locomo);
    assert!(p.rerank, "LoCoMo should enable rerank");
    assert_eq!(p.mode, RetrievalMode::HybridRerank);
    assert_eq!(p.k, 20);
    assert_eq!(p.evidence_window, 6);
}

#[test]
fn ts11_longmemeval_enables_rerank() {
    let p = default_profile(BenchmarkKind::Longmemeval);
    assert!(p.rerank, "LongMemEval should enable rerank");
    assert_eq!(p.mode, RetrievalMode::HybridRerank);
    assert!(p.evidence_window > 0);
}

#[test]
fn ts11_beam1m_disables_rerank() {
    let p = default_profile(BenchmarkKind::Beam1m);
    assert!(!p.rerank, "BEAM-1M should disable rerank (LLM cost not justified)");
    assert_eq!(p.mode, RetrievalMode::Hybrid);
    assert_eq!(p.evidence_window, 0);
}

#[test]
fn ts11_beam10m_disables_rerank() {
    let p = default_profile(BenchmarkKind::Beam10m);
    assert!(!p.rerank, "BEAM-10M should disable rerank");
    assert_eq!(p.mode, RetrievalMode::Hybrid);
    assert_eq!(p.evidence_window, 0);
}
