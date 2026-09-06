//! Call extraction for shell, SQL and Solidity (SC34).
//!
//! All three already reached the symbol table through `extract_node`'s generic
//! arm and stopped there. Measured with `extract_file` immediately before
//! `langcalls::{shell, sql, solidity}` existed:
//!
//! | fixture | symbols | calls |
//! |---|---|---|
//! | `run.sh` (`run` runs `helper` twice) | 3 | **0** |
//! | `schema.sql` (`total` calls `add_one` twice, a view selects `total`) | 4 | **0** |
//! | `Vault.sol` (`deposit` calls `_add` and `require`) | 4 | **0** |
//!
//! So each of them had nodes and no edges: a shell function with no caller and
//! no callee is indistinguishable from a script that does nothing, and a
//! Solidity contract whose functions never call each other is the one thing a
//! Solidity contract is never like. Every test below was run against that tree
//! first and its pre-fix answer is recorded beside the assertion.

use devmap_extract::extract_file;
use devmap_extract::languages::{capabilities_for_language, Capability};
use devmap_extract::model::{Extraction, SymbolKind};

const SHELL: &str = r#"#!/usr/bin/env bash
set -euo pipefail

helper() {
  echo "$1"
}

run() {
  local out
  out="$(helper hello)"
  helper "$out"
  printf '%s\n' "$out"
}

run
"#;

/// Every enclosing construct the grammar wraps a command in, plus the command
/// names that are not identities.
const SHELL_SHAPES: &str = r#"helper() { :; }
run() {
  helper plain
  out=$(helper substitution)
  helper pipeline | grep x
  if helper condition; then helper consequent; fi
  while helper loop; do helper body; done
  case $1 in a) helper branch;; esac
  ( helper subshell )
  ! helper negated
  ./script.sh
  /usr/bin/env python
  cmd=helper
  "$cmd" indirect
  [ -f x ]
}
"#;

const SQL: &str = r#"CREATE FUNCTION add_one(n integer) RETURNS integer AS $$
  SELECT n + 1;
$$ LANGUAGE sql;

CREATE FUNCTION total(n integer) RETURNS integer AS $$
  SELECT add_one(n) + public.add_one(n);
$$ LANGUAGE sql;

CREATE VIEW report AS SELECT total(1) AS t FROM events;

CREATE TRIGGER trg BEFORE INSERT ON events FOR EACH ROW EXECUTE FUNCTION total();
"#;

const SOLIDITY: &str = r#"contract Base { function baseCall() public {} }

library Math {
    function sq(uint256 a) internal pure returns (uint256) { return a * a; }
}

interface IThing { function ping() external; }

contract Vault is Base {
    uint256 total;
    event Moved(uint256 a);
    error Bad(uint256 a);

    modifier onlyPos(uint256 a) { require(a > 0); _; }

    function _add(uint256 a) internal returns (uint256) { return a; }

    function deposit(uint256 a) public onlyPos(a) {
        total = _add(a);
        total = Math.sq(a);
        super.baseCall();
        this.deposit(a);
        uint256 c = uint256(a);
        emit Moved(c);
        revert Bad(a);
        new Vault();
    }
}
"#;

fn targets(extraction: &Extraction) -> Vec<String> {
    let mut out: Vec<String> = extraction
        .calls
        .iter()
        .map(|call| match &call.receiver_expr {
            Some(receiver) => format!("{receiver}::{}", call.callee_name),
            None => call.callee_name.clone(),
        })
        .collect();
    out.sort();
    out
}

fn edges(extraction: &Extraction) -> Vec<String> {
    let mut out: Vec<String> = extraction
        .calls
        .iter()
        .map(|call| {
            format!(
                "{} -> {}",
                call.caller_symbol.as_deref().unwrap_or("<file>"),
                call.callee_name
            )
        })
        .collect();
    out.sort();
    out
}

fn qualified_names(extraction: &Extraction) -> Vec<String> {
    let mut names: Vec<String> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect();
    names.sort();
    names
}

/// Call and reference sources naming no emitted symbol — the SC9/SC10 orphan.
///
/// `None` is deliberately not one. A shell script's top-level `run` really is
/// called by the file, and a bare `SELECT add_one(3);` really does sit at file
/// scope; recording those as file-sourced is the honest answer, and it is what
/// every other module in this directory already does.
fn orphaned_sources(extraction: &Extraction) -> Vec<String> {
    let emitted = qualified_names(extraction);
    let mut orphans: Vec<String> = extraction
        .calls
        .iter()
        .filter_map(|call| call.caller_symbol.clone())
        .chain(
            extraction
                .references
                .iter()
                .filter_map(|reference| reference.enclosing_symbol.clone()),
        )
        .filter(|source| !emitted.contains(source))
        .collect();
    orphans.sort();
    orphans.dedup();
    orphans
}

