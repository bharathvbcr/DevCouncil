//! Call extraction for Lua, Luau and R (SC34).
//!
//! Two different defects, and the tests say which is which rather than letting
//! one story cover both.
//!
//! **Lua is a migration regression.** Measured with the release binary on
//! `function helper(a) return a end` / `function main() return helper(1) end`:
//! this port produced 2 `Contains` edges and **0** `Calls`, while the Python
//! implementation it replaces produced `i.lua::main -> i.lua::helper calls`.
//!
//! **R is a genuine gap.** The same measurement on the R equivalent gives 0
//! `Calls` from *both* implementations. Nothing is being restored here.
//!
//! Every test drives the language module directly, because the dispatcher arm
//! in `langcalls::mod` is applied separately; `the_coverage_list_and_the_
//! dispatcher_agree` is the one test that fails if the two ever disagree.

use devmap_extract::langcalls::{lua, r, CALL_EXTRACTION_LANGUAGES};
use devmap_extract::model::{ExtractedCall, ExtractedReference};
use devmap_extract::{extract_file, Extraction};
use tree_sitter::{Language, Parser};

/// Run one language module over every node of `source`, exactly as
/// `walk_tree` does, so the tests exercise the traversal contract too.
fn drive(
    language: Language,
    path: &str,
    source: &str,
    each: fn(tree_sitter::Node, &str, &str, &mut Vec<ExtractedCall>, &mut Vec<ExtractedReference>),
) -> (Vec<ExtractedCall>, Vec<ExtractedReference>) {
    let mut parser = Parser::new();
    parser.set_language(&language).expect("grammar loads");
    let tree = parser.parse(source, None).expect("source parses");
    let mut calls = Vec::new();
    let mut references = Vec::new();
    let mut worklist = vec![tree.root_node()];
    while let Some(node) = worklist.pop() {
        each(node, source, path, &mut calls, &mut references);
        for index in (0..node.child_count()).rev() {
            if let Some(child) = node.child(index) {
                worklist.push(child);
            }
        }
    }
    (calls, references)
}

fn lua_calls(path: &str, source: &str) -> Vec<ExtractedCall> {
    drive(
        tree_sitter_lua::LANGUAGE.into(),
        path,
        source,
        lua::extract_lua_call,
    )
    .0
}

fn luau_calls(path: &str, source: &str) -> Vec<ExtractedCall> {
    drive(
        tree_sitter_luau::LANGUAGE.into(),
        path,
        source,
        lua::extract_lua_call,
    )
    .0
}

fn r_calls(path: &str, source: &str) -> Vec<ExtractedCall> {
    drive(
        tree_sitter_r::LANGUAGE.into(),
        path,
        source,
        r::extract_r_call,
    )
    .0
}

/// `receiver::callee` when there is a receiver, else the bare callee. Sorted,
/// so a test pins a set rather than a traversal order.
fn targets(calls: &[ExtractedCall]) -> Vec<String> {
    let mut out: Vec<String> = calls
        .iter()
        .map(|call| match &call.receiver_expr {
            Some(receiver) => format!("{receiver}::{}", call.callee_name),
            None => call.callee_name.clone(),
        })
        .collect();
    out.sort();
    out
}

fn qualified_names(extraction: &Extraction) -> Vec<String> {
    extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect()
}

/// Every `caller_symbol` must name a symbol the extractor actually emitted.
///
/// This is the SC9/SC10 invariant, and it is checked rather than assumed: an
/// edge whose source names no node is worse than no edge, because it survives
/// every structural check on the store while joining to nothing.
fn assert_no_orphans(
    path: &str,
    source: &str,
    calls: &[ExtractedCall],
    refs: &[ExtractedReference],
) {
    let emitted = qualified_names(&extract_file(path, source));
    for call in calls {
        if let Some(caller) = &call.caller_symbol {
            assert!(
                emitted.contains(caller),
                "orphaned call edge in {path}: caller {caller:?} names no emitted symbol.\n\
                 emitted: {emitted:?}"
            );
        }
    }
    for reference in refs {
        if let Some(scope) = &reference.enclosing_symbol {
            assert!(
                emitted.contains(scope),
                "orphaned reference in {path}: scope {scope:?} names no emitted symbol.\n\
                 emitted: {emitted:?}"
            );
        }
    }
}

const LUA_SHAPES: &str = r#"
local M = {}
local socket = require "socket"
local json = require("json")

