//! The substring fallback on the held-out stub corpus.
//!
//! `corpus/stub_holdout.tsv` was labelled by the rule in its header before the
//! fallback was extended, and is not tuned to pass: it contains languages the
//! fallback does not read. The matrix is pinned so `rust/STATUS.md` can quote
//! it. A row is flagged when the fallback reports a *blocking*
//! `stub_detection` finding for its line.

use dc_verify::rigor::{Severity, detect_stubs};
use dc_verify::{ChangeStatus, FileDiff};

#[derive(Debug, Default, PartialEq, Eq)]
struct Matrix {
    tp: usize,
    fp: usize,
    fn_: usize,
    tn: usize,
}

#[test]
fn the_fallback_matrix_on_the_held_out_corpus_is_pinned() {
    let raw = include_str!("corpus/stub_holdout.tsv");
    let mut m = Matrix::default();
    let mut wrong = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.splitn(4, '\t').collect();
        let [label, path, note, content] = cols[..] else {
            panic!(
                "stub_holdout.tsv:{}: expected 4 tab-separated columns",
                i + 1
            );
        };
        let positive = match label {
            "stub" => true,
            "clean" => false,
            other => panic!("stub_holdout.tsv:{}: label {other:?}", i + 1),
        };
        let file = FileDiff {
            path: path.to_string(),
            old_path: None,
            status: ChangeStatus::Modified,
            added_lines: vec![(3, content.to_string())],
            removed_lines: Vec::new(),
        };
        let flagged = detect_stubs(&[file])
            .iter()
            .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking);
        match (positive, flagged) {
            (true, true) => m.tp += 1,
            (true, false) => {
                m.fn_ += 1;
                wrong.push(format!("  [missed] {path}: {note}"));
            }
            (false, true) => {
                m.fp += 1;
                wrong.push(format!("  [false positive] {path}: {note}"));
            }
            (false, false) => m.tn += 1,
        }
    }
    let ratio = |a: usize, b: usize| format!("{:.3} ({a}/{b})", a as f64 / b as f64);
    println!(
        "substring fallback on stub_holdout.tsv: tp {} fp {} fn {} tn {}; precision {}, recall {}\n{}",
        m.tp,
        m.fp,
        m.fn_,
        m.tn,
        ratio(m.tp, m.tp + m.fp),
        ratio(m.tp, m.tp + m.fn_),
        wrong.join("\n")
    );
    assert_eq!(m, PINNED, "{}", wrong.join("\n"));
}

/// The four misses are Swift, Scala, Dart and C: languages the fallback has no
/// reading of. Closing them changes this and STATUS.md together.
const PINNED: Matrix = Matrix {
    tp: 22,
    fp: 0,
    fn_: 4,
    tn: 25,
};
