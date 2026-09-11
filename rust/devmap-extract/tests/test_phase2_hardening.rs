use devmap_extract::languages::LANGUAGE_SPECS;
use devmap_extract::*;
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Unique per call, not merely per instant: `SystemTime` ticks every 1 us here,
/// so same-microsecond callers would otherwise share one fixture directory.
fn temp_root(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ))
}

#[derive(Deserialize)]
struct FrozenLanguageSpec {
    name: String,
    grammar: String,
    extensions: Vec<String>,
    embedded: Vec<String>,
    fixture: String,
}

/// Deliberate divergences from the frozen registry's `embedded` lists (R3).
///
/// Recorded here rather than by regenerating the snapshot. The snapshot is
/// what *Python* declared; rewriting it would erase the evidence that the two
/// ever differed, and the next divergence would then land silently. Every row
/// must also appear in `docs/devmap/DIVERGENCES.md`.
const EMBEDDED_DIVERGENCES: &[(&str, &[&str], &str)] = &[(
    "Vue",
    &["typescript", "tsx", "javascript", "css", "html"],
    "X38: Vue single-file components are routinely written with JSX render \
     functions and Vue's own compiler accepts `<script lang=\"tsx\">`. Python \
     never read `embedded` at all, so its list records no decision about TSX. \
     Svelte and Astro deliberately do NOT get it: Svelte's template is not JSX \
     and an Astro `<script>` is plain JS/TS.",
)];

#[test]
fn test_x8_language_authority_matches_frozen_python_registry() {
    let frozen: Vec<FrozenLanguageSpec> =
        serde_json::from_str(include_str!("../../testdata/golden/language_specs.json"))
            .expect("frozen language manifest must be valid JSON");
    assert_eq!(frozen.len(), LANGUAGE_SPECS.len());

    for (rust, python) in LANGUAGE_SPECS.iter().zip(frozen.iter()) {
        assert_eq!(rust.name, python.name);
        assert_eq!(rust.grammar, python.grammar);
        assert_eq!(rust.extensions, python.extensions);
        // Strict everywhere except where a divergence is declared by name, so
        // an *undeclared* change to any `embedded` list still fails loudly.
        match EMBEDDED_DIVERGENCES
            .iter()
            .find(|(name, _, _)| *name == python.name)
        {
            Some((_, expected, reason)) => assert_eq!(rust.embedded, *expected, "{reason}"),
            None => assert_eq!(rust.embedded, python.embedded, "{}", python.name),
        }
        assert!(!python.fixture.is_empty());
    }
}

