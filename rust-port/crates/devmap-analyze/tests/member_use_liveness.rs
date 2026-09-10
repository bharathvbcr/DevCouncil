//! Uses that are not calls, and calls whose receiver decides the target.
//!
//! Liveness reads the edge set. Four gaps meant a symbol could be plainly used
//! and still have no edge naming it, and each one surfaced as a *confident*
//! dead-code finding — the tier that reads as "delete this".
//!
//! Measured on this repository before these fixes: 75 confident findings, of
//! which the property reads, the attribute-passed CLI commands, and the
//! fabricated self-calls below were the three largest groups.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    analyze_liveness(&extractions, &resolution)
}

fn is_reported(reports: &[DeadSymbolReport], symbol: &str) -> bool {
    reports
        .iter()
        .any(|report| report.symbol_name == symbol && !report.is_exempt)
}

/// File-qualified, for fixtures where two files declare the same name — which
/// is the whole point of the receiver tests below.
fn is_reported_in(reports: &[DeadSymbolReport], file: &str, symbol: &str) -> bool {
    reports
        .iter()
        .any(|report| report.file_path == file && report.symbol_name == symbol && !report.is_exempt)
}

/// Reading a property is a use, even though it is never called.
///
/// `cfg.enabled` on an `@property` produces no call, and the extractor
/// flattens the member half to a bare `enabled`. The resolver refuses to
/// resolve a bare name globally — deliberately, so `except Exception as e`
/// cannot bind to an unrelated `def e` — and with nothing distinguishing a
/// member from a local it dropped both. Every property in the corpus was
/// therefore callerless.
#[test]
fn reading_a_property_through_a_typed_receiver_is_a_use() {
    let reports = reports(&[
        (
            "pkg/config.py",
            "class GatesConfig:\n    @property\n    def enabled(self):\n        return True\n\n    def genuinely_dead(self):\n        return 0\n",
        ),
        (
            "pkg/main.py",
            "from pkg.config import GatesConfig\n\n\ndef go():\n    cfg = GatesConfig()\n    return cfg.enabled\n",
        ),
    ]);
    assert!(
        !is_reported(&reports, "GatesConfig.enabled"),
        "the property is read on the line above: {reports:?}"
    );
    assert!(
        is_reported(&reports, "GatesConfig.genuinely_dead"),
        "a method nothing touches must still be reported, or the rule above is \
         a blanket amnesty for methods: {reports:?}"
    );
}

/// Passing a function by attribute is a use.
///
/// `app.command(name="baseline")(cmd.baseline)` never calls `baseline`; it
/// hands it to a decorator. Two things had to hold for this to resolve: the
/// member reference needed its receiver, and `from pkg import cmd` needed to
/// bind the *submodule* rather than the package `__init__`.
#[test]
fn passing_a_function_by_attribute_through_an_imported_module_is_a_use() {
    let reports = reports(&[
        ("pkg/__init__.py", "\n"),
        ("pkg/cmd.py", "def baseline(name):\n    return name\n"),
        (
            "pkg/main.py",
            "from pkg import cmd\n\n\nclass App:\n    def command(self, name):\n        def deco(fn):\n            return fn\n        return deco\n\n\napp = App()\napp.command(name='baseline')(cmd.baseline)\n",
        ),
    ]);
    assert!(
        !is_reported(&reports, "baseline"),
        "the decorator registration on the last line is its call site: {reports:?}"
    );
}

/// A package that really does re-export keeps binding to its `__init__`.
///
/// The submodule fallback must not outrank a name the package itself declares,
/// which is the order Python uses and the meaning of a re-exporting package.
#[test]
fn a_reexporting_package_still_binds_to_its_init() {
    let result = {
        let sources: Vec<(&str, &str)> = vec![
            ("pkg/__init__.py", "def shared():\n    return 1\n"),
            ("pkg/shared.py", "def other():\n    return 2\n"),
            (
                "app.py",
                "from pkg import shared\n\n\ndef use():\n    return shared()\n",
            ),
        ];
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(path, source)| extract_file(path, source))
            .collect();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        resolver.resolve_all(&extractions)
    };
    let target = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("shared"))
        .map(|edge| edge.target_file.clone());
    assert_eq!(
        target.as_deref(),
        Some("pkg/__init__.py"),
        "`__init__` declares `shared`, so that is what the import binds: {:?}",
        result.edges
    );
}

