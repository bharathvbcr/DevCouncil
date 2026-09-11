//! The parse budget must bound the work it is named for.
//!
//! `DEFAULT_PARSE_BUDGET` is 5 s and every caller — `dev map build`, the
//! daemon, the MCP server — treats it as the guarantee that one hostile file
//! cannot stall the index. It was not a bound. Measured against the unmodified
//! extractor, a 140 KB Rust file of `"fn (((("` repeated took **174.69 s under a
//! 200 ms budget** — an 873x overrun. At the shipped 5 s budget the same shape
//! extrapolates to over an hour on a single file, holding the daemon's writer
//! lock for all of it.
//!
//! The cause was not tree-sitter. The parse finishes in 11 ms and polls its
//! cancellation callback 1400 times, never once needing to cancel. The cost was
//! entirely in this crate's own tree walks, which iterate children by index:
//!
//! ```ignore
//! for i in 0..node.child_count() {
//!     if let Some(child) = node.child(i) { .. }
//! }
//! ```
//!
//! `Node::child(i)` walks the sibling chain from the start, so the loop is
//! O(n^2) in the number of children. A degenerate parse gives the root 100,000
//! children, and 5x10^9 sibling steps is the 174 s. Walking the *same* tree with
//! a `TreeCursor` visits the same nodes in 2.5 ms — a 14,800x difference for
//! byte-identical output, because the work was never in the visiting.
//!
//! This test asserts the property that failed, not the implementation that
//! caused it: whatever the extractor does internally, it must come back inside
//! a small multiple of the budget it was handed.

use std::time::{Duration, Instant};

use devmap_extract::model::ParseOutcome;
use devmap_extract::treesitter::extract_treesitter_with_budget;

/// The budget under test. Small so the assertion is quick, and so a violation
/// is a ratio rather than a wall-clock guess.
const BUDGET: Duration = Duration::from_millis(200);

/// How far over budget a run may go before it is a broken bound.
///
/// Not 1x: the budget is checked at walk boundaries, not preemptively, so a
/// little overshoot is inherent and pinning 1x would make this test flaky
/// rather than meaningful. 10x is far below the 873x that was measured and far
/// above any legitimate check-granularity overshoot.
const TOLERANCE: u32 = 10;

/// The budget for the two cases that are *meant* to run to completion.
///
/// Their wall time is the cost of real work, so it scales with the build
/// profile — a debug build is an order of magnitude slower and would blow a
/// 200 ms budget on a perfectly legitimate file. Only the hostile case can
/// assert against `BUDGET` directly, because there the elapsed time is set by
/// the deadline the code enforces rather than by how fast the code is.
const COMPLETION_BUDGET: Duration = Duration::from_secs(60);

/// A file whose parse is fine and whose *tree* is pathological: one root with
/// tens of thousands of ERROR children.
fn degenerate_rust() -> String {
    "fn ((((".repeat(8_000)
}

#[test]
fn a_degenerate_tree_cannot_outrun_its_budget() {
    let source = degenerate_rust();

    let started = Instant::now();
    let extraction = extract_treesitter_with_budget("hostile.rs", "rust", &source, BUDGET);
    let elapsed = started.elapsed();

    assert!(
        elapsed < BUDGET * TOLERANCE,
        "a {BUDGET:?} budget must bound extraction of {} bytes; it took {elapsed:?} \
         ({:.0}x over). The budget is the only thing standing between one hostile file \
         and a stalled index.",
        source.len(),
        elapsed.as_secs_f64() / BUDGET.as_secs_f64()
    );

    // Whatever it decides, it must not claim a clean read of a file it did not
    // fully walk. Refusing is correct; `Clean` with a truncated symbol set is
    // the Class A failure.
    if matches!(extraction.parse_outcome, ParseOutcome::Clean) {
        assert!(
            extraction.symbols.len() > 1,
            "a `Clean` outcome claims the file was read; it carried only the File node"
        );
    }
}

/// The bound must not have been bought by refusing everything.
///
/// Without this, deleting the walk entirely would satisfy the test above.
#[test]
fn ordinary_source_is_untouched_by_the_bound() {
    let source = "fn add(a: i32, b: i32) -> i32 { a + b }\n\
                  pub struct Point { x: f64, y: f64 }\n\
                  impl Point { pub fn norm(&self) -> f64 { self.x.hypot(self.y) } }\n";

    let extraction = extract_treesitter_with_budget("fine.rs", "rust", source, COMPLETION_BUDGET);

    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "ordinary source must still parse cleanly: {:?}",
        extraction.parse_outcome
    );
    let names: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    for expected in ["add", "Point", "norm"] {
        assert!(
            names.contains(&expected),
            "the bound cost us a real symbol: {expected} missing from {names:?}"
        );
    }
}