/// Symbol identity must stay byte-identical to the frozen Python baseline.
///
/// `codeintel` recovers a symbol's owning file by splitting its id on the FIRST
/// `::` (`graph/intel.py:47`), so the whole 103-node golden corpus carries at
/// most one `::` per id and spells a method `<file>::<Type>.<name>`. A second
/// `::` forks symbol identity from that baseline without any parity check
/// noticing, which is exactly how it regressed before this test existed.
#[test]
fn python_symbol_identity_matches_the_frozen_python_baseline() {
    #[derive(Deserialize)]
    struct GoldenNode {
        id: String,
        kind: String,
    }

    let golden: Vec<GoldenNode> =
        serde_json::from_str(include_str!("../../testdata/golden/python_app/nodes.json"))
            .expect("frozen python_app nodes must be valid JSON");
    let extraction = extract_file(
        "app.py",
        include_str!("../../testdata/fixtures/tier_a/python_app/app.py"),
    );

    for node in &golden {
        assert!(
            node.id.matches("::").count() <= 1,
            "frozen baseline id {} already carries a nested `::`",
            node.id
        );
        if node.kind == "file" {
            continue;
        }
        let matched = extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == node.id)
            .unwrap_or_else(|| {
                panic!(
                    "no extracted symbol carries the frozen id {}; extracted ids were {:?}",
                    node.id,
                    extraction
                        .symbols
                        .iter()
                        .map(|symbol| &symbol.qualified_name)
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            format!("{:?}", matched.kind).to_lowercase(),
            node.kind,
            "kind diverged for {}",
            node.id
        );
    }

    for symbol in &extraction.symbols {
        assert!(
            symbol.qualified_name.matches("::").count() <= 1,
            "extracted id {} nests a second `::` and forks file-path recovery",
            symbol.qualified_name
        );
    }
}

#[test]
fn test_d1_x2_x3_extraction_contract_is_complete_and_byte_based() {
    // closes D1, X2, X3. This is the load-bearing Phase 2 contract: callers
    // must be able to distinguish the parser/grammar used and consume explicit
    // export/reference records without reconstructing them from raw source.
    let source = "// é\nexport * from './api';\nconst client = new ApiClient();\n";
    let ext = extract_file("src/index.ts", source);

    assert!(matches!(
        ext.engine,
        ExtractionEngine::TreeSitter { ref grammar, grammar_version }
            if grammar == "typescript" && grammar_version > 0
    ));
    assert!(ext.exports.iter().any(|export| {
        export.exported_name == "*" && export.module_specifier.as_deref() == Some("./api")
    }));
    assert!(ext.references.iter().any(|reference| {
        reference.name == "ApiClient" && reference.kind == ReferenceKind::Constructor
    }));

    let client = ext
        .references
        .iter()
        .find(|reference| reference.name == "ApiClient")
        .expect("constructor reference must exist");
    assert_eq!(
        &source[client.span.start_byte..client.span.end_byte],
        "ApiClient"
    );
}

#[test]
fn test_x5_error_tree_is_partial_not_clean() {
    // closes X5
    let bad_code = "function broken( { let x = ; ";
    let ext = extract_file("src/broken.ts", bad_code);
    match ext.parse_outcome {
        ParseOutcome::Partial { ref error_ranges } => {
            assert!(
                !error_ranges.is_empty(),
                "ERROR tree must yield non-empty error_ranges"
            );
        }
        _ => panic!(
            "Expected ParseOutcome::Partial for malformed tree, got {:?}",
            ext.parse_outcome
        ),
    }
}

#[test]
fn test_x7_failed_extraction_not_cached() {
    // closes X7 — admission gate rejects Failed before any persistence layer sees it
    use devmap_extract::cache::cache_admits;

    let outcome = ParseOutcome::Failed {
        reason: "OOM or parse fatal".to_string(),
    };
    assert!(!cache_admits(&outcome));
    assert!(cache_admits(&ParseOutcome::Clean));
}

#[test]
fn test_x1_ts_extended_syntax() {
    // closes X1
    let code = r#"
export enum Status { Active, Inactive }
export namespace Internal {
    export function helper() {}
}
export declare function globalFunc(): void;
export abstract class AbstractWorker {
    abstract run(): void;
}
"#;
    let ext = extract_file("src/types.ts", code);
    assert!(ext
        .symbols
        .iter()
        .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
    assert!(ext
        .symbols
        .iter()
        .any(|s| s.name == "Internal" && s.kind == SymbolKind::Module));
    assert!(ext
        .symbols
        .iter()
        .any(|s| s.name == "AbstractWorker" && s.kind == SymbolKind::Class));
}

#[test]
fn test_x2_new_expression_and_composite_literal() {
    // closes X2
    let ts_code = "const client = new ApiClient();";
    let ext_ts = extract_file("src/app.ts", ts_code);
    assert!(ext_ts.calls.iter().any(|c| c.callee_name == "ApiClient"));

    let go_code = "cfg := Config{Name: \"test\"}";
    let ext_go = extract_file("src/app.go", go_code);
    assert!(ext_go.calls.iter().any(|c| c.callee_name == "Config"));
}

#[test]
fn test_x3_export_star_sentinel() {
    // closes X3
    let code = "export * from './submodule';";
    let ext = extract_file("src/index.ts", code);
    assert!(ext
        .imports
        .iter()
        .any(|i| i.module_specifier == "./submodule"));
}

#[test]
fn test_x20_g14_decorators() {
    // closes X20, G14
    let code = r#"
@Controller("/api")
export class ApiController {
    @Get("/users")
    getUsers() {}
}
"#;
    let ext = extract_file("src/api.ts", code);
    assert!(ext.symbols.iter().any(|s| s.name == "ApiController"));
}

#[test]
fn test_g16_jsx_member_tags() {
    // closes G16
    let code = "function render() { return <UI.Button color=\"red\" />; }";
    let ext = extract_file("src/component.tsx", code);
    assert!(ext
        .calls
        .iter()
        .any(|c| c.callee_name == "UI.Button" || c.callee_name.contains("Button")));
}

#[test]
fn test_g18_namespace_imports() {
    // closes G18
    let code = "import * as utils from './utils';";
    let ext = extract_file("src/main.ts", code);
    let imp = ext
        .imports
        .iter()
        .find(|i| i.module_specifier == "./utils")
        .unwrap();
    assert_eq!(imp.alias, Some("utils".to_string()));
}

#[test]
fn test_g19_x10_rust_generic_impls() {
    // closes G19, X10
    let code = r#"
pub struct Service<T> {
    pub inner: T,
}
impl<T: std::fmt::Display> Service<T> {
    pub fn process(&self) {}
}
"#;
    let ext = extract_file("src/service.rs", code);
    let sym = ext.symbols.iter().find(|s| s.name == "process").unwrap();
    // Full identity, not `contains`: the frozen baseline spells an impl method
    // `<file>::<Type>.<name>` (testdata/golden/rust_app -> `main.rs::Processor.process`),
    // and a substring check would also accept a nested `::` that forks identity.
    assert_eq!(sym.qualified_name, "src/service.rs::Service.process");
    assert_eq!(
        sym.parent_symbol.as_deref(),
        Some("src/service.rs::Service")
    );
}

#[test]
fn test_x9_rust_grouped_use() {
    // closes X9
    let code = "use std::{fmt::Display, collections::HashMap};";
    let ext = extract_file("src/lib.rs", code);
    assert!(!ext.imports.is_empty());
}

#[test]
fn test_v5_v6_discovery_report() {
    // closes V5, V6
    let code = r#"
[project.scripts]
my-cli = "pkg.cli:main"

[tool.poetry.scripts]
another-cli = "pkg.cli:other"
"#;
    let ext = extract_file("pyproject.toml", code);
    assert!(ext.wiring.iter().any(|w| w.kind == WiringKind::ScriptEntry));

    let root = temp_root("discovery");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("kept.py"), "def kept(): pass\n").unwrap();
    std::fs::write(
        root.join("oversized.py"),
        vec![b'x'; (MAX_SOURCE_BYTES + 1) as usize],
    )
    .unwrap();
    let (sources, report) = collect_sources_with_report(&root).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(report.yielded_paths, ["kept.py"]);
    assert!(report.skipped_paths.iter().any(|(path, reason)| {
        path == "oversized.py" && matches!(reason, DiscoverySkipReason::Oversized { .. })
    }));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn test_r4_determinism_byte_identical() {
    // closes R4, G4, G26
    let code = "def fn_a(): pass\ndef fn_b(): pass\ndef fn_c(): pass\n";
    let ext1 = extract_file("src/a.py", code);
    let ext2 = extract_file("src/a.py", code);
    assert_eq!(ext1.content_hash, ext2.content_hash);
    let json1 = serde_json::to_string(&ext1).unwrap();
    let json2 = serde_json::to_string(&ext2).unwrap();
    assert_eq!(json1, json2);
}

#[test]
fn test_source_collection_order_is_stable() {
    let root = temp_root("source-order");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("zeta.py"), "def zeta(): pass\n").unwrap();
    std::fs::write(root.join("alpha.py"), "def alpha(): pass\n").unwrap();

    let sources = collect_sources(&root).unwrap();
    let paths: Vec<_> = sources.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, vec!["alpha.py", "zeta.py"]);

    let _ = std::fs::remove_dir_all(root);
}

