//! Characterization of the per-file parse budget through the API that existed
//! before the serial retry pass: a file refused for budget is a plain
//! `ParseOutcome::Failed`, publishes no symbols, and is counted as a parse
//! failure. Written against the unmodified code and kept as the baseline the
//! retry pass changes only for the parallel batch entry points.

use devmap_extract::treesitter::extract_treesitter_with_budget;
use devmap_extract::ParseOutcome;
use std::time::Duration;

const SOURCE: &str = "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n";

#[test]
fn a_zero_budget_refuses_a_file_that_parses_with_room() {
    let refused = extract_treesitter_with_budget("m.py", "python", SOURCE, Duration::ZERO);
    assert!(refused.is_parse_failure());
    match &refused.parse_outcome {
        ParseOutcome::Failed { reason } => assert!(reason.contains("budget"), "{reason}"),
        other => panic!("expected a budget refusal, got {other:?}"),
    }
    assert!(
        refused.symbols.iter().all(|s| s.name != "alpha"),
        "a refused file publishes no declarations"
    );

    let parsed = extract_treesitter_with_budget("m.py", "python", SOURCE, Duration::from_secs(60));
    assert!(!parsed.is_parse_failure());
    assert!(parsed.symbols.iter().any(|s| s.name == "alpha"));
}
