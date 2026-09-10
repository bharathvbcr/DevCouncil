use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

#[test]
fn test_g5_no_multicandidate_extracted() {
    // closes G5
    let ext1 = extract_file("pkg/a.py", "def process(): pass\n");
    let ext2 = extract_file("pkg/b.py", "def process(): pass\n");
    let caller = extract_file("main.py", "def run(): process()\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[ext1.clone(), ext2.clone(), caller.clone()]);
    let res = resolver.resolve_all(&[ext1, ext2, caller]);

    let edge = res
        .edges
        .iter()
        .find(|e| e.edge_kind == EdgeKind::Calls && e.target_symbol == "pkg/a.py::process")
        .expect("ambiguous call candidates must remain visible");
    assert_eq!(edge.confidence, Confidence::SPECULATIVE);
    assert!(matches!(
        edge.resolution.as_deref(),
        Some(Resolution::AmbiguousGlobal { .. })
    ));
}

#[test]
fn test_g3_stdlib_guard_python_only() {
    // closes G3
    let py_file = extract_file("app.py", "def run(): open()\n");
    let py_candidate = extract_file("helpers.py", "def open(): pass\n");
    let js_file = extract_file("app.js", "function run() { open(); }\n");
    let js_candidate = extract_file("helpers.js", "function open() {}\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[
        py_file.clone(),
        py_candidate.clone(),
        js_file.clone(),
        js_candidate.clone(),
    ]);
    let res = resolver.resolve_all(&[py_file, py_candidate, js_file, js_candidate]);

    assert!(!res.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Calls
            && edge.source_file == "app.py"
            && edge.target_file == "helpers.py"
    }));
    assert!(res.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Calls
            && edge.source_file == "app.js"
            && edge.target_file == "helpers.js"
    }));
}

#[test]
fn test_g6_import_scoped_no_silent_widening() {
    // closes G6
    let ext_a = extract_file("a.py", "from sys import path\ndef run(): foo()\n");
    let ext_b = extract_file("b.py", "def foo(): pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[ext_a.clone(), ext_b.clone()]);
    let res = resolver.resolve_all(&[ext_a, ext_b]);

    let edge = res
        .edges
        .iter()
        .find(|e| e.edge_kind == EdgeKind::Calls && e.target_symbol == "b.py::foo")
        .expect("unique global fallback should remain explicit");
    assert_eq!(edge.confidence, Confidence::HIGH);
    assert!(matches!(
        edge.resolution.as_deref(),
        Some(Resolution::UniqueGlobal { .. })
    ));
}

#[test]
fn test_g20_go_package_star_topology() {
    // closes G20
    let go1 = extract_file("pkg/a.go", "package pkg\nfunc A() {}\n");
    let go2 = extract_file("pkg/b.go", "package pkg\nfunc B() {}\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[go1.clone(), go2.clone()]);
    let res = resolver.resolve_all(&[go1, go2]);

    assert!(res
        .edges
        .iter()
        .any(|e| e.edge_kind == EdgeKind::MemberOf || e.target_symbol.contains("pkg")));
}

#[test]
fn test_import_bindings_do_not_depend_on_input_order() {
    let importer = extract_file(
        "src/main.py",
        "from .utils import helper\ndef run():\n    helper()\n",
    );
    let target = extract_file("src/utils.py", "def helper(): pass\n");

    // Deliberately put the importer first. Binding construction must happen
    // after the complete symbol/file index exists.
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[importer.clone(), target.clone()]);
    let result = resolver.resolve_all(&[importer, target]);

    let call = result
        .edges
        .iter()
        .find(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol == "src/utils.py::helper"
        })
        .expect("expected a call edge for the imported helper");
    assert_eq!(call.target_file, "src/utils.py");
    assert!(
        matches!(
            call.resolution.as_deref(),
            Some(Resolution::ImportScoped { .. })
        ),
        "imported helper should resolve through the importer binding: {:?}",
        call.resolution
    );
}

#[test]
fn test_aliased_named_import_resolves_to_local_call_name() {
    let importer = extract_file(
        "src/main.ts",
        "import { helper as runHelper } from './utils';\nfunction run() { runHelper(); }\n",
    );
    let target = extract_file("src/utils.ts", "export function helper() {}\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[importer.clone(), target.clone()]);
    let result = resolver.resolve_all(&[importer, target]);
    let call = result
        .edges
        .iter()
        .find(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol == "src/utils.ts::helper"
        })
        .expect("expected aliased local call to resolve");

    assert_eq!(call.target_file, "src/utils.ts");
    assert!(matches!(
        call.resolution.as_deref(),
        Some(Resolution::ImportScoped { .. })
    ));
}

#[test]
fn test_multiple_named_import_aliases_remain_independent() {
    // closes: G18
    let importer = extract_file(
        "src/main.ts",
        "import { first as runFirst, second as runSecond } from './utils';\n\
         function run() { runFirst(); runSecond(); }\n",
    );
    let target = extract_file(
        "src/utils.ts",
        "export function first() {}\nexport function second() {}\n",
    );

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[importer.clone(), target.clone()]);
    let result = resolver.resolve_all(&[importer, target]);

    for (local_call, exported_symbol) in [("runFirst", "first"), ("runSecond", "second")] {
        let call = result
            .edges
            .iter()
            .find(|edge| {
                edge.edge_kind == EdgeKind::Calls
                    && edge.source_file == "src/main.ts"
                    && edge.target_symbol == format!("src/utils.ts::{exported_symbol}")
            })
            .unwrap_or_else(|| panic!("missing aliased call for {local_call}"));
        assert_eq!(call.target_file, "src/utils.ts");
        assert!(matches!(
            call.resolution.as_deref(),
            Some(Resolution::ImportScoped { .. })
        ));
    }
}

#[test]
fn test_default_import_resolves_to_local_binding() {
    let importer = extract_file(
        "src/main.ts",
        "import Worker from './worker';\nfunction run() { Worker(); }\n",
    );
    let target = extract_file("src/worker.ts", "export function Worker() {}\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[importer.clone(), target.clone()]);
    let result = resolver.resolve_all(&[importer, target]);
    let call = result
        .edges
        .iter()
        .find(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol == "src/worker.ts::Worker"
        })
        .expect("default import should resolve to the target symbol");

    assert_eq!(call.target_file, "src/worker.ts");
    assert!(matches!(
        call.resolution.as_deref(),
        Some(Resolution::ImportScoped { .. })
    ));
}

