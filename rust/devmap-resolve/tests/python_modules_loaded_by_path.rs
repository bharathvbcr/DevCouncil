//! A Python module loaded **by file path** is a dependency like any import.
//!
//! `importlib.util.spec_from_file_location(name, ROOT / "scripts/x.py")`
//! followed by `module_from_spec(spec)` is how scripts and tests reach a file
//! that is not on `sys.path`. Nothing about it is an `import` statement, so
//! before this rung the loaded file had no `Imports` edge from its loader and
//! every `module.fn()` through the handle landed in the unresolved ledger as
//! `uninferred_receiver` — `impact fn` answered "no callers" for functions
//! whose only callers were exactly these scripts.
//!
//! Every positive case below is paired with the abstention that bounds it:
//! a path the extractor cannot read literally, a suffix two indexed files
//! share, a handle rebound between load and use, and a same-named local in a
//! different function. A wrong edge here would be worse than a missing one —
//! it would hand a function callers it does not have.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn has_import(resolution: &ResolutionResult, from: &str, to: &str) -> bool {
    resolution.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Imports && edge.source_file == from && edge.target_file == to
    })
}

fn has_call(resolution: &ResolutionResult, caller: &str, callee: &str) -> bool {
    resolution.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Calls
            && edge.source_symbol == caller
            && edge.target_symbol == callee
    })
}

/// Every Calls edge into `callee`, for a failure message that says what did
/// happen rather than only what did not.
fn callers_of(resolution: &ResolutionResult, callee: &str) -> Vec<String> {
    resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol == callee)
        .map(|edge| edge.source_symbol.clone())
        .collect()
}

const LOADED: &str = "scripts/build_supplement.py";
const LOADED_SOURCE: &str = "\
IDENTIFYING = None

def rename_project(text):
    return text, 0

def build_supplement_pdf():
    return 1
";

/// The shape in `BINN/scripts/build_paper.py`, verbatim in everything that
/// matters: `ROOT` anchored on `__file__`, the load inside a function, the
/// module returned, and the caller reaching through `supplement().fn()`.
#[test]
fn the_binn_loader_function_shape_links_file_and_callers() {
    let loader = "\
from __future__ import annotations
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def supplement():
    import importlib.util
    spec = importlib.util.spec_from_file_location(
        \"build_supplement\", ROOT / \"scripts/build_supplement.py\")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def anonymise(text):
    text, _ = supplement().rename_project(text)
    return text


def check():
    handle = supplement()
    handle.build_supplement_pdf()
";
    let resolution = resolve(&[("scripts/build_paper.py", loader), (LOADED, LOADED_SOURCE)]);
    let import_edges = resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Imports
                && edge.source_file == "scripts/build_paper.py"
                && edge.target_file == LOADED
        })
        .count();
    // The handle and the loader function both name the file; it is one
    // dependency.
    assert_eq!(
        import_edges, 1,
        "expected exactly one Imports edge from the loader to the file it loads by path"
    );
    let rename = format!("{LOADED}::rename_project");
    assert!(
        has_call(&resolution, "scripts/build_paper.py::anonymise", &rename),
        "`supplement().rename_project()` did not reach its target; callers: {:?}",
        callers_of(&resolution, &rename)
    );
    let build = format!("{LOADED}::build_supplement_pdf");
    assert!(
        has_call(&resolution, "scripts/build_paper.py::check", &build),
        "`handle = supplement(); handle.build_supplement_pdf()` did not reach its \
         target; callers: {:?}",
        callers_of(&resolution, &build)
    );
}