/// `__all__` is a Python module's explicit public API declaration, so a symbol
/// named in it is exported by definition. Ignoring it made devmap report a
/// module's own published surface as dead code.
#[test]
fn python_dunder_all_marks_symbols_exported() {
    let ext = extract_file(
        "pkg/api.py",
        "__all__ = [\"public_one\"]\n__all__ += [\n    \"PublicTwo\",\n]\n\
         \ndef public_one():\n    return 1\n\nclass PublicTwo:\n    pass\n\ndef private_helper():\n    return 2\n",
    );
    let exported = |name: &str| {
        ext.symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("missing symbol {name}"))
            .is_exported
    };
    assert!(exported("public_one"), "`__all__ = [...]` must export");
    assert!(exported("PublicTwo"), "`__all__ += [...]` must also export");
    assert!(
        !exported("private_helper"),
        "a symbol absent from __all__ stays private"
    );
}

/// A bare mention of `__all__` is not a declaration; treating one as an export
/// list would silently exempt arbitrary names from dead-code reporting.
#[test]
fn python_dunder_all_requires_a_real_assignment() {
    let ext = extract_file(
        "pkg/b.py",
        "# __all__ would list [\"ghost\"] here\ndef ghost():\n    return 1\n",
    );
    assert!(!ext
        .symbols
        .iter()
        .any(|symbol| symbol.name == "ghost" && symbol.is_exported));
}

