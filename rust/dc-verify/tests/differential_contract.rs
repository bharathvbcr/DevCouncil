//! Differential testing, expressed as an acceptance contract.
//!
//! reverify's strongest grounding is `behavior_equiv`: run a reference and a
//! candidate implementation over shared inputs and return a counterexample when
//! they disagree. It is the right gate for a refactor — "the old and new code
//! agree" is far stronger evidence than "the new code is covered" — and it is
//! the one borrow from that comparison DevCouncil should NOT implement inside
//! `dcverify`.
//!
//! The reason is the analysis plane's central boundary. dc-evidence "does not
//! control a desktop, call a model, execute commands, read a database or access
//! files"; `dcverify` reads a diff on stdin and computes over text. Running a
//! candidate implementation means executing arbitrary repository code, and the
//! sandbox that would have to contain it does not exist here — the README says
//! so plainly: the sandbox selector does not implement Docker/Nix isolation.
//! Adding an exec path to the verifier to reach one gate would put the program
//! that decides whether a change is safe in the business of running that
//! change's code.
//!
//! It does not need one. A differential result is an *observation*, and the
//! acceptance evidence protocol already evaluates observations against a
//! contract pinned before execution. The host — Manvi, or CI — runs the
//! comparison and records what it saw; `dcverify evidence-check` decides whether
//! that satisfies the contract. Ownership stays where the protocol already put
//! it: "Manvi owns execution and observation provenance."
//!
//! This file is the proof of that claim rather than the claim itself. Both
//! fixtures are real, both run through the real binary, and the second is the
//! half that matters — a contract that cannot fail is not a gate.
//!
//! One property is worth naming, because it is where DevCouncil is *ahead* of
//! the tool this was borrowed from: the contract is admitted independently and
//! before the run, so the thing under test cannot choose which facts get
//! checked. reverify grounds every claim a model makes and is silent on whether
//! the model claimed the right things.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const CONTRACT: &[u8] = include_bytes!("../../dc-evidence/fixtures/v1/differential-contract.json");
const AGREED: &[u8] = include_bytes!("../../dc-evidence/fixtures/v1/differential-bundle.json");
const DISAGREED: &[u8] =
    include_bytes!("../../dc-evidence/fixtures/v1/differential-bundle-mismatch.json");
const REPORT: &[u8] = include_bytes!("../../dc-evidence/fixtures/v1/differential-report.txt");

/// The contract's own digest, which the host pins before the run.
///
/// A literal rather than a hash computed here from the same bytes. Hashing the
/// file at test time would make the assertion tautological — any edit to the
/// contract would silently move both sides — and this value is exactly what an
/// admission coordinator has to hold independently for the check to mean
/// anything.
const CONTRACT_SHA256: &str = "12d85fa1ac1ef6ee2a5fd834858a56e3a81da163eeefc025c9d102d1254b49a5";
const CAPABILITY_SHA256: &str = "1a4bf7dac3b78318718a5047937473da4898acfb53cbbc9afdbe0c7fa9f3eec2";

/// Writes the fixture set into a private directory and returns its path.
///
/// Per-process and per-call, because the artifact is resolved against
/// `--artifacts-root` and two tests sharing one root would race on it.
fn fixture(bundle: &[u8]) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "dc-differential-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("differential-contract.json"), CONTRACT).unwrap();
    std::fs::write(root.join("bundle.json"), bundle).unwrap();
    std::fs::write(root.join("differential-report.txt"), REPORT).unwrap();
    root
}

fn check(root: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .arg("evidence-check")
        .arg("--contract")
        .arg(root.join("differential-contract.json"))
        .arg("--bundle")
        .arg(root.join("bundle.json"))
        .arg("--artifacts-root")
        .arg(root)
        .args([
            "--expected-contract-sha256",
            CONTRACT_SHA256,
            "--expected-capability-sha256",
            CAPABILITY_SHA256,
            "--expected-run-id",
            "run-diff-1",
            "--expected-session-id",
            "ci-1",
            "--expected-epoch",
            "1",
        ])
        .output()
        .expect("run dcverify");
    assert!(
        output.status.success(),
        "evidence-check exited {:?}; a contract that was evaluated exits 0 whatever \
         its verdict, so a nonzero status here means the evaluation itself failed. \
         stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 reply")
}

#[test]
fn a_refactor_whose_implementations_agree_satisfies_the_contract() {
    let reply = check(&fixture(AGREED));
    assert!(
        reply.contains(r#""verdict":"passed""#),
        "a run with 2000 cases and no mismatch must pass: {reply}"
    );
    // Every criterion, not just the verdict: a contract whose other three
    // clauses were quietly incomplete would still report `passed` on this
    // bundle, and the pass would mean less than it appears to.
    for criterion in [
        "no-mismatch",
        "enough-cases",
        "compared-against-the-pinned-reference",
        "the-run-succeeded",
    ] {
        assert!(
            reply.contains(criterion),
            "the reply does not name criterion {criterion}: {reply}"
        );
    }
    assert!(
        !reply.contains(r#""verdict":"failed""#),
        "no criterion may fail on the agreeing bundle: {reply}"
    );
}

#[test]
fn a_refactor_that_changed_behaviour_fails_it() {
    let reply = check(&fixture(DISAGREED));
    assert!(
        reply.contains(r#""verdict":"failed""#),
        "three mismatches must fail the contract: {reply}"
    );
    // `ok` stays true and the process exits 0. That is the protocol's own
    // distinction and it is the reason this gate is trustworthy: `ok` says the
    // evaluation ran, `verdict` says what it decided, and a consumer that read
    // the exit code alone would call this run a success.
    assert!(
        reply.contains(r#""ok":true"#),
        "a contract that was evaluated and failed still evaluated: {reply}"
    );
}

/// The discriminating check.
///
/// Without it, both tests above are satisfied by an evaluator that ignores the
/// facts and answers from `outcome.kind` alone — the agreeing bundle says
/// `success` and the disagreeing one says `business`. Asserting that the
/// *mismatch count itself* is what failed is what proves the differential
/// result is being read.
#[test]
fn the_mismatch_count_is_what_fails_and_the_unrelated_clauses_still_pass() {
    let reply = check(&fixture(DISAGREED));
    let no_mismatch = reply
        .split(r#"{"id":"no-mismatch""#)
        .nth(1)
        .expect("the reply names the no-mismatch criterion");
    assert!(
        no_mismatch.starts_with(r#","required":true,"verdict":"failed""#),
        "no-mismatch must be the criterion that failed: {reply}"
    );
    let enough_cases = reply
        .split(r#"{"id":"enough-cases""#)
        .nth(1)
        .expect("the reply names the enough-cases criterion");
    assert!(
        enough_cases.starts_with(r#","required":true,"verdict":"passed""#),
        "the case count is unaffected by a mismatch and must still pass: {reply}"
    );
}
