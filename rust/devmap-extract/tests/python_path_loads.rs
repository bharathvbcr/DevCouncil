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

fn search_directories(extraction: &Extraction) -> Vec<(String, Option<u32>)> {
    path_loads(extraction)
        .into_iter()
        .filter(|import| {
            import.path_load.as_ref().unwrap().kind
                == devmap_extract::model::PathLoadKind::SearchDirectory
        })
        .map(|import| {
            (
                import.module_specifier.clone(),
                import.path_load.as_ref().unwrap().anchor_up,
            )
        })
        .collect()
}

/// Each readable module-level `sys.path` write is one `SearchDirectory`
/// import carrying its anchor; the BINN spellings all read.
#[test]
fn sys_path_inserts_record_their_directory_and_anchor() {
    let source = "\
import os
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), \"..\"))
sys.path.append(os.path.join(HERE, \"lib\"))
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT / \"scripts\"))
sys.path[:0] = [str(ROOT / \"vendor\")]
";
    let extraction = extract_file("scripts/test_x.py", source);
    assert_eq!(
        search_directories(&extraction),
        [
            ("scripts/aws".to_string(), Some(1)),
            (String::new(), Some(1)),
            (String::new(), Some(0)),
            ("..".to_string(), Some(0)),
            ("lib".to_string(), Some(0)),
            ("scripts".to_string(), Some(1)),
            ("vendor".to_string(), Some(1)),
        ]
    );
}

/// Any write this pass cannot follow withdraws every entry in the file.
#[test]
fn an_unfollowable_sys_path_write_withdraws_every_entry() {
    for (label, write) in [
        ("reassigned", "sys.path = [\"/opt\"]\n"),
        ("augmented", "sys.path += [\"/opt\"]\n"),
        ("removed", "sys.path.remove(\"/opt\")\n"),
        ("popped", "sys.path.pop(0)\n"),
        ("deleted", "del sys.path[0]\n"),
        ("item set", "sys.path[0] = \"/opt\"\n"),
        (
            "removed in a function",
            "def f():\n    sys.path.remove(\"/opt\")\n",
        ),
    ] {
        let source = format!(
            "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n\
             sys.path.insert(0, str(ROOT / \"scripts\"))\n{write}"
        );
        let extraction = extract_file("scripts/test_x.py", &source);
        assert!(
            search_directories(&extraction).is_empty(),
            "{label}: {:?}",
            search_directories(&extraction)
        );
    }
}

/// A module-level insert whose directory this pass cannot read keeps the
/// entries it did read and adds one file-wide veto: an empty specifier with no
/// anchor, which the resolver reads as "any directory may be on the path".
#[test]
fn an_unreadable_module_level_insert_keeps_the_entries_and_adds_a_veto() {
    for (label, write) in [
        ("unreadable insert", "sys.path.insert(0, somewhere)\n"),
        ("cwd-relative insert", "sys.path.insert(0, \"scripts\")\n"),
    ] {
        let source = format!(
            "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n\
             sys.path.insert(0, str(ROOT / \"scripts\"))\n{write}"
        );
        let extraction = extract_file("scripts/test_x.py", &source);
        assert_eq!(
            search_directories(&extraction),
            [("scripts".to_string(), Some(1)), (String::new(), None)],
            "{label}"
        );
    }
}

/// An insert inside a function or class body is recorded with a scope — the
/// resolver's veto — and one it cannot read has no anchor and no directory.
/// Neither withdraws the module-level entries.
#[test]
fn in_function_sys_path_inserts_are_recorded_as_scoped_vetoes() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))

class Tooling:
    sys.path.insert(0, str(ROOT / \"vendor\"))

    def test_azure(self):
        sys.path.insert(0, str(ROOT / \"scripts\" / \"azure\"))

    def test_anywhere(self, where):
        sys.path.append(where)
";
    let extraction = extract_file("scripts/test_x.py", source);
    let entries: Vec<(String, Option<u32>, Option<String>)> = path_loads(&extraction)
        .into_iter()
        .filter_map(|import| {
            let load = import.path_load.as_ref()?;
            (load.kind == devmap_extract::model::PathLoadKind::SearchDirectory).then(|| {
                (
                    import.module_specifier.clone(),
                    load.anchor_up,
                    load.scope.clone(),
                )
            })
        })
        .collect();
    assert_eq!(entries.len(), 4, "{entries:?}");
    assert_eq!(entries[0], ("scripts/aws".to_string(), Some(1), None));
    // A class body: scoped to the file, and read as unreadable.
    assert_eq!(entries[1].0, "");
    assert_eq!(entries[1].1, None);
    assert!(entries[1].2.is_some(), "{entries:?}");
    // A readable insert in a method keeps its directory and names the method.
    assert_eq!(entries[2].0, "scripts/azure");
    assert_eq!(entries[2].1, Some(1));
    assert!(
        entries[2]
            .2
            .as_deref()
            .is_some_and(|scope| scope.ends_with("test_azure")),
        "{entries:?}"
    );
    // An unreadable one in a method: no anchor, no directory.
    assert_eq!(entries[3].0, "");
    assert_eq!(entries[3].1, None);
    assert!(
        entries[3]
            .2
            .as_deref()
            .is_some_and(|scope| scope.ends_with("test_anywhere")),
        "{entries:?}"
    );
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

/// Every `SearchDirectory` row as `(specifier, anchor_up, scope, start_byte)`,
/// in the order the extraction lists them.
fn search_rows(extraction: &Extraction) -> Vec<(String, Option<u32>, Option<String>, usize)> {
    path_loads(extraction)
        .into_iter()
        .filter_map(|import| {
            let load = import.path_load.as_ref()?;
            (load.kind == devmap_extract::model::PathLoadKind::SearchDirectory).then(|| {
                (
                    import.module_specifier.clone(),
                    load.anchor_up,
                    load.scope.clone(),
                    import.span.start_byte,
                )
            })
        })
        .collect()
}

fn rows_without_spans(extraction: &Extraction) -> Vec<(String, Option<u32>, Option<String>)> {
    search_rows(extraction)
        .into_iter()
        .map(|(tail, anchor, scope, _)| (tail, anchor, scope))
        .collect()
}

const WAVE20_PRELUDE: &str = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
";

/// T1. The veto row is the first unreadable insert's own, in document order.
#[test]
fn an_unreadable_module_level_insert_emits_a_file_wide_veto_row() {
    let source =
        format!("{WAVE20_PRELUDE}sys.path.insert(0, somewhere)\nimport analyse_wave20 as a20\n");
    let extraction = extract_file("scripts/test_x.py", &source);
    assert_eq!(
        rows_without_spans(&extraction),
        [
            ("scripts/aws".to_string(), Some(1), None),
            (String::new(), None, None)
        ]
    );
    let rows = search_rows(&extraction);
    assert_eq!(
        rows[1].3,
        source.find("sys.path.insert(0, somewhere)").unwrap()
    );
}

/// T5. Several unreadable inserts mixed with readable ones: every readable
/// entry stays and there is exactly one veto, at the first unreadable insert.
#[test]
fn several_unreadable_inserts_emit_one_veto_at_the_first() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, one)
sys.path.insert(0, str(ROOT / \"a\"))
sys.path.insert(0, two)
sys.path.insert(0, str(ROOT / \"b\"))
";
    let extraction = extract_file("scripts/test_x.py", source);
    let rows = search_rows(&extraction);
    let tails: Vec<(&str, Option<u32>)> = rows.iter().map(|r| (r.0.as_str(), r.1)).collect();
    assert_eq!(
        tails,
        [("", None), ("a", Some(1)), ("b", Some(1))],
        "{rows:?}"
    );
    assert_eq!(rows[0].3, source.find("sys.path.insert(0, one)").unwrap());
}

/// T7. Scope is kept on every row kind alongside a module-level veto.
#[test]
fn a_module_level_veto_keeps_scoped_rows_and_module_entries() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
sys.path.insert(0, somewhere)

class Tooling:
    sys.path.insert(0, str(ROOT / \"vendor\"))

def test_anywhere(where):
    sys.path.append(where)
";
    let extraction = extract_file("scripts/test_x.py", source);
    let rows = rows_without_spans(&extraction);
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert_eq!(rows[0], ("scripts/aws".to_string(), Some(1), None));
    assert_eq!(rows[1], (String::new(), None, None));
    assert_eq!((rows[2].0.as_str(), rows[2].1), ("", None));
    assert!(rows[2].2.is_some(), "{rows:?}");
    assert_eq!((rows[3].0.as_str(), rows[3].1), ("", None));
    assert!(
        rows[3]
            .2
            .as_deref()
            .is_some_and(|scope| scope.ends_with("test_anywhere")),
        "{rows:?}"
    );
}

/// T9. Unicode, spaces and oversized literals: anchored ones read, bare ones
/// are unreadable and veto.
#[test]
fn unicode_and_odd_directories_read_or_veto_by_anchor() {
    let readable = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"caf\u{e9}\"))
sys.path.insert(0, str(ROOT / \"my dir\"))
sys.path.insert(0, str(ROOT / \"..\" / \"\u{76ee}\u{5f55}\"))
";
    let extraction = extract_file("scripts/test_x.py", readable);
    assert_eq!(
        rows_without_spans(&extraction),
        [
            ("caf\u{e9}".to_string(), Some(1), None),
            ("my dir".to_string(), Some(1), None),
            ("../\u{76ee}\u{5f55}".to_string(), Some(1), None),
        ]
    );
    let long = "x".repeat(5000);
    for (label, bare) in [
        ("bare unicode", "\"caf\u{e9}\"".to_string()),
        (
            "absolute with spaces and unicode",
            "\"/opt/my dir/\u{76ee}\u{5f55}\"".to_string(),
        ),
        ("windows drive", "\"C:\\\\x\"".to_string()),
        ("5000-byte literal", format!("\"{long}\"")),
    ] {
        let source = format!("{WAVE20_PRELUDE}sys.path.insert(0, {bare})\n");
        let extraction = extract_file("scripts/test_x.py", &source);
        assert_eq!(
            rows_without_spans(&extraction),
            [
                ("scripts/aws".to_string(), Some(1), None),
                (String::new(), None, None)
            ],
            "{label}"
        );
    }
}

/// T10. A syntax error after a real unreadable insert still emits the veto.
#[test]
fn a_syntax_error_after_an_unreadable_insert_keeps_the_veto() {
    let source = format!(
        "{WAVE20_PRELUDE}sys.path.insert(0, somewhere)\nimport analyse_wave20 as a20\ndef broken(:\n    pass\n"
    );
    let extraction = extract_file("scripts/test_x.py", &source);
    assert!(
        matches!(
            extraction.parse_outcome,
            devmap_extract::model::ParseOutcome::Partial { .. }
        ),
        "{:?}",
        extraction.parse_outcome
    );
    assert_eq!(
        rows_without_spans(&extraction),
        [
            ("scripts/aws".to_string(), Some(1), None),
            (String::new(), None, None)
        ]
    );
}

/// T11. A NUL byte is refused before parsing, so the file publishes no import
/// a later one could bind. Characterisation of the `Failed` path.
#[test]
fn a_nul_byte_publishes_no_imports_at_all() {
    let source = format!(
        "{WAVE20_PRELUDE}sys.path.insert(0, somewhere)\nimport analyse_wave20 as a20\n\0\n"
    );
    let extraction = extract_file("scripts/test_x.py", &source);
    assert!(
        matches!(
            extraction.parse_outcome,
            devmap_extract::model::ParseOutcome::Failed { .. }
        ),
        "{:?}",
        extraction.parse_outcome
    );
    assert!(extraction.imports.is_empty(), "{:?}", extraction.imports);
}

// ---------------------------------------------------------------------------
// Attack lane A (ft-87363ce5): hostile inputs around the unreadable-insert veto.
// ---------------------------------------------------------------------------

const ATK_PRELUDE: &str = "\
import os
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
";

fn atk_rows(source: &str) -> Vec<(String, Option<u32>, Option<String>)> {
    rows_without_spans(&extract_file("scripts/test_x.py", source))
}

fn atk_aws() -> (String, Option<u32>, Option<String>) {
    ("scripts/aws".to_string(), Some(1), None)
}

fn atk_veto() -> (String, Option<u32>, Option<String>) {
    (String::new(), None, None)
}

/// Inserts the pass cannot pin down each add exactly one veto and keep the
/// readable entry, whatever shape the unreadable expression takes.
#[test]
fn atk_hostile_unreadable_expressions_each_veto_once_and_keep_the_entry() {
    let very_long = format!("os.path.join({})", vec!["\"a\""; 400].join(", "));
    let cases: Vec<(&str, String)> = vec![
        (
            "f-string",
            "sys.path.insert(0, f\"{os.environ['HOME']}/x\")".into(),
        ),
        (
            "os.environ",
            "sys.path.insert(0, os.environ[\"EXTRA\"])".into(),
        ),
        (
            "os.environ.get",
            "sys.path.insert(0, os.environ.get(\"EXTRA\", \"\"))".into(),
        ),
        ("os.getcwd", "sys.path.insert(0, os.getcwd())".into()),
        ("absolute posix", "sys.path.insert(0, \"/opt/lib\")".into()),
        (
            "backslash relative",
            "sys.path.insert(0, \"scripts\\\\aws\")".into(),
        ),
        ("very long join", format!("sys.path.insert(0, {very_long})")),
        ("name from argv", "sys.path.insert(0, sys.argv[1])".into()),
        ("call result", "sys.path.insert(0, compute_dir())".into()),
        (
            "conditional expression",
            "sys.path.insert(0, a if flag else b)".into(),
        ),
        ("append", "sys.path.append(somewhere)".into()),
        (
            "slice assign unreadable",
            "sys.path[:0] = [somewhere]".into(),
        ),
    ];
    for (label, line) in cases {
        let source = format!("{ATK_PRELUDE}{line}\n");
        let rows = atk_rows(&source);
        assert_eq!(rows, [atk_aws(), atk_veto()], "{label}: {rows:?}");
    }
}

/// Module-level compound statements still count as module level.
#[test]
fn atk_unreadable_insert_in_module_level_blocks_vetoes_the_file() {
    for (label, block) in [
        (
            "if __main__",
            "if __name__ == \"__main__\":\n    sys.path.insert(0, somewhere)\n",
        ),
        (
            "try",
            "try:\n    sys.path.insert(0, somewhere)\nexcept Exception:\n    pass\n",
        ),
        (
            "except arm",
            "try:\n    pass\nexcept Exception:\n    sys.path.insert(0, somewhere)\n",
        ),
        ("with", "with ctx():\n    sys.path.insert(0, somewhere)\n"),
        ("for", "for p in paths:\n    sys.path.insert(0, p)\n"),
        (
            "while",
            "while cond:\n    sys.path.insert(0, p)\n    break\n",
        ),
        (
            "else arm",
            "if flag:\n    pass\nelse:\n    sys.path.insert(0, somewhere)\n",
        ),
    ] {
        let source = format!("{ATK_PRELUDE}{block}");
        let rows = atk_rows(&source);
        assert_eq!(rows, [atk_aws(), atk_veto()], "{label}: {rows:?}");
    }
}

/// A readable insert nested in a block, then an unreadable one, and the
/// reverse: document order of the rows follows the file.
#[test]
fn atk_readable_and_unreadable_order_follows_the_file() {
    let a = "sys.path.insert(0, somewhere)\n";
    let b = "if flag:\n    sys.path.insert(0, str(ROOT / \"vendor\"))\n";
    let source = format!(
        "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n{a}{b}"
    );
    assert_eq!(
        atk_rows(&source),
        [atk_veto(), ("vendor".to_string(), Some(1), None)]
    );
}

/// An unreadable module-level insert plus a function-scoped readable one: the
/// scoped row keeps its scope and anchor; the module veto has neither.
#[test]
fn atk_function_scoped_readable_insert_alongside_a_module_veto() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, somewhere)

def f():
    sys.path.insert(0, str(ROOT / \"vendor\"))
";
    let rows = atk_rows(source);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0], atk_veto());
    assert_eq!(rows[1].0, "vendor");
    assert_eq!(rows[1].1, Some(1));
    assert!(rows[1].2.is_some());
}

/// `..` escapes and `parents[N]` past the root are readable at the extractor
/// (the resolver turns them into a veto); they never become veto rows here.
#[test]
fn atk_escapes_above_the_root_are_anchored_rows_not_extractor_vetoes() {
    for (label, line) in [
        (
            "dotdot twice",
            "sys.path.insert(0, str(ROOT / \"..\" / \"..\" / \"..\" / \"lib\"))",
        ),
        (
            "parents[5]",
            "sys.path.insert(0, str(Path(__file__).resolve().parents[5] / \"lib\"))",
        ),
    ] {
        let rows = atk_rows(&format!("{ATK_PRELUDE}{line}\n"));
        assert_eq!(rows.len(), 2, "{label}: {rows:?}");
        assert_eq!(rows[0], atk_aws(), "{label}");
        assert!(rows[1].1.is_some(), "{label}: {rows:?}");
    }
}

/// CRLF line endings, a UTF-8 BOM, and both together read like LF text.
#[test]
fn atk_crlf_and_bom_files_keep_the_veto() {
    let lf = format!("{ATK_PRELUDE}sys.path.insert(0, somewhere)\nimport helper\n");
    let crlf = lf.replace('\n', "\r\n");
    let bom = format!("\u{feff}{lf}");
    let both = format!("\u{feff}{crlf}");
    for (label, source) in [("lf", lf), ("crlf", crlf), ("bom", bom), ("bom+crlf", both)] {
        let rows = atk_rows(&source);
        assert_eq!(rows, [atk_aws(), atk_veto()], "{label}: {rows:?}");
    }
}

/// A syntax error BEFORE the unreadable insert: whatever the parser recovers,
/// no row may silently drop the veto while keeping the readable entry.
#[test]
fn atk_a_syntax_error_before_the_insert_never_keeps_the_entry_without_the_veto() {
    let source = format!(
        "def broken(:\n    pass\n{ATK_PRELUDE}sys.path.insert(0, somewhere)\nimport analyse_wave20\n"
    );
    let rows = atk_rows(&source);
    assert!(
        rows.is_empty() || rows == [atk_aws(), atk_veto()],
        "an entry survived without the veto: {rows:?}"
    );
}

/// The cap boundary: 63 readable + 1 unreadable = 64 keeps every row; 64
/// readable + 1 unreadable and 65 readable withdraw all rows; 1 readable + 64
/// unreadable collapses to one veto and stays under the cap.
#[test]
fn atk_cap_boundary_mixes_readable_and_unreadable() {
    let build = |readable: usize, unreadable: usize| -> String {
        let mut s = String::from(
            "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n",
        );
        for i in 0..readable {
            s.push_str(&format!("sys.path.insert(0, str(ROOT / \"d{i}\"))\n"));
        }
        for _ in 0..unreadable {
            s.push_str("sys.path.insert(0, somewhere)\n");
        }
        s
    };
    assert_eq!(atk_rows(&build(63, 1)).len(), 64);
    assert!(atk_rows(&build(64, 0)).len() == 64);
    assert!(atk_rows(&build(64, 1)).is_empty());
    assert!(atk_rows(&build(65, 0)).is_empty());
    assert!(atk_rows(&build(65, 5)).is_empty());
    let many_unreadable = atk_rows(&build(1, 200));
    assert_eq!(many_unreadable.len(), 2, "{many_unreadable:?}");
    assert_eq!(many_unreadable.iter().filter(|r| r.1.is_none()).count(), 1);
}

/// A write the pass cannot follow, anywhere, still emits nothing even with an
/// unreadable insert (the veto would be dropped by the resolver anyway).
#[test]
fn atk_an_unfollowable_write_wins_over_an_unreadable_insert() {
    for tail in [
        "sys.path.remove(\"/opt\")\n",
        "sys.path = []\n",
        "del sys.path[0]\n",
    ] {
        let before = format!("{ATK_PRELUDE}sys.path.insert(0, somewhere)\n{tail}");
        let after = format!("{ATK_PRELUDE}{tail}sys.path.insert(0, somewhere)\n");
        assert!(atk_rows(&before).is_empty(), "{tail:?} after");
        assert!(atk_rows(&after).is_empty(), "{tail:?} before");
    }
}
