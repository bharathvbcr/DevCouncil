//! Module-qualified (`webcontent::snapshot`, `sysload::webcontent::snapshot`),
//! directory-module (`sysload::init`, `panel::renderPanel`, `auth::check_token`),
//! and `Type::method` (`Ring::dump`, `webcontent::Ring::dump`,
//! `src-tauri/src/sysload/webcontent.rs::Ring::dump`) queries resolve consistently
//! across `search`, `explore`, `impact`, `affected_tests`, `neighbors`, `trace`,
//! `trace_between`, and `dependencies` — and `trace_between` scopes its
//! attribution gap to the nodes it actually walked.

use devmap_extract::extract_file;
use devmap_query::{QueryEngine, Request, ResolutionAvailability, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const WEBCONTENT_RS: &str = r#"
pub struct Ring {
    len: usize,
}

impl Ring {
    pub fn dump(&self) -> usize {
        self.len
    }
}

pub fn snapshot(ring: &Ring) -> usize {
    ring.dump()
}

pub fn render_snapshot(ring: &Ring) -> usize {
    snapshot(ring)
}
"#;

const SYSLOAD_MOD_RS: &str = r#"
pub fn init() -> usize {
    1
}

pub fn boot() -> usize {
    init()
}
"#;

const PANEL_INDEX_TS: &str = r#"
export function renderPanel(): number {
    return 1;
}

export function mountPanel(): number {
    return renderPanel();
}
"#;

const AUTH_INIT_PY: &str =
    "def check_token():\n    return True\n\n\ndef require_auth():\n    return check_token()\n";

const WEBCONTENT_TEST_RS: &str = r#"
#[test]
fn test_webcontent_snapshot() {
    let ring = Ring { len: 1 };
    let _ = snapshot(&ring);
}
"#;

const OTHER_RING_RS: &str = r#"
pub struct Ring {
    n: usize,
}

impl Ring {
    pub fn dump(&self) -> usize {
        self.n
    }
}

pub fn use_other(ring: &Ring) -> usize {
    ring.dump()
}
"#;

/// Unrelated file holding an unattributed receiver call with multiple namesakes.
const UNRELATED_PY: &str = "class Alpha:\n    def frobnicate(self):\n        return 1\n\n\n\
class Beta:\n    def frobnicate(self):\n        return 2\n\n\n\
def unrelated_caller(x):\n    return x.frobnicate()\n";

fn build_fixture(
    files: &[(&str, &str)],
) -> (
    Store,
    Vec<devmap_extract::model::Extraction>,
    devmap_resolve::model::ResolutionResult,
) {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
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
    (store, extractions, resolution)
}

fn req(query: &str, depth: usize) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 20_000,
        min_confidence: 0.0,
        max_depth: depth,
    }
}