/// A large but *well-formed* file must stay linear too.
///
/// The degenerate case above is driven by ERROR nodes. This one has none: it is
/// simply a big flat file, which is the ordinary shape of generated code and of
/// vendored bundles, and it exercises the same child-iteration paths on the
/// happy path where a regression would otherwise go unnoticed.
#[test]
fn a_large_well_formed_file_stays_within_budget() {
    let mut source = String::new();
    for i in 0..6_000 {
        source.push_str(&format!("fn f{i}(a: i32) -> i32 {{ a + {i} }}\n"));
    }

    let started = Instant::now();
    let extraction = extract_treesitter_with_budget("big.rs", "rust", &source, COMPLETION_BUDGET);
    let elapsed = started.elapsed();

    assert!(
        elapsed < COMPLETION_BUDGET,
        "a large well-formed file must finish its own work rather than being cut off by \
         the deadline; {} bytes took {elapsed:?}",
        source.len()
    );
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "a well-formed file must parse cleanly: {:?}",
        extraction.parse_outcome
    );
    assert!(
        extraction.symbols.len() > 6_000,
        "every function must be extracted, got {}",
        extraction.symbols.len()
    );
}

/// A refusal must name a stage where time could actually have been spent.
///
/// The checkpoint after the syntax-tree walk labelled itself *"deriving Go
/// method sets"* for every language, but the block it follows is gated on
/// `lang == "go"` and is a no-op for anything else. A Python file that ran out
/// of budget therefore reported:
///
/// ```text
/// extraction of 25 bytes exceeded the 5s budget for grammar python while
/// deriving Go method sets; no symbols are claimed for this file
/// ```
///
/// — naming a stage that did not run, for a language that has no method sets.
/// The refusal itself was right and the accounting was not, which is the worse
/// half: a maintainer reading that line goes looking for a Go pass that never
/// executed, and the walk tail where the time actually went stays invisible.
///
/// A zero budget reaches this checkpoint rather than the parse one: the source
/// is small enough that tree-sitter never polls its cancellation callback and
/// the walk finishes inside one `DEADLINE_CHECK_STRIDE`, so the first check
/// that can observe the expired deadline is this one.
#[test]
fn a_refusal_names_a_stage_the_language_actually_ran() {
    for (path, lang) in [
        ("mod.py", "python"),
        ("mod.rs", "rust"),
        ("mod.ts", "typescript"),
    ] {
        let extraction =
            extract_treesitter_with_budget(path, lang, "def f():\n    return 1\n", Duration::ZERO);
        let ParseOutcome::Failed { reason } = &extraction.parse_outcome else {
            panic!(
                "a zero budget must refuse {path}: {:?}",
                extraction.parse_outcome
            );
        };
        assert!(
            !reason.contains("Go method sets"),
            "{lang} has no Go method sets and spent no time deriving them; a refusal \
             that names that stage sends a maintainer after a pass which never ran. \
             Got: {reason:?}"
        );
        assert!(
            reason.contains("budget"),
            "the refusal must still say the budget was what stopped it: {reason:?}"
        );
    }
}

/// The Go label is still correct for Go, which is the half worth keeping.
///
/// Without this, relabelling the checkpoint unconditionally would pass the test
/// above while deleting the one accurate attribution it had: on a Go file the
/// method-set derivation really does run between the walk and this check, and
/// it is a real place for the budget to go.
#[test]
fn a_go_refusal_at_that_checkpoint_still_names_the_go_pass() {
    let extraction = extract_treesitter_with_budget(
        "mod.go",
        "go",
        "package main\n\nfunc f() int { return 1 }\n",
        Duration::ZERO,
    );
    let ParseOutcome::Failed { reason } = &extraction.parse_outcome else {
        panic!(
            "a zero budget must refuse mod.go: {:?}",
            extraction.parse_outcome
        );
    };
    assert!(
        reason.contains("Go method sets"),
        "for Go the method-set pass does run at this checkpoint and is a real place \
         for the budget to have gone: {reason:?}"
    );
}