/// `BINN/scripts/test_fit_retention.py`: the handle is a module global and the
/// calls are inside test functions.
#[test]
fn a_module_level_handle_is_visible_inside_functions() {
    let loader = "\
import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location(\"fit_retention\", ROOT / \"scripts/fit_retention.py\")
fit_retention = importlib.util.module_from_spec(spec)
sys.modules[\"fit_retention\"] = fit_retention
spec.loader.exec_module(fit_retention)


def test_fit():
    assert fit_retention.fit(3) == 3
";
    let loaded = "def fit(x):\n    return x\n";
    let resolution = resolve(&[
        ("scripts/test_fit_retention.py", loader),
        ("scripts/fit_retention.py", loaded),
    ]);
    assert!(has_import(
        &resolution,
        "scripts/test_fit_retention.py",
        "scripts/fit_retention.py"
    ));
    assert!(
        has_call(
            &resolution,
            "scripts/test_fit_retention.py::test_fit",
            "scripts/fit_retention.py::fit"
        ),
        "callers: {:?}",
        callers_of(&resolution, "scripts/fit_retention.py::fit")
    );
}

/// The four loader APIs, each with the path spelled a different way.
#[test]
fn every_supported_loader_form_links_its_file() {
    let loader = "\
import os
import imp
import runpy
from importlib.machinery import SourceFileLoader
from importlib.util import spec_from_file_location, module_from_spec
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))