#[test]
fn module_and_type_qualified_queries_resolve_across_every_surface() {
    let (store, extractions, resolution) = build_fixture(&[
        ("src-tauri/src/sysload/webcontent.rs", WEBCONTENT_RS),
        ("src-tauri/src/sysload/mod.rs", SYSLOAD_MOD_RS),
        ("src/ui/panel/index.ts", PANEL_INDEX_TS),
        ("pkg/auth/__init__.py", AUTH_INIT_PY),
        ("tests/webcontent_test.rs", WEBCONTENT_TEST_RS),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let mem = QueryEngine::new(&extractions, &resolution);

    for query in [
        "webcontent::snapshot",
        "sysload::webcontent::snapshot",
        "webcontent.rs::snapshot",
        "src-tauri/src/sysload/webcontent.rs::snapshot",
    ] {
        let search = engine.search(req(query, 2)).unwrap();
        assert!(
            search.items.iter().any(|hit| {
                hit.symbol_name == "snapshot"
                    && hit.file_path == "src-tauri/src/sysload/webcontent.rs"
            }),
            "store search({query:?}) missed snapshot: {search:?}"
        );
        let mem_search = mem.search(req(query, 2));
        assert!(
            mem_search.items.iter().any(|hit| {
                hit.symbol_name == "snapshot"
                    && hit.file_path == "src-tauri/src/sysload/webcontent.rs"
            }),
            "in-memory search({query:?}) missed snapshot: {mem_search:?}"
        );

        let explore = engine.explore(query, 5, 20_000, 0.0, 2).unwrap();
        assert!(
            explore
                .definitions
                .items
                .iter()
                .any(|def| def.qualified_name == "src-tauri/src/sysload/webcontent.rs::snapshot"),
            "explore({query:?}) missed snapshot: {explore:?}"
        );

        let impact = engine.impact(req(query, 2)).unwrap();
        assert!(
            matches!(impact.resolution, ResolutionAvailability::Available),
            "impact({query:?}) unavailable: {impact:?}"
        );
        assert!(
            impact
                .items
                .iter()
                .any(|edge| edge.source_symbol.ends_with("::render_snapshot")),
            "impact({query:?}) missed render_snapshot: {impact:?}"
        );

        let affected = engine
            .affected_tests(&[query.to_string()], 20_000, 0.0, 3)
            .unwrap();
        assert!(
            affected
                .tests
                .items
                .iter()
                .any(|test| test.path == "tests/webcontent_test.rs"),
            "affected_tests({query:?}) missed webcontent_test.rs: {affected:?}"
        );

        let neighbors = engine
            .neighbors(&[query.to_string()], 20_000, 0.0, 1)
            .unwrap();
        assert_eq!(neighbors.len(), 1, "{neighbors:?}");
        assert!(
            matches!(
                neighbors[0].callers.resolution,
                ResolutionAvailability::Available
            ) && matches!(
                neighbors[0].callees.resolution,
                ResolutionAvailability::Available
            ),
            "neighbors({query:?}) unavailable: {neighbors:?}"
        );

        let trace = engine.trace(req(query, 2)).unwrap();
        assert!(
            matches!(trace.resolution, ResolutionAvailability::Available),
            "trace({query:?}) unavailable: {trace:?}"
        );
        assert!(
            trace
                .items
                .iter()
                .any(|edge| edge.target_symbol == "src-tauri/src/sysload/webcontent.rs::Ring.dump"),
            "trace({query:?}) missed Ring.dump: {trace:?}"
        );

        let deps = engine.dependencies(req(query, 1)).unwrap();
        assert!(
            matches!(deps.resolution, ResolutionAvailability::Available),
            "dependencies({query:?}) unavailable: {deps:?}"
        );
        assert!(
            deps.items
                .iter()
                .any(|edge| edge.target_symbol == "src-tauri/src/sysload/webcontent.rs::Ring.dump"),
            "dependencies({query:?}) missed Ring.dump: {deps:?}"
        );
    }

    // `Type::method`, `module::Type::method`, and `file::Type::method` forms all resolve.
    for query in [
        "Ring::dump",
        "webcontent::Ring::dump",
        "sysload::webcontent::Ring::dump",
        "webcontent.rs::Ring::dump",
        "src-tauri/src/sysload/webcontent.rs::Ring::dump",
    ] {
        let impact = engine.impact(req(query, 2)).unwrap();
        assert!(
            matches!(impact.resolution, ResolutionAvailability::Available),
            "impact({query:?}) unavailable: {impact:?}"
        );
        assert!(
            impact
                .items
                .iter()
                .any(|edge| edge.source_symbol == "src-tauri/src/sysload/webcontent.rs::snapshot"),
            "impact({query:?}) missed snapshot caller: {impact:?}"
        );

        let search = engine.search(req(query, 2)).unwrap();
        assert!(
            search.items.iter().any(|hit| {
                hit.symbol_name == "dump" && hit.file_path == "src-tauri/src/sysload/webcontent.rs"
            }),
            "search({query:?}) missed Ring.dump: {search:?}"
        );
    }

    // Directory module files (`mod.rs`, `index.ts`, `__init__.py`).
    for (query, expected_caller) in [
        ("sysload::init", "src-tauri/src/sysload/mod.rs::boot"),
        ("panel::renderPanel", "src/ui/panel/index.ts::mountPanel"),
        ("auth::check_token", "pkg/auth/__init__.py::require_auth"),
    ] {
        let impact = engine.impact(req(query, 2)).unwrap();
        assert!(
            matches!(impact.resolution, ResolutionAvailability::Available),
            "impact({query:?}) unavailable: {impact:?}"
        );
        assert!(
            impact
                .items
                .iter()
                .any(|edge| edge.source_symbol == expected_caller),
            "impact({query:?}) missed {expected_caller}: {impact:?}"
        );
    }

    // `trace_between` works with both module-qualified and `Type::method` endpoints.
    let between = engine
        .trace_between(Request {
            query: ("webcontent::snapshot".to_string(), "Ring::dump".to_string()),
            token_budget: 20_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .unwrap();
    assert!(
        matches!(between.resolution, ResolutionAvailability::Available),
        "trace_between unavailable: {between:?}"
    );
    assert_eq!(between.items.len(), 1, "{between:?}");
    assert_eq!(
        between.items[0].source_symbol,
        "src-tauri/src/sysload/webcontent.rs::snapshot"
    );
    assert_eq!(
        between.items[0].target_symbol,
        "src-tauri/src/sysload/webcontent.rs::Ring.dump"
    );
}

#[test]
fn ambiguous_type_method_is_refused_until_qualified_by_module_or_file() {
    let (store, extractions, resolution) = build_fixture(&[
        ("src-tauri/src/sysload/webcontent.rs", WEBCONTENT_RS),
        ("src/other/ring.rs", OTHER_RING_RS),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let mem = QueryEngine::new(&extractions, &resolution);

    // Pure `Ring::dump` matches two distinct files, so `impact` and `dependencies` refuse it.
    let impact_err = engine
        .impact(req("Ring::dump", 2))
        .expect_err("expected ambiguity refusal for impact(Ring::dump)");
    assert!(
        impact_err.to_string().contains("ambiguous")
            && impact_err.to_string().contains("webcontent.rs::Ring.dump")
            && impact_err.to_string().contains("other/ring.rs::Ring.dump"),
        "unexpected error: {impact_err}"
    );
    let deps_err = engine
        .dependencies(req("Ring::dump", 1))
        .expect_err("expected ambiguity refusal for dependencies(Ring::dump)");
    assert!(
        deps_err.to_string().contains("ambiguous"),
        "unexpected error: {deps_err}"
    );
    let mem_deps = mem.dependencies(req("Ring::dump", 1));
    assert!(
        matches!(
            &mem_deps.resolution,
            ResolutionAvailability::Unavailable { reason } if reason.contains("ambiguous")
        ),
        "expected in-memory ambiguity refusal for dependencies(Ring::dump), got {mem_deps:?}"
    );

    // Disambiguating by module stem succeeds for each file independently.
    let web = engine.impact(req("webcontent::Ring::dump", 2)).unwrap();
    assert!(matches!(web.resolution, ResolutionAvailability::Available));
    assert!(web
        .items
        .iter()
        .all(|edge| edge.target_file == "src-tauri/src/sysload/webcontent.rs"));

    let other = engine.impact(req("other::ring::Ring::dump", 2)).unwrap();
    assert!(matches!(
        other.resolution,
        ResolutionAvailability::Available
    ));
    assert!(other
        .items
        .iter()
        .all(|edge| edge.target_file == "src/other/ring.rs"));
}

#[test]
fn trace_between_reports_only_walked_attribution_gaps() {
    let (store, _, _) = build_fixture(&[
        ("src-tauri/src/sysload/webcontent.rs", WEBCONTENT_RS),
        ("unrelated.py", UNRELATED_PY),
    ]);
    let engine = StoreQueryEngine::new(&store);

    // Unrelated file has an unexplained unbound call, so repo-wide coverage is < 100%,
    // yet tracing between clean Rust functions must NOT carry a repo-wide walk_incomplete note.
    let clean = engine
        .trace_between(Request {
            query: (
                "webcontent::render_snapshot".to_string(),
                "Ring::dump".to_string(),
            ),
            token_budget: 20_000,
            min_confidence: 0.0,
            max_depth: 4,
        })
        .unwrap();
    assert!(matches!(
        clean.resolution,
        ResolutionAvailability::Available
    ));
    assert_eq!(clean.items.len(), 2, "{clean:?}");
    assert_eq!(
        clean.walk_incomplete, None,
        "unrelated unbound call must not taint trace_between: {clean:?}"
    );

    // Tracing from a function that DOES contain an unbound call must report that site.
    let dirty = engine
        .trace_between(Request {
            query: (
                "unrelated::unrelated_caller".to_string(),
                "webcontent::snapshot".to_string(),
            ),
            token_budget: 20_000,
            min_confidence: 0.0,
            max_depth: 4,
        })
        .unwrap();
    let note = dirty
        .walk_incomplete
        .as_deref()
        .expect("walked node with unbound call must report walk_incomplete");
    assert!(note.contains("frobnicate"), "{note}");
    assert!(!note.contains("repository-wide"), "{note}");
}
