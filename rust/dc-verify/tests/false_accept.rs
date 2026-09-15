//! The gates, measured as classifiers.
//!
//! `rigor.rs` and `coverage.rs` prove each gate fires on the input it was
//! written for. Neither can say how *often* a gate is wrong, in either
//! direction, because a test suite made of cases a gate passes has no
//! denominator. That is the gap this file closes, and it is borrowed directly
//! from reverify, which measures its binary verifier against known-true and
//! known-false claims and fails its build if a single wrong claim is accepted.
//!
//! Two error directions, and they are not interchangeable:
//!
//!   - A **false accept** is a case the corpus marks as something a gate must
//!     catch, which the gate passed. This is the direction that turns
//!     "unexamined" into "approved" — a credential reaching history, a `todo!()`
//!     shipping as a finished task.
//!
//!   - A **false positive** is a clean case a gate flagged. This is the
//!     direction that gets a gate switched off, after which it catches nothing
//!     at all. Every gate in this crate carries a comment saying so.
//!
//! Both are gated at zero. Neither is a warning: a corpus that reports a
//! nonzero matrix and exits successfully is a measurement nobody acts on.
//!
//! The corpus is `corpus/cases.txt`, and its header documents the format. Every
//! case declares an expectation for *every* gate, so each fixture is also a
//! standing negative control for the gates it was not written for.

use dc_verify::parse_unified;
use dc_verify::rigor::{Severity, detect_stubs, scan_secrets};
use dc_verify::substance;

/// What a findings gate must say about a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// At least one blocking finding.
    Blocking,
    /// At least one advisory finding and no blocking one.
    Advisory,
    /// Nothing at all, at any severity.
    Clean,
}

impl Expect {
    fn parse(value: &str, case: &str, gate: &str) -> Expect {
        match value {
            "blocking" => Expect::Blocking,
            "advisory" => Expect::Advisory,
            "clean" => Expect::Clean,
            other => panic!(
                "case {case:?}: {gate} expectation {other:?} is not one of \
                 blocking, advisory, clean"
            ),
        }
    }

    /// Reduces a gate's output for one case to the same vocabulary.
    fn observed(blocking: usize, advisory: usize) -> Expect {
        if blocking > 0 {
            Expect::Blocking
        } else if advisory > 0 {
            Expect::Advisory
        } else {
            Expect::Clean
        }
    }
}

/// What the substance measurement must say about a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectSubstance {
    Low,
    Substantive,
    Unjudged,
}

impl ExpectSubstance {
    fn parse(value: &str, case: &str) -> ExpectSubstance {
        match value {
            "low" => ExpectSubstance::Low,
            "substantive" => ExpectSubstance::Substantive,
            "unjudged" => ExpectSubstance::Unjudged,
            other => panic!(
                "case {case:?}: substance expectation {other:?} is not one of \
                 low, substantive, unjudged"
            ),
        }
    }

    fn observed(report: &substance::SubstanceReport) -> ExpectSubstance {
        if !report.judged() {
            ExpectSubstance::Unjudged
        } else if report.is_low() {
            ExpectSubstance::Low
        } else {
            ExpectSubstance::Substantive
        }
    }
}

struct Case {
    name: String,
    secret_scan: Expect,
    stub_detection: Expect,
    substance: ExpectSubstance,
    diff: String,
}

