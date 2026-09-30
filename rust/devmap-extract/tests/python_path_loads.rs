//! What the extractor records for a Python module loaded by file path.
//!
//! The resolver joins these imports to use sites by scope identity, so the
//! load-bearing claim here is that the `scope` an import records is the same
//! string a call inside that function records as its `caller_symbol` and a
//! use site records as `LocalBinding::scope`. If those three drift apart, every
//! function-scoped handle silently stops resolving — or, worse, one function's
//! handle is read in another's.

use devmap_extract::extract_file;
use devmap_extract::model::{ExtractedImport, Extraction};

fn path_loads(extraction: &Extraction) -> Vec<&ExtractedImport> {
    extraction
        .imports
        .iter()
        .filter(|import| import.path_load.is_some())
        .collect()
}

fn bound<'a>(extraction: &'a Extraction, alias: &str) -> Vec<&'a ExtractedImport> {
    path_loads(extraction)
        .into_iter()
        .filter(|import| import.alias.as_deref() == Some(alias))
        .collect()
}

/// The BINN shape: the path is `ROOT / "scripts/x.py"` with `ROOT` anchored on
/// `__file__`, so the specifier is the literal tail and the anchor says where
/// it is relative to.
#[test]
fn the_binn_shape_records_the_literal_tail_and_its_anchor() {
    let source = "\
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def supplement():
    spec = importlib.util.spec_from_file_location(
        \"build_supplement\", ROOT / \"scripts/build_supplement.py\")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.rename_project(\"x\")
    return module
";
    let extraction = extract_file("scripts/build_paper.py", source);
    let handles = bound(&extraction, "module");
    assert_eq!(handles.len(), 1, "imports: {:?}", extraction.imports);
    let handle = handles[0];
    assert_eq!(handle.module_specifier, "scripts/build_supplement.py");
    let load = handle.path_load.as_ref().unwrap();
    // `parents[1]` of the file is two directories up from it: one up from its
    // own directory.
    assert_eq!(load.anchor_up, Some(1));

    // The three scope strings agree.
    let scope = load
        .scope
        .as_deref()
        .expect("a function-scoped handle has a scope");
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "rename_project")
        .expect("the call through the handle is extracted");
    assert_eq!(call.caller_symbol.as_deref(), Some(scope));
    assert_eq!(call.receiver_expr.as_deref(), Some("module"));
    let site = extraction
        .local_binding_at(call.span.start_byte, "module")
        .expect("`module` is a local of `supplement` at the call site");
    assert_eq!(site.scope.as_deref(), Some(scope));

    // A loader function: `supplement` is itself bound at module level.
    let loader = bound(&extraction, "supplement");
    assert_eq!(loader.len(), 1, "imports: {:?}", extraction.imports);
    assert_eq!(loader[0].path_load.as_ref().unwrap().scope, None);
}

/// Each path spelling the task names.
#[test]
fn every_path_spelling_reduces_to_its_literal_tail() {
    let cases: &[(&str, &str, Option<u32>)] = &[
        ("\"x/y.py\"", "x/y.py", None),
        ("ROOT / \"scripts/x.py\"", "scripts/x.py", Some(1)),
        ("Path(__file__).parent / \"x.py\"", "x.py", Some(0)),
        ("os.path.join(HERE, \"x.py\")", "x.py", Some(0)),
        ("HERE_PATH / \"sub\" / \"x.py\"", "sub/x.py", Some(0)),
        (
            "str(Path(__file__).resolve().parent.parent / \"x.py\")",
            "x.py",
            Some(1),
        ),
        (
            "os.path.join(os.path.dirname(__file__), \"a\", \"b.py\")",
            "a/b.py",
            Some(0),
        ),
        (
            "Path(__file__).parent.joinpath(\"j\", \"k.py\")",
            "j/k.py",
            Some(0),
        ),
        ("opaque / \"./z.py\"", "z.py", None),
    ];
    for (expression, specifier, anchor_up) in cases {
        let source = format!(
            "import os\nimport importlib.util\nfrom pathlib import Path\n\
             ROOT = Path(__file__).resolve().parents[1]\n\
             HERE = os.path.dirname(os.path.abspath(__file__))\n\
             HERE_PATH = Path(__file__).parent\n\
             spec = importlib.util.spec_from_file_location(\"m\", {expression})\n\
             mod = importlib.util.module_from_spec(spec)\n"
        );
        let extraction = extract_file("pkg/loader.py", &source);
        let handles = bound(&extraction, "mod");
        assert_eq!(
            handles.len(),
            1,
            "{expression}: expected one bound load, got {:?}",
            extraction.imports
        );
        assert_eq!(&handles[0].module_specifier, specifier, "{expression}");
        let load = handles[0].path_load.as_ref().unwrap();
        assert_eq!(&load.anchor_up, anchor_up, "{expression}");
        assert_eq!(
            load.scope, None,
            "{expression}: a module global has no scope"
        );
    }
}