def by_from_import():
    spec = spec_from_file_location(\"a\", Path(__file__).parent / \"a.py\")
    mod = module_from_spec(spec)
    spec.loader.exec_module(mod)
    mod.alpha()


def by_join():
    mod = imp.load_source(\"b\", os.path.join(HERE, \"b.py\"))
    mod.beta()


def by_loader_chain():
    mod = SourceFileLoader(\"c\", str(Path(__file__).parent / \"sub\" / \"c.py\")).load_module()
    mod.gamma()


def by_loader_variable():
    loader = SourceFileLoader(\"d\", \"tools/d.py\")
    mod = loader.load_module()
    mod.delta()


def by_run_path():
    globs = runpy.run_path(\"tools/e.py\")
    globs.epsilon()
";
    let resolution = resolve(&[
        ("tools/loader.py", loader),
        ("tools/a.py", "def alpha():\n    pass\n"),
        ("tools/b.py", "def beta():\n    pass\n"),
        ("tools/sub/c.py", "def gamma():\n    pass\n"),
        ("tools/d.py", "def delta():\n    pass\n"),
        ("tools/e.py", "def epsilon():\n    pass\n"),
    ]);
    for (function, file, callee) in [
        ("by_from_import", "tools/a.py", "alpha"),
        ("by_join", "tools/b.py", "beta"),
        ("by_loader_chain", "tools/sub/c.py", "gamma"),
        ("by_loader_variable", "tools/d.py", "delta"),
    ] {
        assert!(
            has_import(&resolution, "tools/loader.py", file),
            "{function}: no Imports edge to {file}"
        );
        let target = format!("{file}::{callee}");
        assert!(
            has_call(
                &resolution,
                &format!("tools/loader.py::{function}"),
                &target
            ),
            "{function}: no call to {target}; callers: {:?}",
            callers_of(&resolution, &target)
        );
    }
    // `run_path` executes the file — a real dependency — but returns the
    // module's *globals dict*, so `globs.epsilon()` is a dict attribute and
    // must not be read as a call into the file.
    assert!(has_import(&resolution, "tools/loader.py", "tools/e.py"));
    assert!(
        callers_of(&resolution, "tools/e.py::epsilon").is_empty(),
        "a run_path result was bound as a module: {:?}",
        callers_of(&resolution, "tools/e.py::epsilon")
    );
}

/// A path the extractor cannot read literally, a suffix two files share, and a
/// file that is not Python: every one abstains.
#[test]
fn unreadable_or_ambiguous_paths_abstain() {
    let loader = "\
import importlib.util

def dynamic(relative):
    spec = importlib.util.spec_from_file_location(\"x\", relative)
    mod = importlib.util.module_from_spec(spec)
    mod.only_dynamic()

def formatted(name):
    spec = importlib.util.spec_from_file_location(\"x\", f\"pkg/{name}.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.only_formatted()

def shared_suffix():
    spec = importlib.util.spec_from_file_location(\"x\", get_base() / \"dup.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.duplicated()

def not_python():
    spec = importlib.util.spec_from_file_location(\"x\", \"pkg/data.json\")
    mod = importlib.util.module_from_spec(spec)
    mod.from_json()
";
    let resolution = resolve(&[
        ("runner/loader.py", loader),
        ("pkg/dynamic.py", "def only_dynamic():\n    pass\n"),
        ("pkg/formatted.py", "def only_formatted():\n    pass\n"),
        ("one/dup.py", "def duplicated():\n    pass\n"),
        ("two/dup.py", "def duplicated():\n    pass\n"),
        ("pkg/data.json", "{}"),
        ("pkg/from_json.py", "def from_json():\n    pass\n"),
    ]);
    for target in [
        "pkg/dynamic.py",
        "pkg/formatted.py",
        "one/dup.py",
        "two/dup.py",
        "pkg/data.json",
        "pkg/from_json.py",
    ] {
        assert!(
            !has_import(&resolution, "runner/loader.py", target),
            "abstention broke: an Imports edge reached {target}"
        );
    }
    for callee in [
        "pkg/dynamic.py::only_dynamic",
        "pkg/formatted.py::only_formatted",
        "one/dup.py::duplicated",
        "two/dup.py::duplicated",
        "pkg/from_json.py::from_json",
    ] {
        assert!(
            callers_of(&resolution, callee).is_empty(),
            "abstention broke: {callee} gained callers {:?}",
            callers_of(&resolution, callee)
        );
    }
}

/// A unique suffix is enough when the base is unknown; an anchored base picks
/// the one file even where the suffix alone would be ambiguous.
#[test]
fn a_unique_suffix_or_a_known_anchor_resolves() {
    let loader = "\
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def by_suffix():
    spec = importlib.util.spec_from_file_location(\"x\", get_base() / \"lib/unique.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.only_one()

def by_anchor():
    spec = importlib.util.spec_from_file_location(\"x\", ROOT / \"twin.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.twin()
";
    let resolution = resolve(&[
        ("scripts/loader.py", loader),
        ("vendor/lib/unique.py", "def only_one():\n    pass\n"),
        ("twin.py", "def twin():\n    pass\n"),
        ("scripts/twin.py", "def twin():\n    pass\n"),
    ]);
    assert!(has_import(
        &resolution,
        "scripts/loader.py",
        "vendor/lib/unique.py"
    ));
    assert!(has_call(
        &resolution,
        "scripts/loader.py::by_suffix",
        "vendor/lib/unique.py::only_one"
    ));
    // `parents[1]` of `scripts/loader.py` is the repository root.
    assert!(has_import(&resolution, "scripts/loader.py", "twin.py"));
    assert!(!has_import(
        &resolution,
        "scripts/loader.py",
        "scripts/twin.py"
    ));
    assert!(has_call(
        &resolution,
        "scripts/loader.py::by_anchor",
        "twin.py::twin"
    ));
    assert!(callers_of(&resolution, "scripts/twin.py::twin").is_empty());
}

/// An unanchored base that names two indexed files — one beside the loader,
/// one at the root — is "one of several" and abstains.
#[test]
fn an_unanchored_base_matching_two_files_abstains() {
    let loader = "\
import importlib.util

def load():
    spec = importlib.util.spec_from_file_location(\"x\", BASE / \"twin.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.twin()
";
    let resolution = resolve(&[
        ("scripts/loader.py", loader),
        ("twin.py", "def twin():\n    pass\n"),
        ("scripts/twin.py", "def twin():\n    pass\n"),
    ]);
    assert!(!has_import(&resolution, "scripts/loader.py", "twin.py"));
    assert!(!has_import(
        &resolution,
        "scripts/loader.py",
        "scripts/twin.py"
    ));
    assert!(callers_of(&resolution, "twin.py::twin").is_empty());
    assert!(callers_of(&resolution, "scripts/twin.py::twin").is_empty());
}

/// `spec` rebound between the load and `module_from_spec`: the last binding
/// before the use is what counts, and an opaque one abstains.
#[test]
fn a_spec_rebound_before_module_from_spec_uses_the_latest_binding() {
    let loader = "\
import importlib.util

def rebound_to_other():
    spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
    spec = make_spec()
    mod = importlib.util.module_from_spec(spec)
    mod.alpha()

def rebound_to_second_load():
    spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
    spec = importlib.util.spec_from_file_location(\"b\", \"lib/b.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.beta()
    mod.alpha()
";
    let resolution = resolve(&[
        ("lib/loader.py", loader),
        ("lib/a.py", "def alpha():\n    pass\n"),
        ("lib/b.py", "def beta():\n    pass\n"),
    ]);
    assert!(
        callers_of(&resolution, "lib/a.py::alpha").is_empty(),
        "a module was bound through a spec that had been rebound: {:?}",
        callers_of(&resolution, "lib/a.py::alpha")
    );
    assert!(has_call(
        &resolution,
        "lib/loader.py::rebound_to_second_load",
        "lib/b.py::beta"
    ));
    // Both files are still loaded by path, so both are dependencies.
    assert!(has_import(&resolution, "lib/loader.py", "lib/a.py"));
    assert!(has_import(&resolution, "lib/loader.py", "lib/b.py"));
}

/// The handle rebound after the load: a flow-insensitive refusal, because
/// which call sees which value is not something this rung decides.
#[test]
fn a_handle_rebound_after_the_load_abstains() {
    let loader = "\
import importlib.util

def load():
    spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.alpha()
    mod = other_thing()
    mod.alpha()
";
    let resolution = resolve(&[
        ("lib/loader.py", loader),
        ("lib/a.py", "def alpha():\n    pass\n"),
    ]);
    assert!(callers_of(&resolution, "lib/a.py::alpha").is_empty());
    assert!(has_import(&resolution, "lib/loader.py", "lib/a.py"));
}

/// A spec in one function and a same-named local in another must not share a
/// binding: `mod` in `other` is its own variable.
#[test]
fn a_binding_does_not_leak_into_another_function() {
    let loader = "\
import importlib.util

def load():
    spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
    mod = importlib.util.module_from_spec(spec)
    mod.alpha()

def other():
    mod = make_something()
    mod.shared()

def parameter(mod):
    mod.shared()

def unbound():
    mod.shared()
";
    let resolution = resolve(&[
        ("lib/loader.py", loader),
        (
            "lib/a.py",
            "def alpha():\n    pass\n\ndef shared():\n    pass\n",
        ),
    ]);
    assert!(has_call(
        &resolution,
        "lib/loader.py::load",
        "lib/a.py::alpha"
    ));
    assert!(
        callers_of(&resolution, "lib/a.py::shared").is_empty(),
        "a function-scoped handle leaked into another function: {:?}",
        callers_of(&resolution, "lib/a.py::shared")
    );
}

/// A closure reads its enclosing function's handle.
#[test]
fn a_closure_sees_its_enclosing_functions_handle() {
    let loader = "\
import importlib.util

def outer():
    spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
    mod = importlib.util.module_from_spec(spec)

    def inner():
        return mod.alpha()

    return inner
";
    let resolution = resolve(&[
        ("lib/loader.py", loader),
        ("lib/a.py", "def alpha():\n    pass\n"),
    ]);
    assert!(
        callers_of(&resolution, "lib/a.py::alpha")
            .iter()
            .any(|caller| caller.starts_with("lib/loader.py::outer")),
        "callers: {:?}",
        callers_of(&resolution, "lib/a.py::alpha")
    );
}