/// Splits the corpus into cases.
///
/// Panics rather than returning an error, and on every malformed shape rather
/// than skipping it: a corpus that silently drops a case it could not read
/// reports a clean matrix over fewer cases than it claims, which is this
/// crate's own cardinal sin committed by the file that exists to catch it.
fn load_corpus() -> Vec<Case> {
    let raw = include_str!("corpus/cases.txt");
    let mut cases = Vec::new();
    let mut current: Option<(String, Vec<String>, Vec<String>)> = None;

    for line in raw.lines() {
        if let Some(name) = line.strip_prefix("### case:") {
            if let Some((name, headers, body)) = current.take() {
                cases.push(build_case(name, &headers, &body));
            }
            current = Some((name.trim().to_string(), Vec::new(), Vec::new()));
            continue;
        }
        let Some((_, headers, body)) = current.as_mut() else {
            // Before the first case marker: the file's own documentation.
            continue;
        };
        if let Some(header) = line.strip_prefix("###") {
            // Headers only count before the diff starts. After it, a `###`
            // at column 0 cannot occur — a diff body line always begins with
            // `+`, `-`, a space or a backslash — so this needs no state.
            headers.push(header.trim().to_string());
        } else {
            body.push(line.to_string());
        }
    }
    if let Some((name, headers, body)) = current.take() {
        cases.push(build_case(name, &headers, &body));
    }

    assert!(
        !cases.is_empty(),
        "the corpus produced no cases; a matrix over zero cases is not a measurement"
    );
    cases
}

fn build_case(name: String, headers: &[String], body: &[String]) -> Case {
    let find = |key: &str| -> String {
        headers
            .iter()
            .find_map(|h| h.strip_prefix(key).map(|v| v.trim().to_string()))
            .unwrap_or_else(|| {
                panic!(
                    "case {name:?} declares no `{key}` expectation; every case must \
                     declare one for every gate, so that a fixture written for one \
                     gate is a negative control for the others"
                )
            })
    };
    let secret_scan = Expect::parse(&find("secret_scan:"), &name, "secret_scan");
    let stub_detection = Expect::parse(&find("stub_detection:"), &name, "stub_detection");
    let substance = ExpectSubstance::parse(&find("substance:"), &name);

    let diff = body.join("\n");
    assert!(
        diff.contains("diff --git"),
        "case {name:?} has no diff body"
    );
    Case {
        name,
        secret_scan,
        stub_detection,
        substance,
        diff,
    }
}

/// One row of the report: what went wrong, on which case.
struct Mismatch {
    case: String,
    gate: &'static str,
    expected: String,
    observed: String,
    /// Whether this is the dangerous direction — a gate that should have
    /// caught something and did not.
    false_accept: bool,
}

