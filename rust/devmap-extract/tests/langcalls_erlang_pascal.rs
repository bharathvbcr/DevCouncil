//! Call and declaration extraction for Erlang and Pascal (SC34).
//!
//! These two are the deepest holes the grammar-linkage table had. Both reached
//! `extract_node`'s generic arm, and for both that arm recovered **nothing**:
//! measured with `extract_file` immediately before these modules existed,
//!
//! | fixture | symbols | calls | references |
//! |---|---|---|---|
//! | `worker.erl` (`run` calls `helper` and `lists:sum`) | 1 (`File`) | 0 | 0 |
//! | `worker.pas` (`Run` calls `Helper` and `WriteLn`) | 1 (`File`) | 0 | 8, all attributed to the file |
//!
//! so neither language had a *declaration* to attribute a call to, let alone a
//! call. Every test below was run against that tree first and its pre-fix
//! answer is recorded beside the assertion, so a regression is recognisable
//! rather than merely red.

use devmap_extract::extract_file;
use devmap_extract::languages::{capabilities_for_language, Capability};
use devmap_extract::model::{Extraction, SymbolKind};

const ERLANG: &str = r#"-module(worker).
-export([run/1]).

helper(X) ->
    X + 1.

run(N) ->
    Y = helper(N),
    lists:sum([Y]).
"#;

/// Every Erlang shape that is or is not a call, in one module.
const ERLANG_SHAPES: &str = r#"-module(deep).
-record(state, {a :: integer(), b = f(1) :: list()}).
-spec other(integer()) -> integer().
-type opt() :: atom().
-opaque handle() :: reference().
-callback handle(atom()) -> ok.

f(X) -> X.

other(X) ->
    Ref = fun f/1,
    Remote = fun lists:map/2,
    Same = ?MODULE:f(1),
    Dynamic = Ref(2),
    lists:sum([X]),
    deep:f(3).
"#;

const PASCAL: &str = r#"unit Worker;

interface

function Helper(A: Integer): Integer;
procedure Run;

implementation

function Helper(A: Integer): Integer;
begin
  Result := A + 1;
end;

procedure Run;
var
  X: Integer;
begin
  X := Helper(2);
  WriteLn(X);
end;

end.
"#;

/// A unit with a class, so the owner half of a Pascal identity is exercised.
const PASCAL_CLASS: &str = r#"unit Deep;

interface

type
  TWorker = class
    procedure Go(A: Integer);
    function Calc(A: Integer): Integer;
  end;

function Helper(A: Integer): Integer;

implementation

function Helper(A: Integer): Integer;
begin
  Result := A + 1;
end;

function TWorker.Calc(A: Integer): Integer;
begin
  Result := Helper(A);
end;

procedure TWorker.Go(A: Integer);
var
  W: TWorker;
  X: Integer;
begin
  X := Calc(A);
  X := Self.Calc(A);
  X := W.Calc(A);
  A.B.Calc(1);
  Cleanup;
  inherited;
end;

end.
"#;

/// `receiver::callee` when there is a receiver, else the bare callee. Sorted,
/// so a test pins a set rather than a traversal order.
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

/// `caller -> callee`, sorted. `<file>` stands for a call at file scope, which
/// is a real place for a call to be in both languages.
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

/// Call and reference sources that name no emitted symbol — the SC9/SC10
/// orphan. `None` is not one: it means the file is the caller, which is what a
/// top-level Erlang `-define` body or a Pascal program block genuinely is.
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

fn duplicate_names(extraction: &Extraction) -> Vec<String> {
    let names = qualified_names(extraction);
    let mut duplicates: Vec<String> = names
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0].clone())
        .collect();
    duplicates.dedup();
    duplicates
}

/// Pre-fix: `symbols == ["worker.erl"]`, `calls == []`.
#[test]
fn erlang_recovers_the_declarations_and_the_calls_that_were_both_absent() {
    let extraction = extract_file("worker.erl", ERLANG);
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "worker.erl".to_string(),
            "worker.erl::helper".to_string(),
            "worker.erl::run".to_string(),
        ],
        "the two functions must be symbols before any edge can name them"
    );
    assert_eq!(
        edges(&extraction),
        vec![
            "worker.erl::run -> helper".to_string(),
            "worker.erl::run -> sum".to_string(),
        ]
    );
}