#[test]
fn test_g7_unique_global_resolution_never_crosses_language_families() {
    // closes: G7. A Python call cannot inherit a unique-global TypeScript
    // candidate merely because the names happen to match.
    let python = extract_file("app.py", "def run():\n    SharedComponent()\n");
    let typescript = extract_file("web/component.ts", "export class SharedComponent {}\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[python.clone(), typescript.clone()]);
    let result = resolver.resolve_all(&[python, typescript]);

    assert!(
        !result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "app.py"
                && edge.target_file == "web/component.ts"
        }),
        "global resolution must be constrained to the caller language family"
    );
}

#[test]
fn test_g5_ambiguous_global_resolution_preserves_all_candidates() {
    // closes: G5/G6. Ambiguity must fan out honestly or abstain; selecting
    // the lexicographically first candidate makes the graph confidently wrong.
    let first = extract_file("a.py", "def helper():\n    return 1\n");
    let second = extract_file("b.py", "def helper():\n    return 2\n");
    let caller = extract_file("main.py", "def run():\n    helper()\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[first.clone(), second.clone(), caller.clone()]);
    let result = resolver.resolve_all(&[first, second, caller]);
    let candidates: std::collections::BTreeSet<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "main.py"
                && edge.target_symbol == format!("{}::helper", edge.target_file)
        })
        .map(|edge| edge.target_file.as_str())
        .collect();

    assert_eq!(candidates, ["a.py", "b.py"].into_iter().collect());
    assert!(result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "main.py"
                && edge.target_symbol == format!("{}::helper", edge.target_file)
        })
        .all(|edge| {
            edge.confidence == Confidence::SPECULATIVE
                && matches!(
                    edge.resolution.as_deref(),
                    Some(Resolution::AmbiguousGlobal { .. })
                )
        }));
}

#[test]
fn tsx_relative_imports_use_the_jsts_resolution_ladder() {
    let importer = extract_file(
        "src/view.tsx",
        "import { helper } from './utils';\nexport function View() { return helper(); }\n",
    );
    let target = extract_file("src/utils.ts", "export function helper() { return 1; }\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[importer.clone(), target.clone()]);
    let result = resolver.resolve_all(&[importer, target]);

    assert!(result.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Imports
            && edge.source_file == "src/view.tsx"
            && edge.target_file == "src/utils.ts"
    }));
}

#[test]
fn rust_self_and_super_imports_probe_relative_modules() {
    let lib = extract_file(
        "src/lib.rs",
        "use self::worker::run;\nfn start() { run(); }\n",
    );
    let worker = extract_file("src/worker.rs", "pub fn run() {}\n");
    let nested = extract_file(
        "src/nested/mod.rs",
        "use super::worker::run;\nfn nested() { run(); }\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[lib.clone(), worker.clone(), nested.clone()]);
    let result = resolver.resolve_all(&[lib, worker, nested]);

    for source in ["src/lib.rs", "src/nested/mod.rs"] {
        assert!(result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Imports
                && edge.source_file == source
                && edge.target_file == "src/worker.rs"
        }));
    }
}

#[test]
fn constructor_assignment_drives_receiver_resolution_not_method_name() {
    let source = extract_file(
        "worker.py",
        "class Worker:\n    def run(self):\n        return 1\n\ndef main():\n    worker = Worker()\n    return worker.run()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&source));
    let result = resolver.resolve_all(&[source]);

    assert_eq!(
        result.receiver_types.get("worker.py:worker"),
        Some(&"Worker".to_string()),
        "receiver type must come from `worker = Worker()`, never from `worker.run()`"
    );
    let call = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("run"))
        .expect("worker.run() should resolve to Worker.run");
    assert_eq!(call.confidence, Confidence::DETERMINISTIC);
    assert_eq!(call.target_file, "worker.py");
}

#[test]
fn receiver_resolution_crosses_files_only_through_the_inferred_type() {
    let implementation = extract_file(
        "worker_types.py",
        "class Worker:\n    def run(self):\n        return 1\n",
    );
    let caller = extract_file(
        "app.py",
        "def main():\n    worker = Worker()\n    return worker.run()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[implementation.clone(), caller.clone()]);
    let result = resolver.resolve_all(&[implementation, caller]);

    let call = result
        .edges
        .iter()
        .find(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "app.py"
                && edge.target_symbol.ends_with("run")
        })
        .expect("worker.run() should resolve across files through Worker");
    assert_eq!(call.target_file, "worker_types.py");
    assert_eq!(call.confidence, Confidence::DETERMINISTIC);
}

#[test]
fn ambiguous_constructor_types_do_not_seed_receiver_resolution() {
    let first = extract_file(
        "first.py",
        "class Worker:\n    def run(self):\n        return 1\n",
    );
    let second = extract_file(
        "second.py",
        "class Worker:\n    def run(self):\n        return 2\n",
    );
    let caller = extract_file(
        "app.py",
        "def main():\n    worker = Worker()\n    return worker.run()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[first.clone(), second.clone(), caller.clone()]);
    let result = resolver.resolve_all(&[first, second, caller]);

    assert_eq!(result.receiver_types.get("app.py:worker"), None);
    assert!(result.edges.iter().all(|edge| {
        !(edge.edge_kind == EdgeKind::Calls
            && edge.source_file == "app.py"
            && edge.target_symbol.ends_with("run")
            && edge.confidence == Confidence::DETERMINISTIC)
    }));
}

#[test]
fn duplicate_same_file_methods_do_not_become_a_deterministic_bare_call() {
    let source = extract_file(
        "ambiguous.py",
        "class First:\n    def run(self): pass\n\nclass Second:\n    def run(self): pass\n\ndef invoke():\n    run()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&source));
    let result = resolver.resolve_all(&[source]);

    assert!(result.edges.iter().all(|edge| {
        !(edge.edge_kind == EdgeKind::Calls
            && edge.source_file == "ambiguous.py"
            && edge.target_symbol.ends_with("run")
            && edge.confidence == Confidence::DETERMINISTIC)
    }));
}

#[test]
fn repeated_call_sites_do_not_duplicate_the_same_graph_edge() {
    let source = extract_file(
        "repeat.py",
        "def helper(): pass\n\ndef caller():\n    helper()\n    helper()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&source));
    let result = resolver.resolve_all(&[source]);
    let duplicates = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "repeat.py"
                && edge.target_symbol == "repeat.py::helper"
        })
        .count();
    assert_eq!(duplicates, 1, "logical graph edges must be deduplicated");
}

#[test]
fn route_handler_prefers_its_own_file_over_index_order() {
    let earlier = extract_file("a.py", "def handler():\n    return 'wrong'\n");
    let mut routed = extract_file(
        "z.py",
        "from fastapi import FastAPI\napp = FastAPI()\n@app.get('/items')\ndef handler():\n    return 'right'\n",
    );
    routed.routes.push(ExtractedRoute {
        framework: "fastapi".to_string(),
        http_method: "GET".to_string(),
        path_pattern: "/items".to_string(),
        handler_name: "handler".to_string(),
        span: Span {
            start_byte: 0,
            end_byte: 1,
        },
    });
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[earlier.clone(), routed.clone()]);
    let result = resolver.resolve_all(&[earlier, routed]);
    let route = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::HandlesRoute)
        .expect("route edge");
    assert_eq!(route.source_file, "z.py");
    assert_eq!(route.target_file, "z.py");
    assert!(matches!(
        route.resolution.as_deref(),
        Some(Resolution::SameFile { .. })
    ));
}

