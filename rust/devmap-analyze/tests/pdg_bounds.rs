//! Q-11: the PDG builder walks attacker-shaped input with no ceiling.
//!
//! `FunctionPdgInput` derives `Deserialize`, so the statement tree is untrusted
//! by construction. `validate_statements` and `PdgBuilder::build_sequence` both
//! recurse once per nesting level with no depth parameter, and the reaching
//! definition and taint fixpoints in `add_data_edges` loop with no ceiling —
//! the same shape the type walkers in `devmap-extract` were hardened against
//! after one of them aborted a build with a stack overflow.
//!
//! The module has no caller today, which is exactly why the bounds go in now:
//! the failure mode of an unbounded recursion is `SIGABRT`, not an error a
//! caller can handle, and it arrives the day someone wires the module up.
//!
//! The refusals are asserted in both directions. A builder that refuses
//! everything is not a hardened builder, so every cap below is paired with an
//! input one level under it that must still build.

use devmap_analyze::pdg::{
    build_function_pdg, FunctionPdgInput, PdgStatement, PdgStatementKind, MAX_PDG_NESTING_DEPTH,
    MAX_PDG_STATEMENTS,
};

fn basic(line: u32) -> PdgStatement {
    PdgStatement {
        line,
        definitions: Vec::new(),
        uses: Vec::new(),
        taint_sinks: Vec::new(),
        kind: PdgStatementKind::Basic,
    }
}

/// `depth` nested loops around one basic statement, built iteratively so the
/// *test* never recurses.
fn nested(depth: usize) -> PdgStatement {
    let mut statement = basic(1);
    for _ in 0..depth {
        statement = PdgStatement {
            line: 1,
            definitions: Vec::new(),
            uses: Vec::new(),
            taint_sinks: Vec::new(),
            kind: PdgStatementKind::Loop {
                body: vec![statement],
            },
        };
    }
    statement
}

fn input(body: Vec<PdgStatement>) -> FunctionPdgInput {
    FunctionPdgInput {
        function_name: "f".to_string(),
        generation_id: 1,
        content_hash: 7,
        start_line: 1,
        end_line: 1,
        params: Vec::new(),
        body,
    }
}

/// The abort. 100,000 nesting levels is a 2 MB payload, far under anything a
/// transport would reject, and both walkers descend it one stack frame at a
/// time.
#[test]
fn absurdly_nested_input_is_refused_rather_than_overflowing_the_stack() {
    let deep = input(vec![nested(100_000)]);
    let result = build_function_pdg(&deep);

    assert!(
        result.is_err(),
        "a 100,000-deep statement tree must be refused; recursing it aborts the \
         process, and an abort is not an answer a caller can handle"
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("nesting") || message.contains("depth"),
        "the refusal must name what was exceeded, got {message:?}"
    );
    // Dropping a 100,000-deep tree recurses too — that is the compiler's
    // generated `Drop`, not this module's code, and it is not what is under
    // test here. Leaking it keeps the test measuring the builder.
    std::mem::forget(deep);
}

/// Exactly at the cap still builds. Without this, a builder that refused
/// everything would pass the test above.
#[test]
fn nesting_at_the_cap_still_builds() {
    let at_cap = input(vec![nested(MAX_PDG_NESTING_DEPTH)]);
    let pdg = build_function_pdg(&at_cap).expect("input at the documented cap must build");
    assert_eq!(pdg.function_name, "f");
    // Every nesting level is a node, plus the innermost statement, plus entry
    // and exit — the whole tree was walked, not stopped part way down.
    assert_eq!(pdg.nodes.len(), MAX_PDG_NESTING_DEPTH + 3);
}

/// One level over the cap is refused, so the boundary is where it is
/// documented rather than wherever the stack happens to run out.
#[test]
fn nesting_one_level_over_the_cap_is_refused() {
    let over = input(vec![nested(MAX_PDG_NESTING_DEPTH + 1)]);
    assert!(
        build_function_pdg(&over).is_err(),
        "the cap is {MAX_PDG_NESTING_DEPTH} levels of nesting"
    );
}

/// Breadth is a ceiling too: every statement becomes a CFG node, and both
/// fixpoints sweep every node on every pass, so an unbounded statement list is
/// an unbounded amount of work with no cancellation to stop it.
#[test]
fn an_oversized_statement_list_is_refused_rather_than_swept_forever() {
    let wide = input((0..MAX_PDG_STATEMENTS + 1).map(|_| basic(1)).collect());
    let result = build_function_pdg(&wide);

    assert!(
        result.is_err(),
        "{} statements in one function must be refused, not swept",
        MAX_PDG_STATEMENTS + 1
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("statement"),
        "the refusal must name what was exceeded, got {message:?}"
    );
}

/// A statement list at the cap still builds, and the fixpoints still converge
/// on it — the ceiling is a safety net over a bound, not a truncation of a
/// real computation.
#[test]
fn a_statement_list_at_the_cap_still_builds() {
    let wide = input((0..MAX_PDG_STATEMENTS).map(|_| basic(1)).collect());
    let pdg = build_function_pdg(&wide).expect("a function at the statement cap must build");
    // Entry, exit, and one node per statement.
    assert_eq!(pdg.nodes.len(), MAX_PDG_STATEMENTS + 2);
}

/// The whole point of the module still works: a small, ordinary function
/// builds the control and data edges it always did.
#[test]
fn an_ordinary_function_still_builds_its_control_and_data_edges() {
    let assign = PdgStatement {
        line: 1,
        definitions: vec!["x".to_string()],
        uses: vec!["p".to_string()],
        taint_sinks: Vec::new(),
        kind: PdgStatementKind::Basic,
    };
    let sink = PdgStatement {
        line: 1,
        definitions: Vec::new(),
        uses: vec!["x".to_string()],
        taint_sinks: vec!["x".to_string()],
        kind: PdgStatementKind::Basic,
    };
    let mut ordinary = input(vec![assign, sink]);
    ordinary.params = vec!["p".to_string()];

    let pdg = build_function_pdg(&ordinary).expect("an ordinary function builds");
    assert!(
        pdg.edges.iter().any(|edge| edge.edge_kind == "taint"),
        "a parameter reaching a sink is still a taint edge: {:?}",
        pdg.edges
    );
    assert!(
        pdg.edges.iter().any(|edge| edge.edge_kind == "param"),
        "a parameter use is still a param edge: {:?}",
        pdg.edges
    );
    assert!(pdg
        .edges
        .iter()
        .any(|edge| edge.edge_kind.starts_with("control:")));
}
