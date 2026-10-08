//! `scan_secrets` and `classify_scope`, measured as classifiers against hand
//! labels.
//!
//! `false_accept.rs` gates both error directions at zero over cases written
//! for the gates. That is a regression fence, not a measurement of how good a
//! gate is on input nobody tuned it for: a corpus whose every case passes says
//! nothing about the cases not in it. These two corpora are labelled by a
//! written rule, before either gate was run, so the matrix below is the gate's
//! error rate on that sample rather than a restatement of its own output.
//!
//! The counts are pinned. rust/STATUS.md quotes them, and a change that moves
//! one must move the document with it — an improvement as much as a
//! regression, because a number in a document that no longer matches the code
//! is the "unmeasured" this file replaced, wearing a figure.

use std::collections::BTreeSet;

use dc_verify::rigor::scan_secrets;
use dc_verify::{ChangeStatus, FileDiff, classify_scope, parse_unified};

/// A binary confusion matrix. Positive is the class the gate exists to catch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Matrix {
    tp: usize,
    fp: usize,
    fn_: usize,
    tn: usize,
}

impl Matrix {
    fn record(&mut self, labelled_positive: bool, flagged: bool) {
        match (labelled_positive, flagged) {
            (true, true) => self.tp += 1,
            (true, false) => self.fn_ += 1,
            (false, true) => self.fp += 1,
            (false, false) => self.tn += 1,
        }
    }

    fn report(&self, name: &str) -> String {
        let ratio = |num: usize, den: usize| {
            if den == 0 {
                "undefined (no cases)".to_string()
            } else {
                format!("{:.3} ({num}/{den})", num as f64 / den as f64)
            }
        };
        format!(
            "{name}: {} labelled items — tp {} fp {} fn {} tn {}; precision {}, recall {}",
            self.tp + self.fp + self.fn_ + self.tn,
            self.tp,
            self.fp,
            self.fn_,
            self.tn,
            ratio(self.tp, self.tp + self.fp),
            ratio(self.tp, self.tp + self.fn_),
        )
    }
}

/// One labelled secret line, and whether the gate flagged it.
struct SecretRow {
    secret: bool,
    path: String,
    note: String,
    content: String,
}

fn load_secrets() -> Vec<SecretRow> {
    let raw = include_str!("corpus/secrets.tsv");
    let mut rows = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.splitn(4, '\t').collect();
        let [label, path, note, content] = cols[..] else {
            panic!("secrets.tsv:{}: expected 4 tab-separated columns", i + 1);
        };
        let secret = match label {
            "secret" => true,
            "clean" => false,
            other => panic!(
                "secrets.tsv:{}: label {other:?} is not secret or clean",
                i + 1
            ),
        };
        rows.push(SecretRow {
            secret,
            path: path.to_string(),
            note: note.to_string(),
            content: content.to_string(),
        });
    }
    rows
}

#[test]
fn scan_secrets_precision_and_recall_on_the_hand_labelled_corpus() {
    let rows = load_secrets();
    let mut matrix = Matrix::default();
    let mut wrong = Vec::new();
    for row in &rows {
        let file = FileDiff {
            path: row.path.clone(),
            old_path: None,
            status: ChangeStatus::Modified,
            added_lines: vec![(1, row.content.clone())],
            removed_lines: Vec::new(),
        };
        let flagged = !scan_secrets(&[file]).is_empty();
        matrix.record(row.secret, flagged);
        if flagged != row.secret {
            wrong.push(format!(
                "  [{}] {}: {}",
                if row.secret {
                    "missed"
                } else {
                    "false positive"
                },
                row.path,
                row.note
            ));
        }
    }
    println!("\n{}", matrix.report("scan_secrets"));
    for w in &wrong {
        println!("{w}");
    }

    assert!(
        matrix.tp + matrix.fn_ >= 30 && matrix.fp + matrix.tn >= 30,
        "the secret corpus needs both classes to measure both directions: {matrix:?}"
    );
    assert_eq!(
        matrix,
        Matrix {
            tp: SECRETS_TP,
            fp: SECRETS_FP,
            fn_: SECRETS_FN,
            tn: SECRETS_TN
        },
        "scan_secrets moved on the hand-labelled corpus; update the pinned counts \
         here and the figures in rust/STATUS.md together\n{}",
        wrong.join("\n")
    );
}

// Pinned from the run recorded in rust/STATUS.md.
const SECRETS_TP: usize = 20;
const SECRETS_FP: usize = 0;
const SECRETS_FN: usize = 20;
const SECRETS_TN: usize = 41;