/// D17 / R5: a call no ladder rung resolves must be recorded, not dropped.
///
/// Before this, an unattributable call produced no edge and no ledger entry, so
/// "we could not resolve this call" was byte-for-byte identical to "there is no
/// call here" — and every liveness conclusion drawn downstream inherited that
/// silence.
#[test]
fn unresolvable_calls_are_recorded_rather_than_silently_dropped() {
    let source = extract_file(
        "app.py",
        "def run():\n    definitely_not_defined_anywhere()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&source));
    let result = resolver.resolve_all(&[source]);

    assert!(
        !result
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Calls),
        "an unresolved call must not invent a graph edge to a target that does not exist"
    );
    let entry = result
        .unresolved
        .iter()
        .find(|entry| entry.callee_name == "definitely_not_defined_anywhere")
        .expect("the unresolved call must appear in the ledger");
    assert_eq!(entry.source_file, "app.py");
    assert_eq!(
        entry.source_symbol, "app.py::run",
        "the ledger must attribute the call to its calling symbol"
    );
    assert!(
        matches!(&entry.resolution, Resolution::Unresolved { reason } if !reason.is_empty()),
        "the ledger entry must carry a non-empty reason: {:?}",
        entry.resolution
    );
}

/// SC9: two Go types sharing a receiver variable name must not cross-resolve.
///
/// `receiver_types` was keyed by (file, variable name) with last-write-wins, so
/// `func (s *A)` and `func (s *B)` in one file — ordinary Go — collided on the
/// key `file:s`. Every `s.method()` in the file then resolved against whichever
/// type was indexed last, and was stamped DETERMINISTIC. That is worse than
/// failing to resolve: a real method with real callers was reported confidently
/// dead while a fabricated full-confidence edge pointed at its namesake, and
/// high-confidence edges are exactly what consumers treat as proof of liveness.
///
/// Fails against the pre-fix tree: both calls land on `MemStore.readList`.
#[test]
fn shared_receiver_names_do_not_cross_resolve_between_types() {
    let source = r#"
package store

type RedisStore struct{ items []string }
type MemStore struct{ items []string }

func (s *RedisStore) readList(key string) []string { return s.items }

func (s *RedisStore) uniqueToRedis(key string) []string {
	return s.readList(key)
}

func (s *MemStore) readList(key string) []string { return s.items }

func (s *MemStore) uniqueToMem(key string) []string {
	return s.readList(key)
}
"#;
    let ext = devmap_extract::extract_file("store.go", source);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let result = resolver.resolve_all(std::slice::from_ref(&ext));

    let target_of = |caller: &str| -> Option<String> {
        result
            .edges
            .iter()
            .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol.ends_with(caller))
            .map(|edge| edge.target_symbol.clone())
    };

    assert_eq!(
        target_of("uniqueToRedis").as_deref(),
        Some("store.go::RedisStore.readList"),
        "a RedisStore method calling s.readList() must reach RedisStore's own \
         readList, not its namesake on another type: {:?}",
        result
            .edges
            .iter()
            .map(|e| (&e.source_symbol, &e.target_symbol, e.confidence.0))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        target_of("uniqueToMem").as_deref(),
        Some("store.go::MemStore.readList"),
        "a MemStore method calling s.readList() must reach MemStore's own readList"
    );

    // Both must be confident. Resolving to the right type only by luck of
    // ambiguity would leave these speculative.
    for edge in result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.contains("readList"))
    {
        assert_eq!(
            edge.confidence,
            Confidence::DETERMINISTIC,
            "receiver-typed call {} -> {} should resolve deterministically",
            edge.source_symbol,
            edge.target_symbol
        );
    }
}

/// A Go method's call edges must name a source symbol that a node actually has.
///
/// Go callables previously reported the bare `file::method` as `caller_symbol`,
/// while the method's own node identity is `file::Type.method`. Every Go method
/// call edge therefore named a source that matched no node, so the edge could
/// not be joined back to the symbol it came from — and two types' same-named
/// methods were indistinguishable as scopes, which is what SC9 rode in on.
///
/// Fails against the pre-fix tree: the source symbol is `store.go::caller`.
#[test]
fn go_method_call_edges_name_their_receiver_qualified_source() {
    let source = r#"
package store

type Widget struct{ n int }

func (w *Widget) helper() int { return w.n }

func (w *Widget) caller() int { return w.helper() }
"#;
    let ext = devmap_extract::extract_file("store.go", source);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let result = resolver.resolve_all(std::slice::from_ref(&ext));

    let edge = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("helper"))
        .expect("the helper call must produce an edge");

    assert_eq!(
        edge.source_symbol, "store.go::Widget.caller",
        "the edge source must be the caller's node identity, receiver-qualified"
    );

    // And that identity must actually exist as a symbol, or the edge is orphaned.
    assert!(
        ext.symbols
            .iter()
            .any(|symbol| symbol.qualified_name == edge.source_symbol),
        "edge source {} matches no symbol in the file: {:?}",
        edge.source_symbol,
        ext.symbols
            .iter()
            .map(|s| &s.qualified_name)
            .collect::<Vec<_>>()
    );
}