function helper(a) return a end
local function localized(b) return b end
function M.tablefn(c) return c end
function M:methodfn(d) return d end

function main()
  helper(1)
  localized(2)
  M.tablefn(3)
  M:methodfn(4)
  obj.field.deep(5)
  obj:method(6)
  configure{ retries = 3 }
  print(#M)
end

function outer()
  local function inner() return helper(7) end
  return inner()
end
"#;

/// The defect, at the shape level: every real Lua call form must be recovered,
/// and each must carry the receiver it is reached through.
///
/// Pre-change this file could not compile — `langcalls::lua` did not exist —
/// and the behaviour it pins was measurably absent: `extract_file` on this
/// source returns zero calls without the dispatcher arm, which
/// `the_coverage_list_and_the_dispatcher_agree` pins from the other side.
#[test]
fn lua_recovers_every_call_shape_with_its_receiver() {
    let calls = lua_calls("shapes.lua", LUA_SHAPES);
    assert_eq!(
        targets(&calls),
        vec![
            "M::methodfn",
            "M::tablefn",
            "configure",
            "helper",
            "helper",
            "inner",
            "localized",
            "obj.field::deep",
            "obj::method",
            "print",
            "require",
            "require",
        ],
        "actual: {calls:#?}"
    );
}

/// Argument sugar is not a separate call form and must not be dropped.
///
/// `require "socket"` and `configure{…}` have no parenthesised argument list;
/// keying on the call node rather than on its arguments is what covers them,
/// and this test is what stops that from being narrowed later.
#[test]
fn lua_string_and_table_call_sugar_are_calls() {
    assert_eq!(
        targets(&lua_calls(
            "sugar.lua",
            "require \"socket\"\nconfigure{ a = 1 }\n"
        )),
        vec!["configure", "require"]
    );
}

/// The colon form is the idiom for a method and must not be missed, but it is
/// not a different callee: `t:f()` and `t.f()` name the same `f` on the same
/// `t`. The colon's extra effect is passing an implicit `self` *argument*,
/// which is not part of the callee's identity.
#[test]
fn lua_colon_and_dot_calls_produce_the_same_identity() {
    let colon = lua_calls("c.lua", "function m() t:f(1) end\n");
    let dot = lua_calls("d.lua", "function m() t.f(1) end\n");
    assert_eq!(targets(&colon), vec!["t::f"]);
    assert_eq!(targets(&dot), targets(&colon));
}

/// Luau shares the grammar family and must behave identically — it is the
/// language a fix aimed at Lua alone would silently miss, exactly as Metal was
/// for the C family in SC19.
#[test]
fn luau_matches_lua_on_the_same_source() {
    let typed = "local M = {}\nfunction M.f(a: number): number return a end\nfunction main()\n  M.f(1)\n  helper(2)\n  require \"socket\"\nend\n";
    assert_eq!(
        targets(&luau_calls("t.luau", typed)),
        vec!["M::f", "helper", "require"]
    );
    assert_eq!(
        targets(&luau_calls("t.luau", LUA_SHAPES)),
        targets(&lua_calls("t.lua", LUA_SHAPES)),
        "the two grammars must not disagree on any shape"
    );
}

/// Attribution: a call must be owned by the function that makes it, including
/// inside a nested `local function` and inside an anonymous literal.
#[test]
fn lua_attributes_each_call_to_the_function_that_makes_it() {
    let source = "function outer()\n  local function inner() return one() end\n  local cb = function() return two() end\n  return three()\nend\nfour()\n";
    let calls = lua_calls("attr.lua", source);
    let mut owned: Vec<(String, String)> = calls
        .iter()
        .map(|call| {
            (
                call.callee_name.clone(),
                call.caller_symbol
                    .clone()
                    .unwrap_or_else(|| "<file>".into()),
            )
        })
        .collect();
    owned.sort();
    assert_eq!(
        owned,
        vec![
            ("four".to_string(), "<file>".to_string()),
            ("one".to_string(), "attr.lua::inner".to_string()),
            // The anonymous literal is not a symbol, so its calls belong to the
            // nearest enclosing declaration — the same rule the emitter uses.
            ("three".to_string(), "attr.lua::outer".to_string()),
            ("two".to_string(), "attr.lua::outer".to_string()),
        ]
    );
}

const R_SHAPES: &str = r#"
helper <- function(a) { a }
main <- function() {
  helper(1)
  pkg::exported(2)
  pkg:::internal(3)
  obj$method(4)
  s4obj@slot(5)
  outer(inner(6))
  do.call("helper", list())
  as.numeric("7")
  frame |> dplyr::filter(x)
  frame %>% summarise()
  frame %>% collect
}
"#;

/// R has no baseline to beat: the Python implementation extracts no R calls
/// either. This pins the shapes the new arm recovers.
#[test]
fn r_recovers_every_call_shape_with_its_receiver() {
    assert_eq!(
        targets(&r_calls("shapes.R", R_SHAPES)),
        vec![
            "as.numeric",
            "collect",
            "do.call",
            "dplyr::filter",
            "helper",
            "inner",
            "list",
            "obj::method",
            "outer",
            "pkg::exported",
            "pkg::internal",
            "s4obj::slot",
            "summarise",
        ]
    );
}

/// A dotted name is a name in R, and refusing it would drop most of base R.
///
/// The widening is gated on the grammar's own `identifier` token, so it can
/// admit `do.call` without admitting `obj$method` — which the next test pins
/// from the other direction.
#[test]
fn r_dotted_base_names_are_callees() {
    assert_eq!(
        targets(&r_calls("dots.R", "f <- function(d) {\n  data.frame(x = 1)\n  is.null(d)\n  Sys.time()\n  .hidden(2)\n}\n")),
        vec![".hidden", "Sys.time", "data.frame", "is.null"]
    );
}

/// The pipe decision, pinned.
///
/// `|>` and `%>%` with a call on the right need no special handling — the right
/// operand is already a `call` node. The bare magrittr form is not, and
/// magrittr applies it, so it is recorded. `%$%` masks a name rather than
/// applying it and `%in%` is a comparison; neither is a call, and R rejects a
/// bare right operand for `|>` outright, so inventing one would describe code
/// that cannot run.
#[test]
fn r_pipes_record_only_the_operators_that_apply_their_right_operand() {
    assert_eq!(
        targets(&r_calls(
            "pipe.R",
            "a |> f()\nb %>% g()\nc %>% bare\nd %T>% tee\ne %<>% modify\nh %$% column\ni %in% set\nj %myop% custom\n"
        )),
        vec!["bare", "f", "g", "modify", "tee"]
    );
}

/// Attribution for R, including the nested-helper shape that the shared scope
/// builder gets wrong.
#[test]
fn r_attributes_each_call_to_the_function_that_makes_it() {
    let source = "outer <- function() {\n  inner <- function() { deep(1) }\n  inner()\n}\ntop(2)\n";
    let calls = r_calls("attr.R", source);
    let mut owned: Vec<(String, String)> = calls
        .iter()
        .map(|call| {
            (
                call.callee_name.clone(),
                call.caller_symbol
                    .clone()
                    .unwrap_or_else(|| "<file>".into()),
            )
        })
        .collect();
    owned.sort();
    assert_eq!(
        owned,
        vec![
            ("deep".to_string(), "attr.R::function".to_string()),
            ("inner".to_string(), "attr.R::function".to_string()),
            ("top".to_string(), "<file>".to_string()),
        ],
        "`attr.R::function` is degenerate but joinable — the R declaration path \
         names every function after the `function` keyword rather than after the \
         variable it is bound to, which is reported as a separate defect"
    );
}

/// No callee may be an expression — the SC26/SC32 defect class, checked against
/// the shapes most likely to reintroduce it.
///
/// Every one of these is a real call at runtime whose *callee* names nothing a
/// symbol could carry. Recording their text would produce rows that can never
/// join, crowding the tier reserved for genuine resolution defects, so the rule
/// is fail-closed: drop the name rather than invent one.
#[test]
fn no_callee_is_ever_an_expression() {
    let lua = lua_calls(
        "expr.lua",
        "function m()\n  t[k]()\n  chained()()\n  (function() return 1 end)()\nend\n",
    );
    assert_eq!(
        targets(&lua),
        vec!["chained"],
        "only the inner call of `chained()()` names anything: {lua:#?}"
    );

    let r = r_calls(
        "expr.R",
        "m <- function() {\n  lst[[1]]()\n  chained()(2)\n  (function(x) x)(3)\n  `weird name`(4)\n}\n",
    );
    assert_eq!(
        targets(&r),
        vec!["chained"],
        "backticked and computed callees name nothing joinable: {r:#?}"
    );

    for call in lua.iter().chain(r.iter()) {
        assert!(
            !call.callee_name.is_empty()
                && call
                    .callee_name
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.')),
            "callee {:?} is expression-shaped",
            call.callee_name
        );
    }
}

/// The joinability invariant, on every fixture in this file.
#[test]
fn no_call_or_reference_is_orphaned() {
    for (path, source) in [
        ("shapes.lua", LUA_SHAPES),
        (
            "nested.lua",
            "function outer()\n  local function inner() return one() end\n  return inner()\nend\n",
        ),
        ("sugar.lua", "require \"socket\"\nconfigure{ a = 1 }\n"),
        ("empty.lua", ""),
        ("comment.lua", "-- nothing here\n"),
    ] {
        let (calls, refs) = drive(
            tree_sitter_lua::LANGUAGE.into(),
            path,
            source,
            lua::extract_lua_call,
        );
        assert_no_orphans(path, source, &calls, &refs);
    }

    for (path, source) in [
        ("shapes.R", R_SHAPES),
        (
            "nested.R",
            "outer <- function() {\n  inner <- function() { deep(1) }\n  inner()\n}\n",
        ),
        ("empty.R", ""),
        ("comment.R", "# nothing here\n"),
        (
            "anon.R",
            "setMethod(\"show\", \"cls\", function(object) { print(object) })\n",
        ),
    ] {
        let (calls, refs) = drive(
            tree_sitter_r::LANGUAGE.into(),
            path,
            source,
            r::extract_r_call,
        );
        assert_no_orphans(path, source, &calls, &refs);
    }
}

/// Every call is also a reference, with the same scope, so the two surfaces
/// cannot disagree about who made a call.
#[test]
fn every_call_is_mirrored_by_a_reference_in_the_same_scope() {
    for (language, path, source, each) in [
        (
            Language::from(tree_sitter_lua::LANGUAGE),
            "m.lua",
            LUA_SHAPES,
            lua::extract_lua_call
                as fn(
                    tree_sitter::Node,
                    &str,
                    &str,
                    &mut Vec<ExtractedCall>,
                    &mut Vec<ExtractedReference>,
                ),
        ),
        (
            Language::from(tree_sitter_r::LANGUAGE),
            "m.R",
            R_SHAPES,
            r::extract_r_call,
        ),
    ] {
        let (calls, refs) = drive(language, path, source, each);
        assert_eq!(calls.len(), refs.len(), "{path}");
        for (call, reference) in calls.iter().zip(refs.iter()) {
            assert_eq!(call.callee_name, reference.name, "{path}");
            assert_eq!(call.caller_symbol, reference.enclosing_symbol, "{path}");
        }
    }
}

/// Extraction is a pure function of the source: repeated runs must be
/// byte-identical, including order. Nothing here is keyed by a hash map, and
/// this is what keeps it that way.
#[test]
fn extraction_is_deterministic() {
    let first = format!("{:?}", lua_calls("d.lua", LUA_SHAPES));
    let second = format!("{:?}", lua_calls("d.lua", LUA_SHAPES));
    assert_eq!(first, second);
    let first = format!("{:?}", r_calls("d.R", R_SHAPES));
    let second = format!("{:?}", r_calls("d.R", R_SHAPES));
    assert_eq!(first, second);
}

/// Unparseable input must not panic and must not invent calls.
#[test]
fn a_broken_source_yields_no_calls_rather_than_a_panic() {
    assert!(lua_calls("bad.lua", "function ((( end end end\n").is_empty());
    assert!(r_calls("bad.R", "f <- function( { { {\n").is_empty());
}

/// The coverage list must never claim a language the dispatcher does not
/// route, and must never omit one it does.
///
/// `CALL_EXTRACTION_LANGUAGES` exists so "this language has no call graph" is a
/// stated fact rather than an indistinguishable zero; a list that disagrees
/// with the dispatcher turns it into a lie. This is the one test here that
/// changes answer when the dispatcher arm is applied, and it stays green on
/// both sides of that change — it fails only if the two halves disagree.
#[test]
fn the_coverage_list_and_the_dispatcher_agree() {
    for (language, path, source) in [
        ("lua", "w.lua", "function m() helper(1) end\n"),
        ("luau", "w.luau", "function m() helper(1) end\n"),
        ("r", "w.R", "m <- function() { helper(1) }\n"),
    ] {
        let listed = CALL_EXTRACTION_LANGUAGES.contains(&language);
        let extracted = !extract_file(path, source).calls.is_empty();
        assert_eq!(
            listed, extracted,
            "{language}: coverage list says {listed}, extraction says {extracted} — \
             either the dispatcher arm or the list entry is missing"
        );
    }
}