/// Everything the extractor cannot read literally records nothing at all —
/// not even the file edge, because there is no file to name.
#[test]
fn unreadable_paths_record_nothing() {
    for expression in [
        "relative",
        "f\"pkg/{name}.py\"",
        "F\"pkg/x.py\"",
        "b\"pkg/x.py\"",
        "\"pkg/\" \"x.py\"",
        "\"pkg/\" + \"x.py\"",
        "\"pkg/data.json\"",
        "\"/abs/x.py\"",
        "Path(\"/abs\") / \"x.py\"",
        "\"C:\\\\x.py\"",
        "\"pkg\\\\x.py\"",
        "__file__ / \"x.py\"",
        "ROOT / name",
        "ROOT / \"pkg\" / name",
        "\"pkg/.py\"",
        "*parts",
    ] {
        let source = format!(
            "import importlib.util\nfrom pathlib import Path\n\
             ROOT = Path(__file__).parent\n\
             spec = importlib.util.spec_from_file_location(\"m\", {expression})\n\
             mod = importlib.util.module_from_spec(spec)\n"
        );
        let extraction = extract_file("pkg/loader.py", &source);
        assert!(
            path_loads(&extraction).is_empty(),
            "{expression}: recorded {:?}",
            path_loads(&extraction)
        );
    }
}