/// Pre-fix: `calls == []` on this exact fixture, with all three symbols already
/// present — nodes and no edges.
#[test]
fn shell_recovers_the_call_graph_that_was_empty() {
    let extraction = extract_file("run.sh", SHELL);
    assert_eq!(
        edges(&extraction),
        vec![
            "<file> -> run".to_string(),
            "<file> -> set".to_string(),
            "run.sh::helper -> echo".to_string(),
            "run.sh::run -> helper".to_string(),
            "run.sh::run -> helper".to_string(),
            "run.sh::run -> printf".to_string(),
        ],
        "a shell script's top-level commands are called by the file, and the \
         two calls inside `run` must name `run`"
    );
}

/// One arm covers every construct a command can sit in, and the callee gate
/// refuses every command name that is not an identity.
#[test]
fn shell_records_a_command_in_every_construct_and_refuses_a_non_identity() {
    let extraction = extract_file("shapes.sh", SHELL_SHAPES);
    let helper_calls = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "helper")
        .count();
    assert_eq!(
        helper_calls,
        10,
        "one for each of plain, substitution, pipeline, if-condition, \
         if-consequent, while-condition, while-body, case-branch, subshell and \
         negated — {:?}",
        targets(&extraction)
    );
    for refused in ["./script.sh", "/usr/bin/env", "\"$cmd\"", "[", "cmd=helper"] {
        assert!(
            !targets(&extraction).contains(&refused.to_string()),
            "{refused:?} is not a callee identity and must not be recorded: {:?}",
            targets(&extraction)
        );
    }
}

/// Pre-fix: `calls == []`. The two `add_one` calls live **inside** a
/// dollar-quoted function body, which the grammar parses rather than treating
/// as one string token — the fact that makes a SQL call graph recoverable at
/// all.
#[test]
fn sql_recovers_calls_from_inside_a_dollar_quoted_body() {
    let extraction = extract_file("schema.sql", SQL);
    assert!(
        edges(&extraction).contains(&"schema.sql::total -> add_one".to_string()),
        "{:?}",
        edges(&extraction)
    );
    assert!(
        targets(&extraction).contains(&"public::add_one".to_string()),
        "a schema qualifier is the receiver, not part of the callee name: {:?}",
        targets(&extraction)
    );
    assert!(
        edges(&extraction).contains(&"schema.sql::report -> total".to_string()),
        "a view selecting a function calls it: {:?}",
        edges(&extraction)
    );
}

/// `EXECUTE FUNCTION total()` contains no call node — the routine is a bare
/// `object_reference`, the same kind as the trigger's own name and the table it
/// fires on, told apart only by the keyword before it.
///
/// Without this edge a trigger function has no caller anywhere in any schema
/// and is a dead-code candidate on no evidence.
#[test]
fn sql_records_the_routine_a_trigger_executes() {
    let extraction = extract_file("schema.sql", SQL);
    assert!(
        edges(&extraction).contains(&"schema.sql::trg -> total".to_string()),
        "{:?}",
        edges(&extraction)
    );
    // The trigger's own name and the table it fires on are the same node kind
    // and must not be read as callees.
    assert!(
        !targets(&extraction).contains(&"trg".to_string())
            && !targets(&extraction).contains(&"events".to_string()),
        "only the routine after EXECUTE FUNCTION is invoked: {:?}",
        targets(&extraction)
    );
}

/// Pre-fix: `calls == []` with all four symbols present.
#[test]
fn solidity_recovers_the_call_graph_that_was_empty() {
    let extraction = extract_file("Vault.sol", SOLIDITY);
    assert!(
        edges(&extraction).contains(&"Vault.sol::Vault.deposit -> _add".to_string()),
        "{:?}",
        edges(&extraction)
    );
    assert!(
        targets(&extraction).contains(&"Math::sq".to_string()),
        "a library call carries the library as its receiver: {:?}",
        targets(&extraction)
    );
    assert!(
        targets(&extraction).contains(&"this::deposit".to_string()),
        "`this.` is kept so an external self-call stays distinguishable from a \
         bare internal one: {:?}",
        targets(&extraction)
    );
}