/// SC6a: the two halves of the Go interface-satisfaction join must both travel
/// on the `Extraction`, because the interface and its implementation routinely
/// live in different files of the same package and extraction is per file.
#[test]
fn go_interface_specs_and_method_arities_travel_on_the_extraction() {
    let ext = extract_file(
        "svc/iface.go",
        "package svc\n\
         type Visitor interface {\n\
         \tvisit()\n\
         \tvisitPair(int, string) error\n\
         \tvisitGroup(a, b int) error\n\
         \tvisitAll(xs ...int)\n\
         \tio.Reader\n\
         }\n",
    );
    let specs: Vec<(&str, &str, usize)> = ext
        .go_interface_methods
        .iter()
        .map(|spec| {
            (
                spec.interface_name.as_str(),
                spec.method.as_str(),
                spec.param_count,
            )
        })
        .collect();
    assert_eq!(
        specs,
        vec![
            ("Visitor", "visit", 0),
            ("Visitor", "visitAll", 1),
            ("Visitor", "visitGroup", 2),
            ("Visitor", "visitPair", 2),
        ],
        "interface method specs must carry the declaring interface and an arity \
         that counts each grouped name and treats a variadic as one: {:?}",
        ext.go_interface_methods
    );

    let impls = extract_file(
        "svc/impl.go",
        "package svc\n\
         type walker struct{}\n\
         func (w *walker) visit() {}\n\
         func (w *walker) visitPair(n int, s string) error { return nil }\n\
         func (w *walker) visitGroup(a, b int) error { return nil }\n\
         func (w walker) visitAll(xs ...int) {}\n",
    );
    let arities: Vec<(&str, usize)> = impls
        .go_method_params
        .iter()
        .map(|entry| (entry.qualified_name.as_str(), entry.param_count))
        .collect();
    assert_eq!(
        arities,
        vec![
            ("svc/impl.go::walker.visit", 0),
            ("svc/impl.go::walker.visitAll", 1),
            ("svc/impl.go::walker.visitGroup", 2),
            ("svc/impl.go::walker.visitPair", 2),
        ],
        "method declarations must report the same arity the interface spec does, \
         keyed by the symbol's qualified name: {:?}",
        impls.go_method_params
    );
}