/// One scope case: the plan, the change, and the hand label of every path the
/// change touches.
struct ScopeCase {
    name: String,
    planned: Vec<String>,
    orphans: BTreeSet<String>,
    in_scope: BTreeSet<String>,
    diff: String,
}

fn load_scope() -> Vec<ScopeCase> {
    let raw = include_str!("corpus/scope.txt");
    let mut cases: Vec<ScopeCase> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    let flush = |cases: &mut Vec<ScopeCase>, body: &mut Vec<&str>| {
        if let Some(case) = cases.last_mut() {
            case.diff = body.join("\n");
            case.diff.push('\n');
        }
        body.clear();
    };
    for line in raw.lines() {
        if let Some(name) = line.strip_prefix("### case:") {
            flush(&mut cases, &mut body);
            cases.push(ScopeCase {
                name: name.trim().to_string(),
                planned: Vec::new(),
                orphans: BTreeSet::new(),
                in_scope: BTreeSet::new(),
                diff: String::new(),
            });
            continue;
        }
        let Some(case) = cases.last_mut() else {
            continue;
        };
        if let Some(header) = line.strip_prefix("###") {
            let header = header.trim();
            if let Some(v) = header.strip_prefix("planned:") {
                case.planned.push(v.trim().to_string());
            } else if let Some(v) = header.strip_prefix("orphan:") {
                case.orphans.insert(v.trim().to_string());
            } else if let Some(v) = header.strip_prefix("in:") {
                case.in_scope.insert(v.trim().to_string());
            }
        } else if !(line.is_empty() && body.is_empty()) {
            body.push(line);
        }
    }
    flush(&mut cases, &mut body);
    for case in &mut cases {
        // Blank separator lines between cases are not part of the diff.
        while case.diff.ends_with("\n\n") {
            case.diff.pop();
        }
        assert!(
            !case.orphans.is_empty() || !case.in_scope.is_empty(),
            "scope case {:?} labels no path",
            case.name
        );
    }
    assert!(!cases.is_empty(), "the scope corpus produced no cases");
    cases
}

#[test]
fn classify_scope_precision_and_recall_on_the_hand_labelled_corpus() {
    let cases = load_scope();
    let mut matrix = Matrix::default();
    let mut refused = Vec::new();
    let mut wrong = Vec::new();
    for case in &cases {
        let files = match parse_unified(&case.diff) {
            Ok(files) => files,
            Err(e) => {
                // The verifier fails closed on a diff it cannot parse, so a
                // refusal is not an accept; it is also not a classification,
                // and is counted apart rather than folded into either cell.
                refused.push(format!("  [refused] {}: {e}", case.name));
                continue;
            }
        };
        let report = classify_scope(&files, &case.planned);
        let reported: BTreeSet<&String> = report.orphans.iter().chain(&report.in_scope).collect();
        for path in &reported {
            assert!(
                case.orphans.contains(*path) || case.in_scope.contains(*path),
                "scope case {:?}: the gate reported {path:?}, which the case does not \
                 label; an unlabelled output is not a measurement",
                case.name
            );
        }
        for (path, positive) in case
            .orphans
            .iter()
            .map(|p| (p, true))
            .chain(case.in_scope.iter().map(|p| (p, false)))
        {
            let flagged = report.orphans.contains(path);
            matrix.record(positive, flagged);
            if flagged != positive {
                wrong.push(format!(
                    "  [{}] {}: {path}",
                    if positive { "missed" } else { "false positive" },
                    case.name
                ));
            }
        }
    }
    println!(
        "\n{}; {} case(s) refused by the parser",
        matrix.report("classify_scope"),
        refused.len()
    );
    for line in refused.iter().chain(&wrong) {
        println!("{line}");
    }

    assert_eq!(
        (matrix, refused.len()),
        (
            Matrix {
                tp: SCOPE_TP,
                fp: SCOPE_FP,
                fn_: SCOPE_FN,
                tn: SCOPE_TN
            },
            SCOPE_REFUSED
        ),
        "classify_scope moved on the hand-labelled corpus; update the pinned counts \
         here and the figures in rust/STATUS.md together\n{}",
        refused
            .iter()
            .chain(&wrong)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// Pinned from the run recorded in rust/STATUS.md.
const SCOPE_TP: usize = 14;
const SCOPE_FP: usize = 0;
const SCOPE_FN: usize = 0;
const SCOPE_TN: usize = 22;
const SCOPE_REFUSED: usize = 0;