/// A modifier is a callable body wrapped around a function, and it is where a
/// contract's access control lives.
///
/// Pre-fix `modifier_definition` was in no declaration table, so `onlyPos` was
/// not a node at all and the invocation had nothing to name.
#[test]
fn solidity_records_a_modifier_as_a_symbol_and_its_invocation_as_a_call() {
    let extraction = extract_file("Vault.sol", SOLIDITY);
    assert_eq!(
        extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "Vault.sol::Vault.onlyPos")
            .map(|symbol| symbol.kind),
        Some(SymbolKind::Method),
        "the modifier must be a symbol before its invocation can join to it: {:?}",
        qualified_names(&extraction)
    );
    assert!(
        edges(&extraction).contains(&"Vault.sol::Vault.deposit -> onlyPos".to_string()),
        "{:?}",
        edges(&extraction)
    );
    assert!(
        edges(&extraction).contains(&"Vault.sol::Vault.onlyPos -> require".to_string()),
        "a call in a modifier body belongs to the modifier: {:?}",
        edges(&extraction)
    );
}

/// The four Solidity shapes that look like calls and are not, plus the one that
/// looks like a type and is.
#[test]
fn solidity_refuses_the_shapes_that_only_look_like_calls() {
    let extraction = extract_file("Vault.sol", SOLIDITY);
    assert!(
        !targets(&extraction).contains(&"baseCall".to_string())
            && !targets(&extraction).contains(&"super::baseCall".to_string()),
        "`super.` names the next contract in the linearisation, which the call \
         site never identifies; recording it would bind to this contract's own \
         override: {:?}",
        targets(&extraction)
    );
    for not_a_call in ["Moved", "Bad", "uint256"] {
        assert!(
            !targets(&extraction).contains(&not_a_call.to_string()),
            "{not_a_call} is an event, an error or a cast — none has a body a \
             call edge could reach: {:?}",
            targets(&extraction)
        );
    }
    assert!(
        targets(&extraction).contains(&"Vault".to_string()),
        "`new Vault()` constructs a contract that is a symbol: {:?}",
        targets(&extraction)
    );
}

/// No call and no reference may name a source the emitter did not produce.
#[test]
fn no_shell_sql_or_solidity_edge_is_orphaned() {
    let mut attributed = 0;
    for (path, source) in [
        ("run.sh", SHELL),
        ("shapes.sh", SHELL_SHAPES),
        ("schema.sql", SQL),
        ("Vault.sol", SOLIDITY),
    ] {
        let extraction = extract_file(path, source);
        assert_eq!(
            orphaned_sources(&extraction),
            Vec::<String>::new(),
            "{path} attributed an edge to a symbol that was never emitted"
        );
        attributed += extraction
            .calls
            .iter()
            .filter(|call| {
                call.caller_symbol
                    .as_deref()
                    .is_some_and(|caller| caller.contains("::"))
            })
            .count();
    }
    assert!(
        attributed >= 20,
        "only {attributed} calls were attributed to a symbol rather than to the \
         file; the orphan check would be holding vacuously"
    );
}

/// The coverage list must never claim a language the dispatcher does not route,
/// and must never omit one it does.
#[test]
fn the_coverage_list_and_the_dispatcher_agree_for_shell_sql_and_solidity() {
    for (language, path, source) in [
        ("shell", "w.sh", "m() { helper 1; }\n"),
        (
            "sql",
            "w.sql",
            "CREATE VIEW v AS SELECT helper(1) AS x FROM t;\n",
        ),
        (
            "solidity",
            "w.sol",
            "contract C { function m() public { helper(1); } }\n",
        ),
    ] {
        let listed = capabilities_for_language(language).contains(Capability::Calls);
        let extracted = !extract_file(path, source).calls.is_empty();
        assert_eq!(
            listed, extracted,
            "{language}: coverage list says {listed}, extraction says {extracted}"
        );
    }
}

/// Extraction is a pure function of the source: repeated runs must be
/// byte-identical, including order.
#[test]
fn shell_sql_and_solidity_extraction_is_deterministic() {
    for (path, source) in [
        ("shapes.sh", SHELL_SHAPES),
        ("schema.sql", SQL),
        ("Vault.sol", SOLIDITY),
    ] {
        let first = format!("{:?}", extract_file(path, source).calls);
        for _ in 0..3 {
            assert_eq!(
                first,
                format!("{:?}", extract_file(path, source).calls),
                "{path} call order is not stable"
            );
        }
    }
}

/// Unparseable input must not panic and must not orphan an edge.
#[test]
fn broken_shell_sql_and_solidity_yield_no_orphans() {
    for (path, source) in [
        ("bad.sh", "run() { if while do done fi\n"),
        ("bad.sql", "CREATE FUNCTION ((( AS $$ $$;\n"),
        ("bad.sol", "contract { function ((( }\n"),
    ] {
        let extraction = extract_file(path, source);
        assert_eq!(
            orphaned_sources(&extraction),
            Vec::<String>::new(),
            "{path} orphaned an edge while recovering from a parse error"
        );
    }
}