#[test]
fn the_gates_accept_nothing_they_should_catch_and_flag_nothing_clean() {
    let cases = load_corpus();
    let mut mismatches: Vec<Mismatch> = Vec::new();
    let (mut known_positive, mut known_negative) = (0usize, 0usize);

    for case in &cases {
        let files = parse_unified(&case.diff).unwrap_or_else(|e| {
            panic!(
                "case {:?} does not parse: {e}\n\
                 A fixture the parser rejects is measured as neither a hit nor a \
                 miss, so it would quietly leave the corpus.",
                case.name
            )
        });

        // --- the two findings gates ---
        for (gate, expected, findings) in [
            ("secret_scan", case.secret_scan, scan_secrets(&files)),
            ("stub_detection", case.stub_detection, detect_stubs(&files)),
        ] {
            let mine: Vec<_> = findings.iter().filter(|f| f.gate == gate).collect();
            let blocking = mine
                .iter()
                .filter(|f| f.severity == Severity::Blocking)
                .count();
            let advisory = mine
                .iter()
                .filter(|f| f.severity == Severity::Advisory)
                .count();
            let observed = Expect::observed(blocking, advisory);

            match expected {
                Expect::Clean => known_negative += 1,
                _ => known_positive += 1,
            }

            if observed != expected {
                mismatches.push(Mismatch {
                    case: case.name.clone(),
                    gate,
                    expected: format!("{expected:?}"),
                    observed: format!("{observed:?}"),
                    // Anything weaker than the corpus demanded is an accept:
                    // a missed blocking finding, and also a blocking one
                    // downgraded to advisory, which no longer stops a task.
                    false_accept: matches!(
                        (expected, observed),
                        (Expect::Blocking, _) | (Expect::Advisory, Expect::Clean)
                    ),
                });
            }

            // The evidence of every finding must be safe to print, whatever
            // gate produced it. This is not a separate corpus because the
            // condition is universal: no report may quote a credential, and
            // `secret-beside-a-todo-marker` is the case that once did.
            for finding in &mine {
                assert!(
                    !dc_verify::rigor::contains_secret(&finding.evidence),
                    "case {:?}: the {gate} gate quoted a credential in its evidence: {:?}",
                    case.name,
                    finding.evidence
                );
            }
        }

        // --- the substance measurement ---
        let report = substance::measure(&files);
        let observed = ExpectSubstance::observed(&report);
        if observed != case.substance {
            mismatches.push(Mismatch {
                case: case.name.clone(),
                gate: "substance",
                expected: format!("{:?}", case.substance),
                observed: format!(
                    "{observed:?} ({} substantive of {} added: \
                     trivial {}, moved {}, repeated {}, generated {})",
                    report.substantive_lines,
                    report.added_lines,
                    report.trivial,
                    report.moved,
                    report.repeated,
                    report.generated
                ),
                // A diff measured as substantive when the corpus says it is
                // padding is the accepting direction here.
                false_accept: case.substance == ExpectSubstance::Low,
            });
        }

        // The classes partition the added lines. This is what makes the report
        // arithmetic a reader can check rather than a verdict they must take:
        // if the five counts do not sum, some line was counted twice or not at
        // all and every ratio built on them is wrong.
        assert_eq!(
            report.substantive_lines
                + report.trivial
                + report.moved
                + report.repeated
                + report.generated,
            report.added_lines,
            "case {:?}: the substance classes do not partition the added lines",
            case.name
        );
    }

    let false_accepts = mismatches.iter().filter(|m| m.false_accept).count();
    let false_positives = mismatches.len() - false_accepts;

    // Printed on every run, not only on failure. The number this file exists
    // to produce is "0 of N", and a measurement that is only visible when it
    // fails cannot be cited.
    println!(
        "\nfalse-accept corpus: {} cases, {known_positive} known-positive and \
         {known_negative} known-negative gate expectations\n\
         false accepts:   {false_accepts}\n\
         false positives: {false_positives}",
        cases.len()
    );

    if !mismatches.is_empty() {
        let detail: String = mismatches
            .iter()
            .map(|m| {
                format!(
                    "\n  [{}] {} / {}: expected {}, got {}",
                    if m.false_accept {
                        "FALSE ACCEPT"
                    } else {
                        "false positive"
                    },
                    m.case,
                    m.gate,
                    m.expected,
                    m.observed
                )
            })
            .collect();
        panic!(
            "{false_accepts} false accept(s) and {false_positives} false positive(s) \
             across {} cases:{detail}",
            cases.len()
        );
    }
}

/// The corpus must keep both kinds of case.
///
/// A corpus of only known-positive cases measures sensitivity and says nothing
/// about the false-positive rate that gets gates disabled; a corpus of only
/// clean cases is satisfied by a gate that never fires. Either drift makes the
/// matrix above meaningless while it still prints a zero, so the shape of the
/// corpus is asserted rather than assumed.
#[test]
fn the_corpus_measures_both_directions() {
    let cases = load_corpus();
    let positives = cases
        .iter()
        .filter(|c| c.secret_scan != Expect::Clean || c.stub_detection != Expect::Clean)
        .count();
    let negatives = cases
        .iter()
        .filter(|c| c.secret_scan == Expect::Clean && c.stub_detection == Expect::Clean)
        .count();
    assert!(
        positives >= 5,
        "only {positives} case(s) expect a finding; the corpus cannot measure misses"
    );
    assert!(
        negatives >= 5,
        "only {negatives} clean case(s); the corpus cannot measure false positives"
    );
    assert!(
        cases.iter().any(|c| c.substance == ExpectSubstance::Low),
        "no case exercises a low-substance diff"
    );
    assert!(
        cases
            .iter()
            .any(|c| c.substance == ExpectSubstance::Substantive),
        "no case exercises a substantive diff; a measurement that only ever \
         reports `low` would satisfy every other assertion here"
    );
}