/// Symbol identities within a file must be unique.
///
/// A qualified name is the graph's join key: edges name their endpoints by it,
/// and two nodes sharing one are two things the graph cannot tell apart. This
/// pins the invariant across the shapes where it currently holds — inherent
/// impls, free functions, structs, nested types, and same-named methods on
/// different types.
///
/// Trait impls are included (SC11/SC6b). They used to be the exception: every
/// implementor of a trait shared one qualified name, and a single `impl`
/// overriding a defaulted method already produced the same name twice. Impl
/// methods are now qualified by their type and the trait's own declarations by
/// the trait, so the declaration and each implementation are distinct nodes.
#[test]
fn symbol_identities_are_unique_within_a_file() {
    let rust = extract_file(
        "lib.rs",
        r#"
pub struct Alpha;
pub struct Beta;

impl Alpha {
    fn shared(&self) -> u32 { 1 }
    fn only_alpha(&self) -> u32 { 2 }
}

impl Beta {
    fn shared(&self) -> u32 { 3 }
}

pub fn free_function() -> u32 { 4 }

mod inner {
    pub fn nested() -> u32 { 5 }
}
"#,
    );

    let traits = extract_file(
        "traits.rs",
        r#"
pub trait Greeter {
    fn greet(&self) -> String;
    fn farewell(&self) -> String { "bye".to_string() }
}

pub struct English;
pub struct French;

impl Greeter for English {
    fn greet(&self) -> String { "hello".to_string() }
    fn farewell(&self) -> String { "cheerio".to_string() }
}

impl Greeter for French {
    fn greet(&self) -> String { "bonjour".to_string() }
}
"#,
    );

    let go = extract_file(
        "svc.go",
        r#"
package svc

type Redis struct{}
type Mem struct{}

func (r *Redis) read() string { return "r" }
func (m *Mem) read() string { return "m" }
func plain() string { return "p" }
"#,
    );

    for ext in [rust, traits, go] {
        let mut seen = std::collections::BTreeMap::new();
        for symbol in &ext.symbols {
            *seen.entry(symbol.qualified_name.clone()).or_insert(0usize) += 1;
        }
        let duplicates: Vec<_> = seen
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(name, count)| format!("{name} x{count}"))
            .collect();
        assert!(
            duplicates.is_empty(),
            "{}: duplicate symbol identities make two nodes indistinguishable \
             to every edge that names them: {duplicates:?}",
            ext.file_path
        );

        // And the same-named methods on different types must both be present,
        // or "unique" would be satisfied by silently dropping one.
        let shared: Vec<_> = ext
            .symbols
            .iter()
            .filter(|s| s.name == "shared" || s.name == "read" || s.name == "greet")
            .map(|s| s.qualified_name.as_str())
            .collect();
        assert!(
            shared.len() >= 2,
            "{}: same-named methods on different owners must survive as distinct \
             symbols, found {shared:?}",
            ext.file_path
        );
    }
}