/// A call with a module receiver must not resolve to a same-named local.
///
/// `ast_lsp_handlers.reset_caches()` inside a file that itself declares
/// `reset_caches` resolved to *itself*: a fabricated self-call edge, and the
/// real target left with no caller and a confident dead-code finding. The same
/// shape put `a.cfg.capabilityFor(model)` on `Adapter.capabilityFor` and
/// reported `Config.capabilityFor` dead at 0.9.
#[test]
fn a_module_qualified_call_does_not_bind_to_a_same_named_local() {
    let result = {
        let sources: Vec<(&str, &str)> = vec![
            ("pkg/__init__.py", "\n"),
            ("pkg/inner.py", "def reset_caches():\n    return 1\n"),
            (
                "pkg/outer.py",
                "from pkg import inner as inner_handlers\n\n\ndef reset_caches():\n    return inner_handlers.reset_caches()\n",
            ),
        ];
        let extractions: Vec<Extraction> = sources
            .iter()
            .map(|(path, source)| extract_file(path, source))
            .collect();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        resolver.resolve_all(&extractions)
    };
    let self_call = result.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Calls
            && edge.source_symbol == "pkg/outer.py::reset_caches"
            && edge.target_symbol == "pkg/outer.py::reset_caches"
    });
    assert!(
        !self_call,
        "the receiver names the imported module, not this file: {:?}",
        result.edges
    );
    let reports = reports(&[
        ("pkg/__init__.py", "\n"),
        ("pkg/inner.py", "def reset_caches():\n    return 1\n"),
        (
            "pkg/outer.py",
            "from pkg import inner as inner_handlers\n\n\ndef reset_caches():\n    return inner_handlers.reset_caches()\n",
        ),
    ]);
    assert!(
        !is_reported_in(&reports, "pkg/inner.py", "reset_caches"),
        "the imported module's `reset_caches` is called on the line above: {reports:?}"
    );
    // And the caller's own same-named function really is uncalled here, so the
    // assertion above cannot be passing merely because nothing is reported.
    assert!(
        is_reported_in(&reports, "pkg/outer.py", "reset_caches"),
        "nothing calls the outer function; the fix must not have made every \
         same-named symbol look live: {reports:?}"
    );
}

/// A self-reference still resolves in its own file.
///
/// The receiver gate above must not swallow `self.helper()`, where the
/// receiver *is* this scope and a sibling declaration is exactly the right
/// answer.
#[test]
fn a_self_qualified_call_still_resolves_in_its_own_file() {
    let reports = reports(&[(
        "pkg/one.py",
        "class Thing:\n    def helper(self):\n        return 1\n\n    def run(self):\n        return self.helper()\n",
    )]);
    assert!(
        !is_reported(&reports, "Thing.helper"),
        "`self.helper()` names a sibling of the caller: {reports:?}"
    );
}

/// A method call's *receiver* is a use of the receiver.
///
/// `console.print(…)` names two things: the method, which the call record
/// already carries, and the receiver, which is an ordinary use of a different
/// symbol. `is_call_callee` refused both, so every method receiver in every
/// language was invisible to the graph.
///
/// A module-level singleton used the way singletons are used — bound at the top
/// of a file, called on in every function below — therefore had no inbound edge
/// at all. Measured on this repository: eleven files reporting their `console`
/// dead at 0.9, in each of which the line beneath it calls `console.print`.
#[test]
fn a_method_call_receiver_is_a_use_of_the_receiver() {
    let reports = reports(&[
        (
            "pkg/common.py",
            "class Console:\n    def print(self, msg):\n        pass\n\n\nconsole = Console()\n",
        ),
        (
            "pkg/aider.py",
            "from pkg import common as _common\n\nconsole = _common.console\nunused_alias = _common.console\n\n\ndef show():\n    console.print('hi')\n",
        ),
    ]);
    assert!(
        !is_reported_in(&reports, "pkg/aider.py", "console"),
        "the line below binds it and calls a method on it: {reports:?}"
    );
    // The control: an identical binding that nothing ever uses stays reported,
    // so the rule above credits the *use* and not merely the assignment.
    assert!(
        is_reported_in(&reports, "pkg/aider.py", "unused_alias"),
        "an alias nothing reads is still dead: {reports:?}"
    );
}