/// A type annotation is spelled with call syntax in this grammar, and there are
/// more of them in real Erlang than there are calls.
///
/// Pre-fix this test could not run at all (no calls existed). Written against
/// the parse tree: `-spec`, `-callback`, `-type`, `-opaque` and a record
/// field's `:: list()` each contain a node of kind `call`.
#[test]
fn erlang_type_annotations_do_not_become_call_edges() {
    let extraction = extract_file("deep.erl", ERLANG_SHAPES);
    for annotation in ["integer", "atom", "reference", "list", "ok"] {
        assert!(
            !targets(&extraction).contains(&annotation.to_string()),
            "{annotation}() is a type, not a call: {:?}",
            targets(&extraction)
        );
    }
    // The counterweight: a record field's *default value* sits in the same
    // `record_decl` as a type annotation and is a real call. Excluding the
    // whole form would drop it.
    assert!(
        edges(&extraction).contains(&"deep.erl::state -> f".to_string()),
        "a record field default is code: {:?}",
        edges(&extraction)
    );
}

/// The module qualifier, the self-module shorthand, and the two `fun` reference
/// forms — each verified against the grammar's own tree.
#[test]
fn erlang_records_every_way_a_function_is_named() {
    let extraction = extract_file("deep.erl", ERLANG_SHAPES);
    assert_eq!(
        targets(&extraction),
        vec![
            // `deep:f(3)` — an explicit module qualifier is the receiver.
            "deep::f".to_string(),
            // Three bare `f`, in source order: the record field default
            // `b = f(1)`, the reference `fun f/1`, and `?MODULE:f(1)` — whose
            // receiver is dropped so the edge resolves to this file's own `f`,
            // which is what the preprocessor makes it.
            "f".to_string(),
            "f".to_string(),
            "f".to_string(),
            // `fun lists:map/2` — a remote reference keeps its module.
            "lists::map".to_string(),
            // `lists:sum([X])`
            "lists::sum".to_string(),
        ],
        "`Dynamic = Ref(2)` must contribute nothing: a call through a variable \
         names no function in the source"
    );
}

/// `generic_symbol_kind` maps the node kind `module` to `SymbolKind::Module`,
/// and in `tree-sitter-erlang` `module` is the *module half of a function
/// reference*, not a declaration.
///
/// Pre-fix, on this exact source: `qualified_names` contained `t.erl::t`, a
/// `Module` symbol for a module the file does not declare.
#[test]
fn erlang_a_function_reference_does_not_declare_its_module() {
    let extraction = extract_file("t.erl", "-module(t).\ng() -> F = fun other:f/2, F.\n");
    assert!(
        !qualified_names(&extraction).contains(&"t.erl::other".to_string()),
        "`fun other:f/2` names a module, it does not declare one: {:?}",
        qualified_names(&extraction)
    );
    assert!(
        !extraction
            .symbols
            .iter()
            .any(|symbol| symbol.kind == SymbolKind::Module),
        "no Erlang declaration in this file is a module: {:?}",
        qualified_names(&extraction)
    );
}

/// Pre-fix: `symbols == ["worker.pas"]`, `calls == []`, and all eight
/// references attributed to the file.
#[test]
fn pascal_recovers_the_declarations_and_the_calls_that_were_both_absent() {
    let extraction = extract_file("worker.pas", PASCAL);
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "worker.pas".to_string(),
            "worker.pas::Helper".to_string(),
            "worker.pas::Run".to_string(),
        ]
    );
    assert_eq!(
        edges(&extraction),
        vec![
            "worker.pas::Run -> Helper".to_string(),
            "worker.pas::Run -> WriteLn".to_string(),
        ]
    );
}