/// A call target that is not a name records no callee at all.
///
/// SC26 found five call shapes that recorded a whole *expression* where a callee
/// name belongs, and fixed those five. The defect is a class, not a list: the
/// text fallback that produced them survived in every arm the fix did not visit,
/// and the DevCouncil repository still carried three of its shapes.
///
/// Each case below was read out of `generation_unresolved` on the live store
/// before it was written here. All of them are structurally the same mistake —
/// the callee slot holds something that has no name — and each is now refused at
/// the one place a callee name is built.
#[test]
fn a_call_target_that_is_not_a_name_records_no_callee() {
    let cases: [(&str, &str, &str); 5] = [
        // `src/devcouncil/cli/main.py:161`. The decorator is applied by hand, so
        // the outer call's `function` field is the inner *call*; the whole
        // `app.command(name="apply-patch")` became a callee name, one row per
        // registration.
        (
            "cli.py",
            "def register():\n    app.command(name=\"apply-patch\")(apply_patch)\n",
            "command",
        ),
        // `tests/unit/test_companion_sca.py:343`, a curried factory.
        (
            "curry.py",
            "def probe():\n    _default_runner(5)([\"tool\"], tmp)\n",
            "_default_runner",
        ),
        // Bundled JavaScript. SC26 fixed the Go `defer func(){…}()` form in the
        // Go arm; the JS arm still recorded the whole minified body.
        (
            "bundle.js",
            "function boot(){ (function(t){ if(Array.isArray(t)) return t })(x); }\n",
            "isArray",
        ),
        // The same shape in Rust, which SC26 also did not reach.
        (
            "iife.rs",
            "fn run() {\n    let v = (|| { helper() })();\n}\n",
            "helper",
        ),
        // A `new` whose constructor is an expression rather than a name. This
        // arm never went through the splitter at all.
        (
            "ctor.ts",
            "function make(){ const v = new (flag ? Alpha : Beta)(); return v; }\n",
            "make",
        ),
    ];

    for (path, source, still_present) in cases {
        let ext = extract_file(path, source);
        let bogus: Vec<&str> = ext
            .calls
            .iter()
            .map(|call| call.callee_name.as_str())
            .filter(|name| {
                name.chars()
                    .any(|ch| !(ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#')))
            })
            .collect();
        assert!(
            bogus.is_empty(),
            "{path}: a callee name must be a name, not an expression: {bogus:?}"
        );

        // And the fix must not be "record nothing": the real callee in each
        // fixture still has to be there, or this would pass by deleting the
        // graph.
        let kept: Vec<&str> = ext
            .calls
            .iter()
            .map(|call| call.callee_name.as_str())
            .collect();
        assert!(
            kept.contains(&still_present) || {
                let symbols: Vec<&str> = ext.symbols.iter().map(|s| s.name.as_str()).collect();
                symbols.contains(&still_present)
            },
            "{path}: refusing the expression must not remove {still_present:?}: \
             calls {kept:?}"
        );
    }

    // The receiver survives the split rather than being thrown away with the
    // rejected text: `new pkg.Widget()` constructs `Widget` through `pkg`, which
    // is what lets import evidence classify it.
    let member_new = extract_file("m.ts", "function f(){ return new pkg.Widget(); }\n");
    let widget = member_new
        .calls
        .iter()
        .find(|call| call.callee_name == "Widget")
        .unwrap_or_else(|| {
            panic!(
                "`new pkg.Widget()` must construct `Widget`: {:?}",
                member_new
                    .calls
                    .iter()
                    .map(|c| (&c.callee_name, &c.receiver_expr))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(widget.receiver_expr.as_deref(), Some("pkg"));
}

/// A Python re-export alias is a symbol.
///
/// `src/devcouncil/domain/evidence.py:55` is `TestEvidence = VerificationEvidence`
/// and the module declares no `__all__`, so the alias was dropped with the
/// private constants and existed nowhere in the graph — while twenty import
/// bindings across the repository named it, resolved to a file that (as far as
/// the resolver could tell) does not declare it, and fell off the ladder.
///
/// The scope is the point of the test: an alias is kept, and the constants that
/// motivated the `__all__` filter are still dropped. Widening this to every
/// module-level assignment would add a node per private constant and turn each
/// one into a dead-code candidate.
#[test]
fn a_python_reexport_alias_is_a_symbol_and_a_constant_is_not() {
    let ext = extract_file(
        "domain/evidence.py",
        concat!(
            "class VerificationEvidence:\n",
            "    pass\n",
            "\n",
            "TestEvidence = VerificationEvidence\n",
            "QualifiedAlias = other.Thing\n",
            "DEFAULT_TIMEOUT = 30\n",
            "_private = 1\n",
            "logger = logging.getLogger(__name__)\n",
            "computed = [1, 2, 3]\n",
        ),
    );
    let names: Vec<&str> = ext.symbols.iter().map(|s| s.name.as_str()).collect();

    for alias in ["TestEvidence", "QualifiedAlias"] {
        assert!(
            names.contains(&alias),
            "a re-export alias is a name other modules import: {names:?}"
        );
    }
    for dropped in ["DEFAULT_TIMEOUT", "_private", "logger", "computed"] {
        assert!(
            !names.contains(&dropped),
            "{dropped:?} is a value, not an alias, and must stay out of the \
             graph as it always has: {names:?}"
        );
    }
}

/// Every callable exports the values it binds, under the identity its own calls
/// report as their caller.
///
/// The resolver has a `LocalBinding` tier that answers "this bare call went to a
/// value this scope declared", but it could only see bindings that carry a
/// written type, so it fired on 4 calls out of roughly 50. The extractor already
/// computed the full per-scope set for reference shadowing; it simply never left
/// the crate.
///
/// The join is what this pins. A `scope_locals` key that does not equal the
/// `caller_symbol` of a call in the same scope is unusable — the same
/// unjoinable-identity failure as SC9/SC10 — so the assertion is made against
/// the calls themselves rather than against a string that merely looks right.
#[test]
fn scope_locals_are_keyed_by_the_identity_calls_report_as_their_caller() {
    let rust = extract_file(
        "lib/scope.rs",
        concat!(
            "fn run() {\n",
            "    let handler = |name: &str| name.len();\n",
            "    let bare = |v| v;\n",
            "    handler(\"x\");\n",
            "    bare(1);\n",
            "}\n",
            "fn other() {\n",
            "    handler(\"y\");\n",
            "}\n",
            "fn own(typed: &str, plain: i32) {}\n",
        ),
    );
    let locals_of = |ext: &devmap_extract::model::Extraction, scope: &str| -> Vec<String> {
        ext.scope_locals
            .iter()
            .filter(|(owner, _)| owner == scope)
            .map(|(_, local)| local.clone())
            .collect()
    };

    let run_locals = locals_of(&rust, "lib/scope.rs::run");
    assert!(
        run_locals.contains(&"handler".to_string()),
        "a `let`-bound closure is a local of the scope that binds it: {run_locals:?}"
    );
    assert!(
        !run_locals.contains(&"name".to_string()) && !run_locals.contains(&"v".to_string()),
        "closure parameters must not leak into the enclosing function: {run_locals:?}"
    );
    // The former expectation merged Rust closure parameters into `run` and
    // contradicted a_rust_closure_parameter_does_not_hide_an_outer_function:
    // `|marker| ...; marker()` outside the closure must still call the module
    // function. Keep the binding oracle on the scope that declares it.
    let own_locals = locals_of(&rust, "lib/scope.rs::own");
    assert!(
        own_locals.contains(&"typed".to_string()) && own_locals.contains(&"plain".to_string()),
        "the callable's own parameters must remain bound: {own_locals:?}"
    );
    assert!(
        locals_of(&rust, "lib/scope.rs::other").is_empty(),
        "`other` binds nothing; claiming otherwise would relabel a real \
         resolution failure as expected"
    );

    // The key must be the string a call in that scope actually carries.
    let caller = rust
        .calls
        .iter()
        .find(|call| call.callee_name == "handler")
        .and_then(|call| call.caller_symbol.clone())
        .expect("the `handler(\"x\")` call must be recorded");
    assert!(
        rust.scope_locals
            .iter()
            .any(|(scope, local)| *scope == caller && local == "handler"),
        "the scope key must equal `caller_symbol` or the resolver's lookup can \
         never hit: caller {caller:?}, pairs {:?}",
        rust.scope_locals
    );

    // An unnamed callback is not a scope the graph names, so its bindings belong
    // to the enclosing named function — exactly where its calls are attributed.
    let js = extract_file(
        "app.js",
        "function mount() {\n  useEffect(() => { const timer = 1; tick(timer); });\n}\n",
    );
    assert!(
        locals_of(&js, "app.js::mount").contains(&"timer".to_string()),
        "an unnamed arrow's locals belong to the scope its calls name: {:?}",
        js.scope_locals
    );

    // Serialized output is digested by the determinism gate, so the derived set
    // must be ordered, not merely stable within one process.
    let mut sorted = rust.scope_locals.clone();
    sorted.sort();
    assert_eq!(
        rust.scope_locals, sorted,
        "scope_locals is derived from a HashSet and must be sorted before it is \
         serialized"
    );
}

/// A parameter's *name* is a binding; its *type* is a use.
///
/// The parameter-list rule reaches an identifier the grammar gives no field —
/// `def f(a)`, `def f(a: T)`, `function f(a)`, `|handler|`. Both halves of
/// `typed_parameter` sit under that one node, so a rule written as plain
/// containment would take the type with the name and erase the reference to it.
/// That is the mistake `c_declarator_suppression_spares_real_references` pins in
/// the C family, and this is the same mistake's shape in Python.
#[test]
fn a_parameter_type_is_still_a_reference_when_its_name_is_a_binding() {
    let ext = extract_file(
        "svc/handler.py",
        concat!(
            "class Widget:\n",
            "    pass\n",
            "\n",
            "def handle(item: Widget, plain, defaulted=1):\n",
            "    return item\n",
        ),
    );
    let referenced: Vec<&str> = ext
        .references
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    assert!(
        referenced.contains(&"Widget"),
        "an annotation names a type that is used here: {referenced:?}"
    );

    let locals: Vec<&str> = ext
        .scope_locals
        .iter()
        .filter(|(scope, _)| scope == "svc/handler.py::handle")
        .map(|(_, local)| local.as_str())
        .collect();
    for bound in ["item", "plain", "defaulted"] {
        assert!(
            locals.contains(&bound),
            "{bound:?} is declared by this signature: {locals:?}"
        );
    }
    assert!(
        !locals.contains(&"Widget"),
        "a type annotation declares nothing: {locals:?}"
    );
}
