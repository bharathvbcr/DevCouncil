//! The stub gates, measured on the labelled corpus down both paths.
//!
//! `false_accept.rs` gates `cases.txt` at zero errors down the path a new-file
//! diff takes (the syntax tree). That says nothing about the *other* path: every
//! file with no usable post-image — a modified file the host cannot read, an
//! unparseable one, any language without a grammar — takes the substring
//! fallback. Here the same labelled cases are run down both, and the confusion
//! matrix of each is pinned, per gate, so a number quoted in `rust/STATUS.md`
//! cannot drift from the code.
//!
//! A case is positive for a gate when the corpus expects anything but `clean`
//! from it, and flagged when the gate reports a finding at any severity — the
//! retired Python detector (`corpus/retired_python_oracle.py`) had no severity,
//! so any-severity is the one reading both sides share.
//!
//! The fallback matrix is *not* gated at zero, and that is the finding: it
//! records what the fallback misses (the tree-only gates, and constructs a line
//! cannot show), where the tree path is gated at zero.

use dc_verify::rigor::{detect_stubs, detect_stubs_with};
use dc_verify::{ChangeStatus, FileDiff, parse_unified};

const GATES: [&str; 3] = ["stub_detection", "assert_free_test", "skipped_test"];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Matrix {
    tp: usize,
    fp: usize,
    fn_: usize,
    tn: usize,
}

impl Matrix {
    fn record(&mut self, positive: bool, flagged: bool) {
        match (positive, flagged) {
            (true, true) => self.tp += 1,
            (true, false) => self.fn_ += 1,
            (false, true) => self.fp += 1,
            (false, false) => self.tn += 1,
        }
    }
}

struct Case {
    name: String,
    expect: [bool; 3],
    diff: String,
}

fn load() -> Vec<Case> {
    let raw = include_str!("corpus/cases.txt");
    let mut cases = Vec::new();
    let mut current: Option<(String, Vec<String>, Vec<String>)> = None;
    for line in raw.lines() {
        if let Some(name) = line.strip_prefix("### case:") {
            if let Some(done) = current.take() {
                cases.push(build(done));
            }
            current = Some((name.trim().to_string(), Vec::new(), Vec::new()));
        } else if let Some((_, headers, body)) = current.as_mut() {
            match line.strip_prefix("###") {
                Some(header) => headers.push(header.trim().to_string()),
                None => body.push(line.to_string()),
            }
        }
    }
    if let Some(done) = current.take() {
        cases.push(build(done));
    }
    assert!(!cases.is_empty(), "the corpus produced no cases");
    cases
}

fn build((name, headers, body): (String, Vec<String>, Vec<String>)) -> Case {
    let mut expect = [false; 3];
    for (i, gate) in GATES.iter().enumerate() {
        let key = format!("{gate}:");
        let value = headers
            .iter()
            .find_map(|h| h.strip_prefix(&key).map(|v| v.trim().to_string()))
            .unwrap_or_else(|| panic!("case {name:?} declares no `{key}`"));
        expect[i] = match value.as_str() {
            "clean" => false,
            "blocking" | "advisory" => true,
            other => panic!("case {name:?}: {gate} expectation {other:?}"),
        };
    }
    Case {
        name,
        expect,
        diff: body.join("\n"),
    }
}

fn measure(fallback: bool) -> ([Matrix; 3], usize) {
    let mut matrices = [Matrix::default(); 3];
    let cases = load();
    for case in &cases {
        let mut files: Vec<FileDiff> = parse_unified(&case.diff)
            .unwrap_or_else(|e| panic!("case {:?} does not parse: {e}", case.name));
        let findings = if fallback {
            // No post-image: a whole-file addition is demoted to a modification
            // so the diff alone is what the checks see.
            for f in &mut files {
                f.status = ChangeStatus::Modified;
            }
            detect_stubs_with(&files, &|_| None)
        } else {
            detect_stubs(&files)
        };
        for (i, gate) in GATES.iter().enumerate() {
            let flagged = findings.iter().any(|f| f.gate == *gate);
            matrices[i].record(case.expect[i], flagged);
        }
    }
    (matrices, cases.len())
}

fn show(path: &str, m: [Matrix; 3], n: usize) -> String {
    let ratio = |a: usize, b: usize| {
        if b == 0 {
            "undefined".to_string()
        } else {
            format!("{:.3} ({a}/{b})", a as f64 / b as f64)
        }
    };
    let mut out = format!("{path} path, {n} cases\n");
    for (gate, m) in GATES.iter().zip(m) {
        out += &format!(
            "  {gate}: tp {} fp {} fn {} tn {}; precision {}, recall {}\n",
            m.tp,
            m.fp,
            m.fn_,
            m.tn,
            ratio(m.tp, m.tp + m.fp),
            ratio(m.tp, m.tp + m.fn_),
        );
    }
    out
}

#[test]
fn the_tree_path_is_exact_on_the_corpus() {
    let (m, n) = measure(false);
    println!("{}", show("tree", m, n));
    for (gate, m) in GATES.iter().zip(m) {
        assert_eq!((m.fp, m.fn_), (0, 0), "{gate} on the tree path: {m:?}");
    }
}

#[test]
fn the_substring_fallback_matrix_is_pinned() {
    let (m, n) = measure(true);
    println!("{}", show("substring fallback", m, n));
    // Updated together with rust/STATUS.md §4. See the output of
    // `cargo test -p dc-verify --test stub_measurement -- --nocapture`.
    let pinned: [Matrix; 3] = PINNED;
    assert_eq!(m, pinned, "{}", show("substring fallback", m, n));
}

/// Measured before this change (the substring fallback as it was, with only the
/// exact no-argument spellings), stub_detection was tp 21 fp 4 fn 5 tn 40. The
/// fallback has no reading of tests, so both test gates miss every case.
const PINNED: [Matrix; 3] = [
    Matrix {
        tp: 23,
        fp: 4,
        fn_: 3,
        tn: 40,
    },
    Matrix {
        tp: 0,
        fp: 0,
        fn_: 2,
        tn: 68,
    },
    Matrix {
        tp: 0,
        fp: 0,
        fn_: 4,
        tn: 66,
    },
];