/// One qualified name per routine, though the unit spells each of them twice.
///
/// The `interface` section declares `Helper` and `Run`, the `implementation`
/// section defines them. Emitting both spellings would give two nodes one name,
/// which is the SC14 broken join key — and it is the shape a `declProc`-based
/// rule produces for *every* routine in *every* unit, not for a corner case.
#[test]
fn pascal_a_routine_declared_and_defined_in_one_unit_is_one_symbol() {
    for (path, source) in [("worker.pas", PASCAL), ("deep.pas", PASCAL_CLASS)] {
        let extraction = extract_file(path, source);
        assert_eq!(
            duplicate_names(&extraction),
            Vec::<String>::new(),
            "{path} emitted a duplicate qualified name: {:?}",
            qualified_names(&extraction)
        );
    }
}

/// Every Pascal call shape, including the one with no call syntax in it.
#[test]
fn pascal_records_every_call_shape_including_the_parenthesis_free_one() {
    let extraction = extract_file("deep.pas", PASCAL_CLASS);
    assert_eq!(
        targets(&extraction),
        vec![
            // `A.B.Calc(1)` — reached through the qualifier's last segment,
            // because a receiver is looked up as a variable name and `A.B` is
            // not one.
            "B::Calc".to_string(),
            // `X := Calc(A);`
            "Calc".to_string(),
            // `Cleanup;` — a statement that is one identifier is a call, and
            // this is how most parameterless Pascal calls are written.
            "Cleanup".to_string(),
            // `Result := Helper(A);` in `TWorker.Calc`.
            "Helper".to_string(),
            // `X := Self.Calc(A);`
            "Self::Calc".to_string(),
            // `X := W.Calc(A);`
            "W::Calc".to_string(),
        ],
        "`inherited;` must contribute nothing: it names the parent type's \
         routine and the call site never says which type that is"
    );
}

/// A call inside a method must name the method, not the file and not the type.
#[test]
fn pascal_attributes_a_method_body_to_the_method() {
    let extraction = extract_file("deep.pas", PASCAL_CLASS);
    assert!(
        edges(&extraction).contains(&"deep.pas::TWorker.Calc -> Helper".to_string()),
        "{:?}",
        edges(&extraction)
    );
    assert_eq!(
        extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "deep.pas::TWorker.Go")
            .map(|symbol| symbol.kind),
        Some(SymbolKind::Method),
        "a routine whose definition names its owner is a method: {:?}",
        qualified_names(&extraction)
    );
}

/// No call and no reference may name a source the emitter did not produce.
#[test]
fn no_erlang_or_pascal_edge_is_orphaned() {
    let mut attributed = 0;
    for (path, source) in [
        ("worker.erl", ERLANG),
        ("deep.erl", ERLANG_SHAPES),
        ("worker.pas", PASCAL),
        ("deep.pas", PASCAL_CLASS),
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
    // Without this the check above passes for an extractor that attributes
    // everything to the file, which is what a broken mirror degrades to.
    assert!(
        attributed >= 12,
        "only {attributed} calls were attributed to a symbol rather than to the \
         file; the orphan check would be holding vacuously"
    );
}

/// The declared capability must never claim a language the dispatcher does not
/// route, and must never omit one it does.
///
/// Repointed from `CALL_EXTRACTION_LANGUAGES`, which had no production reader
/// and was wrong by omission for 14 languages; `LanguageSpec::capabilities` is
/// the registry that replaced it.
#[test]
fn the_coverage_list_and_the_dispatcher_agree_for_erlang_and_pascal() {
    for (language, path, source) in [
        ("erlang", "w.erl", "-module(w).\nm() -> helper(1).\n"),
        ("pascal", "w.pas", "program W;\nbegin\n  Helper(1);\nend.\n"),
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
fn erlang_and_pascal_extraction_is_deterministic() {
    for (path, source) in [("deep.erl", ERLANG_SHAPES), ("deep.pas", PASCAL_CLASS)] {
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

/// Unparseable input must not panic and must not invent calls.
#[test]
fn broken_erlang_and_pascal_yield_no_invented_calls() {
    for (path, source) in [
        ("bad.erl", "-module(bad).\nf( -> ,,, end.\n"),
        ("bad.pas", "unit U; begin begin begin\n"),
    ] {
        let extraction = extract_file(path, source);
        assert_eq!(
            orphaned_sources(&extraction),
            Vec::<String>::new(),
            "{path} orphaned an edge while recovering from a parse error"
        );
    }
}