/// SC10: every call edge must name a source symbol that some node actually has.
///
/// `callable_binding_name` accepted kinds the symbol emitter did not, so a call
/// made inside one of them was attributed to a scope with no corresponding
/// node. Those edges are unjoinable: a traversal starting from the enclosing
/// symbol never finds them, and the call silently disappears from the graph.
/// Two shapes did this — `function*`/`async function*` declarations, which were
/// simply never emitted, and named function *expressions*, whose internal name
/// is only in scope inside themselves and is deliberately not a symbol.
///
/// Fails against the pre-fix tree with `js.ts::patchedLookup` and
/// `js.ts::streamIt` as orphaned sources.
#[test]
fn every_call_edge_names_a_source_symbol_that_exists() {
    let source = r#"
export async function* streamIt(items: string[]) {
  for (const item of items) {
    yield transform(item);
  }
}

function transform(value: string): string {
  return value.trim();
}

const target: Record<string, unknown> = {};
target.lookup = function patchedLookup(host: string) {
  return transform(host);
};

export function makeResolver() {
  return function resolveOnce(options: string) {
    return transform(options);
  };
}
"#;
    let ext = devmap_extract::extract_file("js.ts", source);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let result = resolver.resolve_all(std::slice::from_ref(&ext));

    let identities: std::collections::BTreeSet<&str> = ext
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    let orphans: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .filter(|edge| !identities.contains(edge.source_symbol.as_str()))
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect();

    assert!(
        orphans.is_empty(),
        "call edges name sources that match no symbol, so they cannot be \
         traversed from the node they belong to: {orphans:?}; known symbols: {identities:?}"
    );

    // The generator declaration is a real, exported declaration and must be a
    // node in its own right, not merely a scope label.
    assert!(
        identities.contains("js.ts::streamIt"),
        "an exported `async function*` declaration must be extracted as a symbol: {identities:?}"
    );
}

/// SC12: a parameter's declared type is a receiver binding.
///
/// Two defects met here. Rust method calls were recorded with the whole dotted
/// chain as the callee and no receiver at all (`s.read_list` rather than
/// callee `read_list` on receiver `s`), so receiver-type resolution could never
/// fire for Rust. And receiver typing only ever learned from constructor
/// assignments, so a function operating on a value it did not construct had no
/// binding for its own parameter. Together these meant the call did not resolve
/// weakly — it produced **no edge at all** and vanished from the graph.
///
/// Fails against the pre-fix tree: no Calls edge exists for `first_item`.
#[test]
fn a_parameters_declared_type_resolves_its_method_calls() {
    let source = r#"
pub struct Store { items: Vec<String> }

impl Store {
    fn read_list(&self) -> Vec<String> { self.items.clone() }
}

pub fn first_item(s: &Store) -> String {
    s.read_list().into_iter().next().unwrap_or_default()
}
"#;
    let ext = devmap_extract::extract_file("lib.rs", source);

    // The call must be split into receiver + method before anything can resolve.
    let call = ext
        .calls
        .iter()
        .find(|call| call.callee_name == "read_list")
        .expect("a Rust method call must record the method as its callee, not the whole chain");
    assert_eq!(
        call.receiver_expr.as_deref(),
        Some("s"),
        "the receiver expression must be captured for receiver typing"
    );

    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let result = resolver.resolve_all(std::slice::from_ref(&ext));

    let edge = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("read_list"))
        .unwrap_or_else(|| {
            panic!(
                "the call through a typed parameter produced no edge: {:?}",
                result
                    .edges
                    .iter()
                    .map(|e| (&e.source_symbol, &e.target_symbol))
                    .collect::<Vec<_>>()
            )
        });

    assert_eq!(edge.source_symbol, "lib.rs::first_item");
    assert_eq!(edge.target_symbol, "lib.rs::Store.read_list");
    assert_eq!(
        edge.confidence,
        Confidence::DETERMINISTIC,
        "a declared parameter type is an exact binding, not a guess"
    );
}