/// A receiver that is not the loader module is somebody else's method.
#[test]
fn a_foreign_method_of_the_same_name_is_not_a_load() {
    let source = "\
class Runner:
    def go(self):
        self.run_path(\"x.py\")
        tool.spec_from_file_location(\"m\", \"y.py\")
";
    let extraction = extract_file("pkg/runner.py", source);
    assert!(
        path_loads(&extraction).is_empty(),
        "recorded {:?}",
        path_loads(&extraction)
    );
}

/// `run_path` is a dependency, but it returns a globals dict, so nothing is
/// bound: the import exists and has no alias.
#[test]
fn run_path_records_the_file_and_binds_nothing() {
    let source = "\
import runpy
globs = runpy.run_path(\"tools/e.py\")
";
    let extraction = extract_file("tools/loader.py", source);
    let loads = path_loads(&extraction);
    assert_eq!(loads.len(), 1, "{loads:?}");
    assert_eq!(loads[0].module_specifier, "tools/e.py");
    assert_eq!(loads[0].alias, None);
}

/// A handle bound to two different files, or rebound to something opaque
/// after its load, is not bound. Both files are still recorded as loaded.
#[test]
fn a_conflicting_or_rebound_handle_is_not_bound() {
    let source = "\
import importlib.util

def two_files(flag):
    if flag:
        spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
        mod = importlib.util.module_from_spec(spec)
    else:
        spec = importlib.util.spec_from_file_location(\"b\", \"lib/b.py\")
        mod = importlib.util.module_from_spec(spec)
    mod.run()

def rebound():
    spec = importlib.util.spec_from_file_location(\"c\", \"lib/c.py\")
    other = importlib.util.module_from_spec(spec)
    for other in range(3):
        pass
";
    let extraction = extract_file("lib/loader.py", source);
    assert!(
        bound(&extraction, "mod").is_empty(),
        "{:?}",
        extraction.imports
    );
    assert!(
        bound(&extraction, "other").is_empty(),
        "{:?}",
        extraction.imports
    );
    let mut files: Vec<&str> = path_loads(&extraction)
        .iter()
        .map(|import| import.module_specifier.as_str())
        .collect();
    files.sort();
    assert_eq!(files, ["lib/a.py", "lib/b.py", "lib/c.py"]);
}

/// A class attribute is neither a local nor a module global.
#[test]
fn a_class_body_binding_is_not_bound() {
    let source = "\
import importlib.util
spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")

class Holder:
    mod = importlib.util.module_from_spec(spec)
";
    let extraction = extract_file("lib/loader.py", source);
    assert!(
        bound(&extraction, "mod").is_empty(),
        "{:?}",
        extraction.imports
    );
    assert_eq!(path_loads(&extraction).len(), 1);
}

/// A loader function rebound at module level is no longer the function read.
#[test]
fn a_rebound_loader_function_is_not_a_loader() {
    let source = "\
import importlib.util

def load():
    spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
    module = importlib.util.module_from_spec(spec)
    return module

load = something_else
";
    let extraction = extract_file("lib/loader.py", source);
    assert!(
        bound(&extraction, "load").is_empty(),
        "{:?}",
        extraction.imports
    );
}

/// Emission is deterministic: the same source yields the same imports.
#[test]
fn emission_is_deterministic() {
    let source = "\
import importlib.util, runpy
def a():
    spec = importlib.util.spec_from_file_location(\"a\", \"lib/a.py\")
    m = importlib.util.module_from_spec(spec)
    return m
def b():
    runpy.run_path(\"lib/b.py\")
x = a()
";
    let first = format!("{:?}", path_loads(&extract_file("lib/l.py", source)));
    for _ in 0..5 {
        assert_eq!(
            first,
            format!("{:?}", path_loads(&extract_file("lib/l.py", source)))
        );
    }
}

/// A module-level path constant carries its anchor into every use, at module
/// level and inside a method.
#[test]
fn a_path_constant_records_its_literal_tail_and_anchor() {
    let source = "\
import importlib.util
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUILDER = ROOT / \"scripts/build_paper.py\"

spec = importlib.util.spec_from_file_location(\"build_paper\", BUILDER)
bp = importlib.util.module_from_spec(spec)

class T:
    @staticmethod
    def _builder():
        spec = importlib.util.spec_from_file_location(\"build_paper\", str(BUILDER))
        mod = importlib.util.module_from_spec(spec)
        return mod
";
    let extraction = extract_file("scripts/test_paper_build.py", source);
    for alias in ["bp", "mod"] {
        let handles = bound(&extraction, alias);
        assert_eq!(handles.len(), 1, "{alias}: {:?}", extraction.imports);
        assert_eq!(handles[0].module_specifier, "scripts/build_paper.py");
        assert_eq!(handles[0].path_load.as_ref().unwrap().anchor_up, Some(1));
    }
}

/// Each call of a parameterised loader is its own import, bound to its own
/// handle; the loader's name is never bound, because it names a different
/// file at every call.
#[test]
fn a_parameterised_loader_records_one_import_per_call_site() {
    let source = "\
import importlib.util
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    mod = importlib.util.module_from_spec(spec)
    return mod

W21 = load(\"analyse_wave21\", \"scripts/aws/analyse_wave21.py\")
W20 = load(name=\"analyse_wave20\", relative=\"scripts/aws/analyse_wave20.py\")
load(\"analyse_wave19\", \"scripts/aws/analyse_wave19.py\")
";
    let extraction = extract_file("scripts/test_wave21_analyser.py", source);
    for (alias, specifier) in [
        ("W21", "scripts/aws/analyse_wave21.py"),
        ("W20", "scripts/aws/analyse_wave20.py"),
    ] {
        let handles = bound(&extraction, alias);
        assert_eq!(handles.len(), 1, "{alias}: {:?}", extraction.imports);
        assert_eq!(handles[0].module_specifier, specifier);
        assert_eq!(handles[0].path_load.as_ref().unwrap().anchor_up, Some(1));
    }
    assert!(
        bound(&extraction, "load").is_empty(),
        "{:?}",
        extraction.imports
    );
    // The unbound call is still a dependency.
    assert!(
        path_loads(&extraction)
            .iter()
            .any(
                |import| import.module_specifier == "scripts/aws/analyse_wave19.py"
                    && import.alias.is_none()
            ),
        "{:?}",
        extraction.imports
    );
    // The `def` itself names no file.
    assert_eq!(path_loads(&extraction).len(), 3, "{:?}", extraction.imports);
}
