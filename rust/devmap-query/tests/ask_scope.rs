//! A scope narrows `ask`, `ask_evidence` and semantic `search` to part of the
//! repository, before ranking.
//!
//! The motivating case: auditing a React `frontend/` of a repository whose Go
//! backend has far more resolved names, a question no React symbol is named
//! after was answered with Go. These tests pin that a scope (a) keeps every
//! other language and subtree out of the answer, (b) is applied before the
//! TF-IDF index is built and to the PageRank node set — not as a filter over
//! an answer ranked against the whole corpus — and (c) is refused, never
//! answered, when it names nothing that was indexed.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{StoreQueryEngine, SymbolScope, ASK_DEFAULT_MIN_CONFIDENCE};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn store_of(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn scope(paths: &[&str], languages: &[&str]) -> SymbolScope {
    let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    let languages: Vec<String> = languages.iter().map(|l| l.to_string()).collect();
    SymbolScope::new(&paths, &languages)
        .unwrap()
        .expect("a non-empty scope")
}

/// A Go backend and a TypeScript frontend that both have names about runtime
/// state — the backend more of them, as on the repository that prompted this.
fn polyglot_store() -> Store {
    store_of(&[
        (
            "backend/state/store.go",
            "package state\n\n\
             func NewRuntimeStateStore() *Store { return &Store{} }\n\n\
             func UpdateRuntimeState(s *Store) { s.apply() }\n\n\
             func RuntimeStateSnapshot(s *Store) int { return 1 }\n",
        ),
        (
            "frontend/src/state.ts",
            "export function updateComponentState(next: number): number {\n  return next;\n}\n",
        ),
        (
            "frontend/src/view.tsx",
            "import { updateComponentState } from \"./state\";\n\nexport function StateView(): number {\n  return updateComponentState(1);\n}\n",
        ),
    ])
}

fn paths_of(response: &devmap_query::Response<devmap_query::SymbolHit>) -> Vec<String> {
    response
        .items
        .iter()
        .map(|hit| hit.file_path.clone())
        .collect()
}

#[test]
fn an_unscoped_ask_answers_from_every_language() {
    // Characterises the behaviour a scope exists to change: without one, the
    // backend's names are in the answer to a frontend question.
    let store = polyglot_store();
    let response = StoreQueryEngine::new(&store)
        .ask("update runtime state", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    let paths = paths_of(&response);
    assert!(
        paths.iter().any(|path| path.starts_with("backend/")),
        "{paths:?}"
    );
    assert!(response.scope.is_none(), "{:?}", response.scope);
}

#[test]
fn a_scoped_ask_answers_only_from_its_subtree() {
    let store = polyglot_store();
    let response = StoreQueryEngine::new(&store)
        .ask_scoped(
            "update runtime state",
            10_000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["frontend/"], &[])),
        )
        .unwrap();
    let paths = paths_of(&response);
    assert!(
        !paths.is_empty(),
        "the frontend has state names: {response:?}"
    );
    assert!(
        paths.iter().all(|path| path.starts_with("frontend/")),
        "{paths:?}"
    );
    let report = response.scope.expect("a scoped answer reports its scope");
    assert_eq!(report.paths, vec!["frontend".to_string()]);
    assert_eq!(report.files, 2);
    assert_eq!(report.corpus_files, 3);
    assert!(
        report.symbols >= 2 && report.symbols < report.corpus_symbols,
        "{report:?}"
    );
    assert!(
        response.total <= report.symbols,
        "total counts matches within the scope: {} > {}",
        response.total,
        report.symbols
    );
}

#[test]
fn a_language_scope_answers_only_from_that_language() {
    let store = polyglot_store();
    let response = StoreQueryEngine::new(&store)
        .ask_scoped(
            "update runtime state",
            10_000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&[], &["Go"])),
        )
        .unwrap();
    let paths = paths_of(&response);
    assert!(!paths.is_empty(), "{response:?}");
    assert!(paths.iter().all(|path| path.ends_with(".go")), "{paths:?}");
    assert_eq!(response.scope.unwrap().languages, vec!["go".to_string()]);
}