/// SC18. A call that failed the ladder used to be recorded identically whether
/// it was `len` (which no file can ever declare), `strings.TrimSpace` (which the
/// import proves is outside the corpus), or a genuine resolution failure. 380k
/// rows of the first two buried the third, which is why the SC17 extraction bugs
/// survived every gate — they were only found by hand-reading the top-N.
///
/// The classification must be drawn from evidence, and must fail toward
/// `Unresolved` so it can never hide a defect behind an "expected" label.
#[test]
fn unresolved_calls_separate_builtins_and_external_imports_from_real_failures() {
    let go = extract_file(
        "svc/main.go",
        concat!(
            "package main\n",
            "import \"strings\"\n",
            "func run(xs []string) {\n",
            "\t_ = len(xs)\n",
            "\t_ = strings.TrimSpace(xs[0])\n",
            "\tmysteryHelper()\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&go));
    let res = resolver.resolve_all(std::slice::from_ref(&go));

    let class_of = |callee: &str| {
        res.unresolved
            .iter()
            .find(|u| u.callee_name == callee)
            .unwrap_or_else(|| {
                panic!(
                    "{callee:?} must be recorded; got {:?}",
                    res.unresolved
                        .iter()
                        .map(|u| (&u.callee_name, &u.class))
                        .collect::<Vec<_>>()
                )
            })
            .class
            .clone()
    };

    // A language builtin: declared by Go itself, so no file can declare it.
    assert_eq!(class_of("len"), UnresolvedClass::Builtin);

    // The `strings` import resolved to no indexed file, which is the evidence.
    assert_eq!(
        class_of("TrimSpace"),
        UnresolvedClass::External {
            module: "strings".to_string()
        }
    );

    // No evidence either way: this is the only tier that means "possible bug",
    // and it must stay visible.
    assert_eq!(class_of("mysteryHelper"), UnresolvedClass::Unresolved);
}

/// The classifier must never launder a real failure into an expected one.
///
/// A method that merely *shares a name* with a builtin is library API, and a
/// receiver whose module was never imported proves nothing.
#[test]
fn builtin_names_behind_a_receiver_are_not_classified_as_builtins() {
    let go = extract_file(
        "svc/list.go",
        concat!(
            "package main\n",
            "func run(c Collection, t Thing) {\n",
            // `len` is a builtin, but `c.len()` is a method call on a value.
            "\tc.len()\n",
            // No import named `t` exists, so nothing is proven about it.
            "\tt.Fatalf(\"x\")\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&go));
    let res = resolver.resolve_all(std::slice::from_ref(&go));

    for callee in ["len", "Fatalf"] {
        let found = res.unresolved.iter().find(|u| u.callee_name == callee);
        if let Some(reference) = found {
            // The load-bearing claim: a name that merely *matches* a builtin
            // must never be laundered into an expected tier just because the
            // spelling collides. `c.len()` is a method on a value.
            assert_ne!(
                reference.class,
                UnresolvedClass::Builtin,
                "{callee:?} has a receiver, so it is library API, not a builtin"
            );
            // And no import binds `c` or `t` or their types, so nothing proves
            // they are external either.
            assert_eq!(
                reference.class,
                UnresolvedClass::UninferredReceiver,
                "{callee:?} has an untypeable receiver: a known limitation, \
                 recorded as such rather than claimed to be external"
            );
        }
    }
}

/// SC25. `t.Fatalf()` is not a resolution defect — `t` is a `*testing.T`, and
/// `testing` is an import that resolved to no indexed file, so the method
/// belongs to a package this corpus will never contain. Before receiver-type
/// inference it was indistinguishable from a genuine bare-name failure, and at
/// 9,181 rows it was the single largest entry in the actionable tier.
///
/// The evidence chain is: receiver `t` → declared type `T` → `T` is bound by an
/// external import → External. Nothing here guesses from the shape of a name.
#[test]
fn a_receiver_typed_by_an_external_import_is_classified_external() {
    let go = extract_file(
        "svc/thing_test.go",
        concat!(
            "package svc\n",
            "import \"testing\"\n",
            "func TestThing(t *testing.T) {\n",
            "\tt.Fatalf(\"boom\")\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&go));
    let res = resolver.resolve_all(std::slice::from_ref(&go));

    let fatalf = res
        .unresolved
        .iter()
        .find(|u| u.callee_name == "Fatalf")
        .unwrap_or_else(|| {
            panic!(
                "Fatalf must be recorded; got {:?}",
                res.unresolved
                    .iter()
                    .map(|u| (&u.callee_name, &u.class))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        fatalf.class,
        UnresolvedClass::External {
            module: "testing".to_string()
        },
        "a method on a value whose declared type comes from an external import \
         is external, not a defect"
    );
}

/// The inference must not fire when the declared type is a *local* one.
///
/// A method on a type this corpus does declare, that still did not resolve, is
/// a real signal — laundering it as `External` because some unrelated import
/// exists in the file would hide exactly the defect this tier is for.
#[test]
fn a_receiver_typed_by_a_local_type_is_never_called_external() {
    let go = extract_file(
        "svc/local.go",
        concat!(
            "package svc\n",
            "import \"testing\"\n",
            "type Widget struct{}\n",
            "func run(w *Widget) {\n",
            "\tw.MissingMethod()\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&go));
    let res = resolver.resolve_all(std::slice::from_ref(&go));

    if let Some(reference) = res
        .unresolved
        .iter()
        .find(|u| u.callee_name == "MissingMethod")
    {
        assert_ne!(
            reference.class,
            UnresolvedClass::External {
                module: "testing".to_string()
            },
            "an unrelated import in the same file must not make a local type's \
             method look external"
        );
    }
}

/// SC25 hardening. The classifier's tiers must be *structurally* exclusive, not
/// merely usually right, because each one licenses a different conclusion:
/// `builtin`/`external` mean "expected", `uninferred_receiver` means "known
/// limitation", and only `unresolved` means "possible defect".
///
/// Two invariants make the tiers auditable:
///   - a call with a receiver is never `Builtin` (a method that merely shares a
///     builtin's spelling is library API, not a builtin);
///   - a bare call is never `UninferredReceiver` (there is no receiver to fail
///     to infer);
///   - a call with a receiver is never `HostGlobal` (a host global is a
///     property of the global object, so `x.setTimeout()` is a method on `x`),
///     and a `HostGlobal` only ever comes from a JavaScript-family file, since
///     no other family has a global object for these names to live on;
///   - a call with a receiver is never `LocalBinding` (the claim is about the
///     callee itself being a value in scope, which a method name is not), and
///     every `LocalBinding` names a scope that is not the file itself.
///
/// Asserted over a corpus that exercises every arm rather than one example.
#[test]
fn classification_tiers_are_structurally_exclusive() {
    let sources: &[(&str, &str)] = &[
        // Bare host globals, a host global used as a method, and a bare call
        // to this function's own constructed binding.
        (
            "web/host.ts",
            "export function run(x: any) {\n  setTimeout(() => {}, 1);\n  \
             fetch('/x');\n  x.setTimeout(1);\n  mysteryHost();\n}\n",
        ),
        // A Rust parameter invoked as a callback, alongside a bare call the
        // scope does *not* declare.
        (
            "lib/e.rs",
            "pub struct Handler;\nfn takes(handler: Handler) {\n    handler();\n}\n\
             fn other() {\n    handler();\n}\n",
        ),
        (
            "svc/a.go",
            "package svc\nimport \"strings\"\nimport \"testing\"\ntype W struct{}\n\
             func run(t *testing.T, w *W, xs []string) {\n\
             \t_ = len(xs)\n\t_ = strings.TrimSpace(\"x\")\n\tt.Fatalf(\"b\")\n\
             \tw.Missing()\n\tmysteryFn()\n\txs[0].Weird()\n}\n",
        ),
        (
            "app/b.py",
            "import os\nfrom pathlib import Path\n\
             def run(p):\n    print(p)\n    p.write_text('x')\n    os.getcwd()\n    mystery()\n",
        ),
        (
            "web/c.ts",
            "import { useState } from 'react';\n\
             export function run(v: string) {\n  useState(0);\n  v.toLowerCase();\n  \
             parseInt(v);\n  mystery();\n}\n",
        ),
        (
            "lib/d.rs",
            "fn run(v: String) {\n    println!(\"{}\", v);\n    let _ = v.to_string();\n    \
             mystery();\n}\n",
        ),
    ];
    let extractions: Vec<_> = sources
        .iter()
        .map(|(path, src)| extract_file(path, src))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let res = resolver.resolve_all(&extractions);

    assert!(
        !res.unresolved.is_empty(),
        "the fixture must actually produce unresolved calls"
    );
    for reference in &res.unresolved {
        match (&reference.class, &reference.receiver) {
            (UnresolvedClass::Builtin, Some(receiver)) => panic!(
                "{}: `{}` has receiver {receiver:?} and cannot be a builtin",
                reference.source_file, reference.callee_name
            ),
            (UnresolvedClass::UninferredReceiver, None) => panic!(
                "{}: `{}` is a bare call, so there is no receiver to infer",
                reference.source_file, reference.callee_name
            ),
            (UnresolvedClass::HostGlobal { .. }, Some(receiver)) => panic!(
                "{}: `{}` has receiver {receiver:?}, so it is a method on that \
                 value and not a property of the global object",
                reference.source_file, reference.callee_name
            ),
            (UnresolvedClass::LocalBinding, Some(receiver)) => panic!(
                "{}: `{}` has receiver {receiver:?}; a scope binding the name \
                 `{}` says nothing about a method called on {receiver:?}",
                reference.source_file, reference.callee_name, reference.callee_name
            ),
            _ => {}
        }
        if let UnresolvedClass::HostGlobal { environment } = &reference.class {
            assert!(
                reference.source_file.ends_with(".ts")
                    || reference.source_file.ends_with(".tsx")
                    || reference.source_file.ends_with(".js")
                    || reference.source_file.ends_with(".jsx"),
                "{}: `{}` was called a {environment} host global, but only the \
                 JavaScript family has a global object",
                reference.source_file,
                reference.callee_name
            );
        }
        if reference.class == UnresolvedClass::LocalBinding {
            assert_ne!(
                reference.source_symbol, reference.source_file,
                "{}: `{}` was attributed to the file itself, so there is no \
                 enclosing scope that could have declared it",
                reference.source_file, reference.callee_name
            );
        }
    }

    // And the tiering is not vacuous — every tier is reachable from this corpus.
    let labels: std::collections::BTreeSet<&str> =
        res.unresolved.iter().map(|u| u.class.label()).collect();
    for expected in [
        "builtin",
        "external",
        "host_global",
        "local_binding",
        "uninferred_receiver",
        "unresolved",
    ] {
        assert!(
            labels.contains(expected),
            "tier {expected:?} was never produced; got {labels:?}"
        );
    }
}

/// A runtime-supplied global gets its own tier, with its runtime cited.
///
/// These were left in the defect tier on purpose until now, because folding
/// them into `Builtin` would claim ECMA-262 declares `setTimeout`, which it does
/// not. The fix is a tier with a different authority, not a bigger builtin
/// table — so this asserts both halves: the classification, and that the
/// language tier still refuses them.
#[test]
fn a_runtime_supplied_global_is_its_own_tier_and_never_a_language_builtin() {
    let ts = extract_file(
        "web/timers.ts",
        concat!(
            "export function run() {\n",
            "  const id = setTimeout(() => {}, 10);\n",
            "  clearTimeout(id);\n",
            "  fetch('/x');\n",
            "  structuredClone({});\n",
            "  setImmediate(() => {});\n",
            "  mysteryHelper();\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ts));
    let res = resolver.resolve_all(std::slice::from_ref(&ts));

    let class_of = |callee: &str| {
        res.unresolved
            .iter()
            .find(|u| u.callee_name == callee)
            .unwrap_or_else(|| {
                panic!(
                    "{callee:?} must be recorded; got {:?}",
                    res.unresolved
                        .iter()
                        .map(|u| (&u.callee_name, &u.class))
                        .collect::<Vec<_>>()
                )
            })
            .class
            .clone()
    };

    for (callee, environment) in [
        ("setTimeout", "web+node"),
        ("clearTimeout", "web+node"),
        ("fetch", "web+node"),
        ("structuredClone", "web+node"),
        ("setImmediate", "node"),
    ] {
        assert_eq!(
            class_of(callee),
            UnresolvedClass::HostGlobal {
                environment: environment.to_string()
            },
            "{callee:?} is supplied by a runtime, and the tier must cite which"
        );
    }

    // The tier is a narrowing, not a widening: a name with no authority behind
    // it stays exactly where it was.
    assert_eq!(class_of("mysteryHelper"), UnresolvedClass::Unresolved);
}

/// An import in the file outranks the global name list.
///
/// `import { fetch } from 'node-fetch'` is evidence about *this file*, and a
/// name list is evidence about a runtime that may not even be the one in use.
/// The stronger, file-specific evidence has to win, or the module recorded
/// against the call would be a guess.
#[test]
fn an_import_outranks_the_host_global_table() {
    let ts = extract_file(
        "web/polyfill.ts",
        concat!(
            "import { fetch } from 'node-fetch';\n",
            "export function run() {\n",
            "  fetch('/x');\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ts));
    let res = resolver.resolve_all(std::slice::from_ref(&ts));

    let fetched = res
        .unresolved
        .iter()
        .find(|u| u.callee_name == "fetch")
        .expect("the call must be recorded");
    assert_eq!(
        fetched.class,
        UnresolvedClass::External {
            module: "node-fetch".to_string()
        },
        "an explicit import names where the binding came from; the host-global \
         table only knows what some runtime would have supplied"
    );
}

/// A bare call to a value the calling function itself declares is not a defect.
///
/// `handler(...)` inside a function whose own parameter list declares
/// `handler` is a callback invocation. There is no cross-file symbol to find,
/// so the ladder failing is the correct outcome and reporting it as a probable
/// bug is noise — this was the largest single shape left in the defect tier.
#[test]
fn a_call_to_the_enclosing_symbols_own_parameter_is_a_local_binding() {
    let rust = extract_file(
        "lib/callback.rs",
        concat!(
            "pub struct Handler;\n",
            "fn takes(handler: Handler) {\n",
            "    handler();\n",
            "}\n",
        ),
    );
    let go = extract_file(
        "svc/callback.go",
        concat!(
            "package svc\n",
            "type Callback struct{}\n",
            "func run(cb Callback) {\n",
            "\tcb()\n",
            "}\n",
        ),
    );
    for extraction in [rust, go] {
        let mut resolver = Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&extraction));
        let res = resolver.resolve_all(std::slice::from_ref(&extraction));

        let callee = if extraction.file_path.ends_with(".rs") {
            "handler"
        } else {
            "cb"
        };
        let call = res
            .unresolved
            .iter()
            .find(|u| u.callee_name == callee)
            .unwrap_or_else(|| {
                panic!(
                    "{}: {callee:?} must be recorded; got {:?}",
                    extraction.file_path,
                    res.unresolved
                        .iter()
                        .map(|u| (&u.callee_name, &u.class))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            call.class,
            UnresolvedClass::LocalBinding,
            "{}: {callee:?} is this function's own parameter, invoked as a \
             callback",
            extraction.file_path
        );
    }
}

/// A parameter shadows the language, so the scope is asked first.
///
/// Go permits `func run(len Counter)`, and inside that function `len(...)`
/// calls the parameter, not the predeclared builtin. Answering `Builtin` there
/// would be the right label for the wrong reason — and the wrong reason is what
/// makes a classification untrustworthy.
#[test]
fn a_parameter_shadowing_a_builtin_is_classified_by_its_scope() {
    let go = extract_file(
        "svc/shadow.go",
        concat!(
            "package svc\n",
            "type Counter struct{}\n",
            "func run(len Counter) {\n",
            "\tlen()\n",
            "}\n",
            "func other(xs []string) {\n",
            "\t_ = len(xs)\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&go));
    let res = resolver.resolve_all(std::slice::from_ref(&go));

    let in_scope = |scope: &str| {
        res.unresolved
            .iter()
            .find(|u| u.callee_name == "len" && u.source_symbol.ends_with(scope))
            .unwrap_or_else(|| {
                panic!(
                    "a `len` call in {scope:?} must be recorded; got {:?}",
                    res.unresolved
                        .iter()
                        .map(|u| (&u.source_symbol, &u.callee_name, &u.class))
                        .collect::<Vec<_>>()
                )
            })
            .class
            .clone()
    };

    assert_eq!(
        in_scope("::run"),
        UnresolvedClass::LocalBinding,
        "inside `run`, `len` is the parameter"
    );
    assert_eq!(
        in_scope("::other"),
        UnresolvedClass::Builtin,
        "inside `other`, nothing shadows the predeclared `len`"
    );
}

/// The SC9/SC25 defect shape, applied to the local-binding lookup.
///
/// A binding belongs to one scope. `takes` declaring a `handler` parameter says
/// nothing about a bare `handler()` in a *different* function of the same file,
/// and answering from a file-wide map there would relabel a genuine failure as
/// expected — the same mistake that once declared a local type's method
/// external at full confidence, in a third place.
///
/// The file-wide binding demonstrably exists here (`takes` writes it), so this
/// fails the moment the lookup falls back to it.
#[test]
fn a_local_binding_does_not_leak_between_functions_in_one_file() {
    let rust = extract_file(
        "lib/scope.rs",
        concat!(
            "pub struct Handler;\n",
            "fn takes(handler: Handler) {\n",
            "    handler();\n",
            "}\n",
            "fn other() {\n",
            "    handler();\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&rust));
    let res = resolver.resolve_all(std::slice::from_ref(&rust));

    let in_scope = |scope: &str| {
        res.unresolved
            .iter()
            .find(|u| u.callee_name == "handler" && u.source_symbol.ends_with(scope))
            .unwrap_or_else(|| {
                panic!(
                    "a `handler` call in {scope:?} must be recorded; got {:?}",
                    res.unresolved
                        .iter()
                        .map(|u| (&u.source_symbol, &u.callee_name, &u.class))
                        .collect::<Vec<_>>()
                )
            })
            .class
            .clone()
    };

    assert_eq!(in_scope("::takes"), UnresolvedClass::LocalBinding);
    assert_eq!(
        in_scope("::other"),
        UnresolvedClass::Unresolved,
        "`other` declares no `handler`; inheriting the binding `takes` wrote \
         would hide exactly the failure this tier exists to surface"
    );
}

/// SC25 hardening, applying the SC9 lesson to the new map.
///
/// Two functions in one file may reuse a parameter name for different types.
/// If `t` is a `*testing.T` in one and a local `*Tracker` in the other, the
/// external classification must not leak across — that would be the SC9 defect
/// in a new place: a confident claim produced by whichever binding was indexed
/// last.
#[test]
fn declared_type_bindings_do_not_leak_between_functions_sharing_a_parameter_name() {
    let go = extract_file(
        "svc/shared.go",
        concat!(
            "package svc\n",
            "import \"testing\"\n",
            "type Tracker struct{}\n",
            "func TestOne(t *testing.T) {\n",
            "\tt.Fatalf(\"x\")\n",
            "}\n",
            "func useTracker(t *Tracker) {\n",
            "\tt.RecordMissing()\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&go));
    let res = resolver.resolve_all(std::slice::from_ref(&go));

    if let Some(tracked) = res
        .unresolved
        .iter()
        .find(|u| u.callee_name == "RecordMissing")
    {
        assert_ne!(
            tracked.class,
            UnresolvedClass::External {
                module: "testing".to_string()
            },
            "a `*Tracker` receiver must not inherit the `*testing.T` binding \
             from another function that happens to reuse the name `t`"
        );
    }
}

/// The receiver is recorded for every method call and absent for every bare
/// call. Without it the tiering above cannot be audited from the store, which
/// is the whole reason the column exists.
#[test]
fn the_receiver_is_recorded_exactly_when_the_call_has_one() {
    let go = extract_file(
        "svc/rec.go",
        "package svc\nimport \"strings\"\nfunc run() {\n\tstrings.TrimSpace(\"x\")\n\tmystery()\n}\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&go));
    let res = resolver.resolve_all(std::slice::from_ref(&go));

    let trim = res
        .unresolved
        .iter()
        .find(|u| u.callee_name == "TrimSpace")
        .expect("TrimSpace must be recorded");
    assert_eq!(trim.receiver.as_deref(), Some("strings"));

    let mystery = res
        .unresolved
        .iter()
        .find(|u| u.callee_name == "mystery")
        .expect("mystery must be recorded");
    assert_eq!(
        mystery.receiver, None,
        "a bare call must record no receiver, so that \"no receiver\" stays \
         distinguishable from \"receiver text we failed to capture\""
    );
}

/// SC26. A Rust associated-function call used to record its whole path
/// (`Worker::new`) as the callee, matching no symbol. Splitting it recovers a
/// real edge to a user-defined associated function — this is not noise removal.
///
/// The split must not cost receiver typing: `let w = Worker::new(); w.go();`
/// still has to dispatch `go` onto `Worker`, which is the SC9 machinery.
#[test]
fn rust_associated_function_calls_resolve_and_keep_receiver_typing() {
    let ext = extract_file(
        "f.rs",
        concat!(
            "struct Worker;\n",
            "impl Worker {\n",
            "    fn new() -> Self { Worker }\n",
            "    fn go(&self) {}\n",
            "}\n",
            "fn run() {\n",
            "    let worker = Worker::new();\n",
            "    worker.go();\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let res = resolver.resolve_all(std::slice::from_ref(&ext));

    let targets: Vec<&str> = res
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == "f.rs::run")
        .map(|e| e.target_symbol.as_str())
        .collect();

    assert!(
        targets.contains(&"f.rs::Worker.new"),
        "the associated function call must resolve — this edge did not exist \
         before the split; got {targets:?}"
    );
    assert!(
        targets.contains(&"f.rs::Worker.go"),
        "receiver typing from the constructor assignment must survive the \
         split; got {targets:?}"
    );
}

/// An untyped local binding is classified by its scope, not left in the defect
/// tier.
///
/// `scope_declares_local` could only consult tables that carry a *written type*,
/// so it answered for Rust and Go signatures and for nothing else. On the
/// DevCouncil repository that meant the tier fired on 4 rows out of roughly 50:
/// a Python `cls`, a `let`-bound closure invoked below its own definition, a
/// typed-but-callable parameter — all of them values the scope declares, all of
/// them sitting in the tier that means "possible resolution defect".
///
/// The `Unresolved` half of each case is the load-bearing half. A tier that
/// answers "local" for a scope that binds nothing would hide exactly the
/// failures it exists to surface.
#[test]
fn an_untyped_local_binding_is_classified_by_its_scope() {
    let python = extract_file(
        "svc/factory.py",
        concat!(
            "class Message:\n",
            "    @classmethod\n",
            "    def from_dict(cls, raw):\n",
            "        return cls(raw)\n",
            "\n",
            "def detect(next_gap_id: Callable):\n",
            "    return next_gap_id(\"t\", \"k\")\n",
            "\n",
            "def stray():\n",
            "    return next_gap_id(\"t\", \"k\")\n",
        ),
    );
    let rust = extract_file(
        "lib/handlers.rs",
        concat!(
            "fn run() {\n",
            "    let handler = |name: &str| name.len();\n",
            "    handler(\"x\");\n",
            "}\n",
            "fn other() {\n",
            "    handler(\"y\");\n",
            "}\n",
        ),
    );
    let extractions = vec![python, rust];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let res = resolver.resolve_all(&extractions);

    let class_of = |callee: &str, scope: &str| {
        res.unresolved
            .iter()
            .find(|u| u.callee_name == callee && u.source_symbol.ends_with(scope))
            .unwrap_or_else(|| {
                panic!(
                    "a {callee:?} call in {scope:?} must be recorded; got {:?}",
                    res.unresolved
                        .iter()
                        .map(|u| (&u.source_symbol, &u.callee_name, &u.class))
                        .collect::<Vec<_>>()
                )
            })
            .class
            .clone()
    };

    assert_eq!(
        class_of("cls", "::Message.from_dict"),
        UnresolvedClass::LocalBinding,
        "`cls` is this classmethod's own first parameter"
    );
    assert_eq!(
        class_of("next_gap_id", "::detect"),
        UnresolvedClass::LocalBinding,
        "an annotated parameter is still a parameter"
    );
    assert_eq!(
        class_of("next_gap_id", "::stray"),
        UnresolvedClass::Unresolved,
        "`stray` declares no `next_gap_id`; inheriting the binding `detect` \
         wrote would hide the failure this tier exists to surface"
    );
    assert_eq!(
        class_of("handler", "::run"),
        UnresolvedClass::LocalBinding,
        "a `let`-bound closure is a value this scope declares"
    );
    assert_eq!(
        class_of("handler", "::other"),
        UnresolvedClass::Unresolved,
        "the binding belongs to `run`, not to the file"
    );
}

/// A Python re-export alias resolves the imports that name it.
///
/// `integrate.py` re-exports `_project_root = common._project_root`, and
/// `agents.py` imports `_project_root` from `integrate`. With no symbol behind
/// the alias the import binding named a file that did not declare it, fell off
/// the ladder, and the call resolved instead by ambiguous global fan-out — three
/// speculative 0.2 edges, one of them to the right function by luck. This is the
/// shape that produced 87 such edges on the DevCouncil repository.
#[test]
fn a_python_reexport_alias_resolves_the_import_that_names_it() {
    let common = extract_file(
        "clients/common.py",
        "def _project_root(path):\n    return path\n",
    );
    let integrate = extract_file(
        "cli/integrate.py",
        concat!(
            "from clients import common\n",
            "\n",
            "_project_root = common._project_root\n",
        ),
    );
    let agents = extract_file(
        "cli/agents.py",
        concat!(
            "from cli.integrate import _project_root\n",
            "\n",
            "def add_agent(root):\n",
            "    return _project_root(root)\n",
        ),
    );
    let extractions = vec![common, integrate, agents];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let res = resolver.resolve_all(&extractions);

    let resolved: Vec<(&str, Confidence)> = res
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == "cli/agents.py::add_agent")
        .map(|e| (e.target_symbol.as_str(), e.confidence))
        .collect();

    assert!(
        resolved.iter().any(
            |(target, confidence)| *target == "cli/integrate.py::_project_root"
                && *confidence == Confidence::DETERMINISTIC
        ),
        "the import binding must resolve to the alias the module declares; got {resolved:?}"
    );
}

/// `let x = x()` is a call of the function, not a local of the binding it
/// introduces. The same-named parameter case stays a local, which is the
/// existing RA1 test.
#[test]
fn a_rust_let_initializer_call_is_not_a_local_binding() {
    let extraction = extract_file(
        "inv.rs",
        concat!(
            "fn test_commands() -> u8 { 1 }\n",
            "fn go() {\n",
            "    let test_commands = test_commands();\n",
            "    let _ = test_commands;\n",
            "}\n",
            "fn takes(handler: u8) {\n",
            "    let handler = handler();\n",
            "    let _ = handler;\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&extraction));
    let res = resolver.resolve_all(std::slice::from_ref(&extraction));

    let edge = res
        .edges
        .iter()
        .find(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_symbol.ends_with("::go")
                && edge.target_symbol.ends_with("::test_commands")
        })
        .expect("go must call test_commands");
    assert!(
        matches!(
            edge.resolution.as_deref(),
            Some(Resolution::SameFile { .. })
        ),
        "the initializer names the function: {:?}",
        edge.resolution
    );

    let handler = res
        .unresolved
        .iter()
        .find(|u| u.callee_name == "handler" && u.source_symbol.ends_with("::takes"))
        .expect("takes(handler) still shadows");
    assert_eq!(
        handler.class,
        UnresolvedClass::LocalBinding,
        "a parameter is in scope in a same-named let's initializer"
    );
}

/// A closure parameter stays local inside the closure and does not hide a
/// same-named function called from the enclosing function.
#[test]
fn a_rust_closure_parameter_does_not_hide_an_outer_function() {
    let extraction = extract_file(
        "hyg.rs",
        concat!(
            "fn marker(_path: &str, _prefix: &str) -> bool { false }\n",
            "fn local_provider() {\n",
            "    let has = |marker: &str| marker.starts_with(\"x\");\n",
            "    let _ = has(\"n\") && marker(\"Cargo.toml\", \".\");\n",
            "}\n",
        ),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&extraction));
    let res = resolver.resolve_all(std::slice::from_ref(&extraction));

    let edge = res
        .edges
        .iter()
        .find(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_symbol.ends_with("::local_provider")
                && edge.target_symbol.ends_with("::marker")
        })
        .expect("local_provider must call marker");
    assert!(
        matches!(
            edge.resolution.as_deref(),
            Some(Resolution::SameFile { .. })
        ),
        "the outer marker(...) names the function: {:?}",
        edge.resolution
    );
}