/// The method half stays suppressed.
///
/// The call record already carries it. Emitting a `Name` reference for it too
/// would double-count the call and let a bare same-named symbol anywhere in the
/// corpus look reachable.
#[test]
fn the_method_half_of_a_call_emits_no_name_reference() {
    let extraction = devmap_extract::extract_file(
        "m.py",
        "def helper():\n    pass\n\n\ndef go(obj):\n    return obj.helper()\n",
    );
    let name_refs: Vec<&str> = extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Name)
        .map(|reference| reference.name.as_str())
        .collect();
    assert!(
        !name_refs.contains(&"helper"),
        "`obj.helper()` is a call, not a bare use of a same-named function: {name_refs:?}"
    );
    assert!(
        name_refs.contains(&"obj"),
        "the receiver is still a use: {name_refs:?}"
    );
}

/// Shapes that were extracted-tier dead on the rust-port itself: a use the
/// graph could not see, not an unused function.
#[test]
fn rust_uses_that_are_not_ordinary_calls_keep_the_symbol_live() {
    let reports = reports(&[(
        "lib.rs",
        concat!(
            "fn unknown_ctime() -> i128 { -1 }\n",
            "fn genuinely_dead() -> i128 { 0 }\n",
            "#[derive(serde::Deserialize)]\n",
            "struct Stamp {\n",
            "    #[serde(default = \"unknown_ctime\")]\n",
            "    ctime_ns: i128,\n",
            "}\n",
            "\n",
            "fn test_commands() -> u8 { 1 }\n",
            "fn go() {\n",
            "    let test_commands = test_commands();\n",
            "    let _ = test_commands;\n",
            "}\n",
            "\n",
            "fn marker(_path: &str, _prefix: &str) -> bool { false }\n",
            "fn local_provider() {\n",
            "    let has = |marker: &str| marker.starts_with(\"x\");\n",
            "    let _ = has(\"n\") && marker(\"Cargo.toml\", \".\");\n",
            "}\n",
            "\n",
            "unsafe extern \"C\" {\n",
            "    fn tree_sitter_liquid() -> *const ();\n",
            "}\n",
            "fn from_raw(_: unsafe extern \"C\" fn() -> *const ()) -> u8 { 0 }\n",
            "const LANG: u8 = unsafe { from_raw(tree_sitter_liquid) };\n",
            "\n",
            "struct Cache;\n",
            "impl Cache {\n",
            "    fn stamp_rule() {}\n",
            "    fn unused_method() {}\n",
            "    fn rule_stamps() {\n",
            "        let _ = [()].into_iter().map(Self::stamp_rule);\n",
            "    }\n",
            "}\n",
            "\n",
            "struct Daemon;\n",
            "impl Daemon {\n",
            "    fn vanished_reason(&self) -> Option<u8> { None }\n",
            "    fn run_loop(&self) {\n",
            "        tokio::select! {\n",
            "            _ = ticker.tick() => {\n",
            "                self.vanished_reason();\n",
            "            }\n",
            "        }\n",
            "    }\n",
            "}\n",
            "\n",
            "struct Store;\n",
            "impl Store {\n",
            "    fn checkpoint_wal(&self) {\n",
            "        fn run() {}\n",
            "        run();\n",
            "    }\n",
            "}\n",
            "\n",
            "struct Adjacency;\n",
            "impl Adjacency {\n",
            "    fn run(&self, _rank: u32) {}\n",
            "    fn unused_run(&self) {}\n",
            "}\n",
            "struct Edges {\n",
            "    by_source_symbol: Adjacency,\n",
            "    by_target_symbol: Adjacency,\n",
            "}\n",
            "impl Edges {\n",
            "    fn symbols(&self, reverse: bool) {\n",
            "        let adjacency = if reverse {\n",
            "            &self.by_target_symbol\n",
            "        } else {\n",
            "            &self.by_source_symbol\n",
            "        };\n",
            "        adjacency.run(0);\n",
            "    }\n",
            "}\n",
        ),
    )]);
    for live in [
        "unknown_ctime",
        "test_commands",
        "marker",
        "tree_sitter_liquid",
        "Cache.stamp_rule",
        "Daemon.vanished_reason",
        "Store.checkpoint_wal.run",
        "Adjacency.run",
    ] {
        assert!(
            !is_reported(&reports, live),
            "{live} is used and must not be dead: {reports:?}"
        );
    }
    assert!(
        is_reported(&reports, "genuinely_dead"),
        "a helper nothing touches must still be reported: {reports:?}"
    );
    assert!(
        is_reported(&reports, "Cache.unused_method"),
        "a method nothing touches must still be reported: {reports:?}"
    );
    assert!(
        is_reported(&reports, "Adjacency.unused_run"),
        "typing adjacency must not amnesty every method of Adjacency: {reports:?}"
    );
}