#[test]
fn a_scoped_semantic_search_answers_only_from_its_subtree() {
    let store = polyglot_store();
    let engine = StoreQueryEngine::new(&store);
    let unscoped = engine.search_semantic("runtime state", 10_000).unwrap();
    assert!(
        paths_of(&unscoped)
            .iter()
            .any(|path| path.starts_with("backend/")),
        "{unscoped:?}"
    );
    let scoped = engine
        .search_semantic_scoped("runtime state", 10_000, Some(&scope(&["frontend"], &[])))
        .unwrap();
    let paths = paths_of(&scoped);
    assert!(!paths.is_empty(), "{scoped:?}");
    assert!(
        paths.iter().all(|path| path.starts_with("frontend/")),
        "{paths:?}"
    );
    assert_eq!(scoped.scope.unwrap().files, 2);
}

#[test]
fn a_scoped_evidence_pack_holds_only_scoped_files() {
    let store = polyglot_store();
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence_scoped(
            "update runtime state",
            10_000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["frontend"], &[])),
        )
        .unwrap();
    assert!(!pack.files.is_empty(), "{pack:?}");
    assert!(
        pack.files
            .iter()
            .all(|file| file.file_path.starts_with("frontend/")),
        "{:?}",
        pack.files
    );
    assert_eq!(pack.scope.expect("the pack reports its scope").files, 2);
}

/// IDF is a property of the corpus, so a scope must be applied before the
/// index is built. Over the whole repository `render` is common (the backend
/// renders eleven things) and `cache` rare, so the frontend's `cache*` names
/// outrank its one `render` name. Within the frontend it is the reverse:
/// three `cache*` names and one `render` name. A scope applied as a filter
/// over the whole-corpus ranking would still lead with a `cache*` name.
#[test]
fn a_scope_weighs_terms_by_the_scoped_corpus() {
    let backend: String = (0..11)
        .map(|i| format!("def render_page_{i}():\n    return {i}\n\n"))
        .collect();
    let store = store_of(&[
        ("backend/pages.py", backend.as_str()),
        (
            "frontend/view.py",
            "def cache_alpha():\n    return 1\n\n\
             def cache_beta():\n    return 2\n\n\
             def cache_gamma():\n    return 3\n\n\
             def render_view():\n    return 4\n",
        ),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let first_frontend = |response: &devmap_query::Response<devmap_query::SymbolHit>| {
        response
            .items
            .iter()
            .find(|hit| hit.file_path.starts_with("frontend/"))
            .map(|hit| hit.symbol_name.clone())
    };
    let unscoped = engine.search_semantic("render cache", 10_000).unwrap();
    assert!(
        first_frontend(&unscoped).unwrap().starts_with("cache_"),
        "over the whole corpus `cache` is the rarer term: {unscoped:?}"
    );
    let scoped = engine
        .search_semantic_scoped("render cache", 10_000, Some(&scope(&["frontend"], &[])))
        .unwrap();
    assert_eq!(
        first_frontend(&scoped).as_deref(),
        Some("render_view"),
        "within the frontend `render` is the rarer term: {scoped:?}"
    );
    let asked = engine
        .ask_scoped(
            "render cache",
            10_000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["frontend"], &[])),
        )
        .unwrap();
    assert_eq!(
        first_frontend(&asked).as_deref(),
        Some("render_view"),
        "{asked:?}"
    );
}

/// The PageRank node set is scoped too: rank must not leave the scope through
/// a call and come back through code the caller excluded.
///
/// `state_alpha` and `state_beta` are equally relevant to "state". Unscoped,
/// alpha calls `relay` in `lib/`, which calls beta, so beta collects graph
/// mass alpha sends round through `lib/`. Scoped to `app/`, that path does not
/// exist and the two stay level.
#[test]
fn a_scope_keeps_rank_from_flowing_through_excluded_code() {
    let store = store_of(&[
        (
            "app/x.py",
            "from lib.hub import relay\n\n\
             def state_alpha():\n    return relay()\n\n\
             def state_beta():\n    return 2\n",
        ),
        (
            "lib/hub.py",
            "from app.x import state_beta\n\n\
             def relay():\n    return state_beta()\n",
        ),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let score_of = |response: &devmap_query::Response<devmap_query::SymbolHit>, name: &str| {
        response
            .items
            .iter()
            .find(|hit| hit.symbol_name == name)
            .map(|hit| hit.score)
            .unwrap_or_else(|| panic!("{name} missing from {response:?}"))
    };
    let unscoped = engine
        .ask("state", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    assert!(
        score_of(&unscoped, "state_beta") > score_of(&unscoped, "state_alpha"),
        "the round trip through lib/ must lift beta unscoped, or this fixture \
         does not exercise the node set: {unscoped:?}"
    );
    let scoped = engine
        .ask_scoped(
            "state",
            10_000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["app"], &[])),
        )
        .unwrap();
    assert_eq!(
        score_of(&scoped, "state_beta"),
        score_of(&scoped, "state_alpha"),
        "no in-scope edge joins them, so neither may be lifted: {scoped:?}"
    );
}

#[test]
fn a_prefix_that_matches_no_indexed_file_is_refused_by_every_scoped_query() {
    let store = polyglot_store();
    let engine = StoreQueryEngine::new(&store);
    let typo = scope(&["frontnd"], &[]);
    let errors = [
        engine
            .ask_scoped("state", 2000, ASK_DEFAULT_MIN_CONFIDENCE, Some(&typo))
            .unwrap_err()
            .to_string(),
        engine
            .ask_evidence_scoped("state", 2000, ASK_DEFAULT_MIN_CONFIDENCE, Some(&typo))
            .unwrap_err()
            .to_string(),
        engine
            .search_semantic_scoped("state", 2000, Some(&typo))
            .unwrap_err()
            .to_string(),
    ];
    for error in errors {
        assert!(
            error.contains("\"frontnd\" matches no indexed file"),
            "{error}"
        );
        assert!(
            error.contains("frontend/"),
            "the refusal names what is there: {error}"
        );
    }
}

#[test]
fn a_prefix_is_refused_even_when_the_question_is_empty() {
    // The refusal is about the scope, so it cannot depend on the question.
    let store = polyglot_store();
    assert!(StoreQueryEngine::new(&store)
        .ask_scoped(
            "",
            2000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["nope"], &[]))
        )
        .is_err());
}

#[test]
fn a_language_that_labels_no_indexed_file_is_refused() {
    let store = polyglot_store();
    let error = StoreQueryEngine::new(&store)
        .ask_scoped(
            "state",
            2000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&[], &["rust"])),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("\"rust\" labels no indexed file"), "{error}");
    assert!(error.contains("go"), "{error}");
}

#[test]
fn a_prefix_and_language_with_no_file_in_common_are_refused() {
    let store = polyglot_store();
    let error = StoreQueryEngine::new(&store)
        .ask_scoped(
            "state",
            2000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["frontend"], &["go"])),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("no indexed file is both"), "{error}");
}

#[test]
fn a_file_that_declares_nothing_is_an_empty_answer_not_a_refusal() {
    // The refusal condition is "matches no indexed file". A file that was
    // indexed and declares nothing is a scope with nothing in it that answers
    // the question, which is an answer: empty, with the scope saying so. Its
    // one symbol is the file-level node every indexed file carries.
    let store = store_of(&[
        ("src/lib.py", "def state_alpha():\n    return 1\n"),
        ("docs/notes.py", "# nothing declared here\n"),
    ]);
    let response = StoreQueryEngine::new(&store)
        .ask_scoped(
            "state",
            2000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["docs"], &[])),
        )
        .unwrap();
    assert!(response.items.is_empty(), "{response:?}");
    let report = response.scope.unwrap();
    assert_eq!((report.files, report.symbols), (1, 1), "{report:?}");
}

#[test]
fn related_tests_outside_the_scope_are_left_out_and_counted() {
    let store = store_of(&[
        (
            "frontend/src/state.ts",
            "export function updateComponentState(next: number): number {\n  return next;\n}\n",
        ),
        (
            "e2e/flow.test.ts",
            "import { updateComponentState } from \"../frontend/src/state\";\n\n\
             export function verifiesNext(): number {\n  return updateComponentState(2);\n}\n",
        ),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let related = |pack: &devmap_query::EvidencePack| -> Vec<String> {
        pack.related_tests
            .items
            .iter()
            .map(|test| test.path.clone())
            .collect()
    };
    let unscoped = engine
        .ask_evidence("update component state", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    assert_eq!(
        related(&unscoped),
        vec!["e2e/flow.test.ts".to_string()],
        "the fixture must reach the e2e test unscoped: {unscoped:?}"
    );
    let scoped = engine
        .ask_evidence_scoped(
            "update component state",
            10_000,
            ASK_DEFAULT_MIN_CONFIDENCE,
            Some(&scope(&["frontend"], &[])),
        )
        .unwrap();
    assert!(related(&scoped).is_empty(), "{scoped:?}");
    assert_eq!(
        scoped.scope.unwrap().related_tests_outside_scope,
        1,
        "a test the walk reached and the scope dropped is counted, not silent"
    );
}
