//! The two path-load shapes that dominate a real research repository.
//!
//! Measured on BINN: of its path-load sites, most were either
//!
//! * a **module-level path constant** — `BUILDER = ROOT / "scripts/x.py"`
//!   then `spec_from_file_location("x", BUILDER)`, at module level and inside
//!   a staticmethod (`test_paper_build.py`); or
//! * a **parameterised loader** — `def load(name, relative): spec =
//!   spec_from_file_location(name, ROOT / relative) ... return mod`, called as
//!   `W21 = load("analyse_wave21", "scripts/aws/analyse_wave21.py")`
//!   (`test_wave21_analyser.py`, `test_reproduction_check.py`,
//!   `did_confidence_intervals.py`).
//!
//! Both abstained, because the path at the `spec_from_file_location` call is
//! not a literal. Each positive case is paired with the abstention that bounds
//! it: a rebound constant, a non-literal argument, a parameter that is not the
//! path tail, and a same-named `load` that is not a path loader.

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

fn import_count(resolution: &ResolutionResult, from: &str, to: &str) -> usize {
    resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Imports
                && edge.source_file == from
                && edge.target_file == to
        })
        .count()
}

fn callers_of(resolution: &ResolutionResult, callee: &str) -> Vec<String> {
    resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol == callee)
        .map(|edge| edge.source_symbol.clone())
        .collect()
}

fn has_call(resolution: &ResolutionResult, caller: &str, callee: &str) -> bool {
    callers_of(resolution, callee).iter().any(|c| c == caller)
}

/// `BINN/scripts/test_paper_build.py:15-29` and `:409-414`, in shape.
const PAPER_BUILD: &str = "\
import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUILDER = ROOT / \"scripts/build_paper.py\"

spec = importlib.util.spec_from_file_location(\"build_paper\", BUILDER)
bp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bp)


class PureGuardTest(unittest.TestCase):
    def test_guard(self):
        bp.pure_guard()

    @staticmethod
    def _builder():
        spec = importlib.util.spec_from_file_location(\"build_paper\", BUILDER)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        mod.inside_builder()
        return mod
";

const BUILD_PAPER: &str = "\
def pure_guard():
    pass

def inside_builder():
    pass
";

#[test]
fn a_module_level_path_constant_is_read_at_module_level_and_in_a_staticmethod() {
    let resolution = resolve(&[
        ("scripts/test_paper_build.py", PAPER_BUILD),
        ("scripts/build_paper.py", BUILD_PAPER),
    ]);
    assert_eq!(
        import_count(
            &resolution,
            "scripts/test_paper_build.py",
            "scripts/build_paper.py"
        ),
        1
    );
    assert!(
        has_call(
            &resolution,
            "scripts/test_paper_build.py::PureGuardTest.test_guard",
            "scripts/build_paper.py::pure_guard"
        ),
        "callers: {:?}",
        callers_of(&resolution, "scripts/build_paper.py::pure_guard")
    );
    assert!(
        callers_of(&resolution, "scripts/build_paper.py::inside_builder")
            .iter()
            .any(|caller| caller.ends_with("_builder")),
        "callers: {:?}",
        callers_of(&resolution, "scripts/build_paper.py::inside_builder")
    );
}

/// `REPO / "scripts" / "x.py"`: a constant built of several literal segments.
#[test]
fn a_multi_segment_path_constant_is_read() {
    let source = "\
import importlib.util
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
GATE = REPO / \"scripts\" / \"check_terminology.py\"

spec = importlib.util.spec_from_file_location(\"check_terminology\", GATE)
ct = importlib.util.module_from_spec(spec)

def test_gate():
    ct.check()
";
    let resolution = resolve(&[
        ("scripts/test_terminology.py", source),
        ("scripts/check_terminology.py", "def check():\n    pass\n"),
    ]);
    assert!(has_call(
        &resolution,
        "scripts/test_terminology.py::test_gate",
        "scripts/check_terminology.py::check"
    ));
}

/// Every way a constant stops being a constant, and a non-Python target.
#[test]
fn a_rebound_global_or_non_python_constant_abstains() {
    for (label, extra) in [
        ("rebound at module level", "BUILDER = somewhere_else()\n"),
        (
            "rebound through global",
            "def retarget():\n    global BUILDER\n    BUILDER = somewhere_else()\n",
        ),
        ("augmented", "BUILDER += \"x\"\n"),
    ] {
        let source = format!(
            "import importlib.util\nfrom pathlib import Path\n\
             ROOT = Path(__file__).resolve().parent.parent\n\
             BUILDER = ROOT / \"scripts/build_paper.py\"\n\
             {extra}\
             spec = importlib.util.spec_from_file_location(\"build_paper\", BUILDER)\n\
             bp = importlib.util.module_from_spec(spec)\n\
             def test_guard():\n    bp.pure_guard()\n"
        );
        let resolution = resolve(&[
            ("scripts/test_paper_build.py", &source),
            ("scripts/build_paper.py", BUILD_PAPER),
        ]);
        assert_eq!(
            import_count(
                &resolution,
                "scripts/test_paper_build.py",
                "scripts/build_paper.py"
            ),
            0,
            "{label}: a rebound constant was read"
        );
        assert!(
            callers_of(&resolution, "scripts/build_paper.py::pure_guard").is_empty(),
            "{label}"
        );
    }

    let shell = "\
import importlib.util
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
CHECK = ROOT / \"scripts/check_gc4.sh\"
spec = importlib.util.spec_from_file_location(\"check_gc4\", CHECK)
mod = importlib.util.module_from_spec(spec)
";
    let resolution = resolve(&[
        ("scripts/test_gc4.py", shell),
        ("scripts/check_gc4.sh", "echo ok\n"),
    ]);
    assert_eq!(
        import_count(&resolution, "scripts/test_gc4.py", "scripts/check_gc4.sh"),
        0
    );
}

/// A function-local of the constant's name is not the constant.
#[test]
fn a_local_shadowing_the_constant_abstains() {
    let source = "\
import importlib.util
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
BUILDER = ROOT / \"scripts/build_paper.py\"

def load(BUILDER):
    spec = importlib.util.spec_from_file_location(\"build_paper\", BUILDER)
    mod = importlib.util.module_from_spec(spec)
    mod.pure_guard()
";
    let resolution = resolve(&[
        ("scripts/test_paper_build.py", source),
        ("scripts/build_paper.py", BUILD_PAPER),
    ]);
    assert!(callers_of(&resolution, "scripts/build_paper.py::pure_guard").is_empty());
}

/// `BINN/scripts/test_wave21_analyser.py:22-32`, in shape.
const WAVE21_TEST: &str = "\
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


W21 = load(\"analyse_wave21\", \"scripts/aws/analyse_wave21.py\")
W20 = load(\"analyse_wave20\", relative=\"scripts/aws/analyse_wave20.py\")


def test_verdict():
    assert W21.verdict() == W20.verdict_20()


def test_local():
    w = load(\"analyse_wave21\", \"scripts/aws/analyse_wave21.py\")
    w.local_use()
";

const WAVE21: &str = "\
def verdict():
    return 1

def local_use():
    return 2
";

#[test]
fn a_parameterised_loader_binds_each_call_site_to_its_literal() {
    let resolution = resolve(&[
        ("scripts/test_wave21_analyser.py", WAVE21_TEST),
        ("scripts/aws/analyse_wave21.py", WAVE21),
        (
            "scripts/aws/analyse_wave20.py",
            "def verdict_20():\n    return 1\n",
        ),
    ]);
    for target in [
        "scripts/aws/analyse_wave21.py",
        "scripts/aws/analyse_wave20.py",
    ] {
        assert_eq!(
            import_count(&resolution, "scripts/test_wave21_analyser.py", target),
            1,
            "{target}"
        );
    }
    for (caller, callee) in [
        ("test_verdict", "scripts/aws/analyse_wave21.py::verdict"),
        ("test_verdict", "scripts/aws/analyse_wave20.py::verdict_20"),
        ("test_local", "scripts/aws/analyse_wave21.py::local_use"),
    ] {
        let caller = format!("scripts/test_wave21_analyser.py::{caller}");
        assert!(
            has_call(&resolution, &caller, callee),
            "{caller} -> {callee}; callers: {:?}",
            callers_of(&resolution, callee)
        );
    }
}

/// `BINN/scripts/did_confidence_intervals.py:86-100`: `sys.modules[name] =`
/// between the steps, type annotations on the parameters, a private name.
#[test]
fn the_did_confidence_intervals_loader_shape_binds() {
    let source = "\
import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _module(name: str, relpath: str):
    \"\"\"Import a frozen analyser by path under a distinct name.\"\"\"
    spec = importlib.util.spec_from_file_location(name, ROOT / relpath)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


MC = _module(\"mechanism_coverage\", \"scripts/mechanism_coverage.py\")


def report():
    return MC.coverage()
";
    let resolution = resolve(&[
        ("scripts/did_confidence_intervals.py", source),
        (
            "scripts/mechanism_coverage.py",
            "def coverage():\n    return 1\n",
        ),
    ]);
    assert!(has_call(
        &resolution,
        "scripts/did_confidence_intervals.py::report",
        "scripts/mechanism_coverage.py::coverage"
    ));
}

/// Every way a parameterised call site stops being readable.
#[test]
fn a_parameterised_loader_abstains_where_it_cannot_read_the_path() {
    let cases: &[(&str, &str)] = &[
        (
            "non-literal argument",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             W = load(\"w\", PICK)\n",
        ),
        (
            "argument is not a .py file",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             W = load(\"w\", \"scripts/aws/analyse_wave21.json\")\n",
        ),
        (
            "parameter inside an f-string",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / f\"{relative}\")\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             W = load(\"w\", \"scripts/aws/analyse_wave21.py\")\n",
        ),
        (
            "parameter used twice in the path",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             W = load(\"w\", \"scripts/aws/analyse_wave21.py\")\n",
        ),
        (
            "parameter rebound in the body",
            "def load(name, relative):\n    relative = relative.replace(\"a\", \"b\")\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             W = load(\"w\", \"scripts/aws/analyse_wave21.py\")\n",
        ),
        (
            "loader rebound at module level",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             load = other_loader\n\
             W = load(\"w\", \"scripts/aws/analyse_wave21.py\")\n",
        ),
        (
            "loader rebound through global",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             def retarget():\n    global load\n    load = other_loader\n\
             W = load(\"w\", \"scripts/aws/analyse_wave21.py\")\n",
        ),
        (
            "loader shadowed at the call site",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             def use(load):\n    W = load(\"w\", \"scripts/aws/analyse_wave21.py\")\n    W.verdict()\n",
        ),
        (
            "splatted arguments",
            "def load(name, relative):\n    spec = importlib.util.spec_from_file_location(name, ROOT / relative)\n    mod = importlib.util.module_from_spec(spec)\n    return mod\n\
             W = load(*ARGS)\n",
        ),
    ];
    for (label, body) in cases {
        let source = format!(
            "import importlib.util\nfrom pathlib import Path\n\
             ROOT = Path(__file__).resolve().parents[1]\n{body}\
             def test_it():\n    W.verdict()\n"
        );
        let resolution = resolve(&[
            ("scripts/test_wave21_analyser.py", &source),
            ("scripts/aws/analyse_wave21.py", WAVE21),
            ("scripts/aws/analyse_wave21.json", "{}"),
        ]);
        assert_eq!(
            import_count(
                &resolution,
                "scripts/test_wave21_analyser.py",
                "scripts/aws/analyse_wave21.py"
            ),
            0,
            "{label}: an Imports edge was made"
        );
        assert!(
            callers_of(&resolution, "scripts/aws/analyse_wave21.py::verdict").is_empty(),
            "{label}: callers {:?}",
            callers_of(&resolution, "scripts/aws/analyse_wave21.py::verdict")
        );
    }
}

/// A `load` that is not a path loader — the data loader other BINN files
/// define — binds nothing, even with a `.py`-looking argument; and a path
/// loader in one file says nothing about `load` in another.
#[test]
fn a_same_named_non_loader_binds_nothing() {
    let data_loader = "\
import json

def load(name, relative=\"x\"):
    return json.load(open(name))

W = load(\"armB2-nokernel\", \"scripts/aws/analyse_wave21.py\")

def test_it():
    W.verdict()
";
    let resolution = resolve(&[
        ("scripts/test_data.py", data_loader),
        ("scripts/test_wave21_analyser.py", WAVE21_TEST),
        ("scripts/aws/analyse_wave21.py", WAVE21),
        (
            "scripts/aws/analyse_wave20.py",
            "def verdict_20():\n    return 1\n",
        ),
    ]);
    assert_eq!(
        import_count(
            &resolution,
            "scripts/test_data.py",
            "scripts/aws/analyse_wave21.py"
        ),
        0
    );
    assert!(
        !callers_of(&resolution, "scripts/aws/analyse_wave21.py::verdict")
            .iter()
            .any(|caller| caller.starts_with("scripts/test_data.py")),
        "callers {:?}",
        callers_of(&resolution, "scripts/aws/analyse_wave21.py::verdict")
    );
}

/// `load("a", "x.py").fn()` through a parameterised loader names a different
/// file at each call, so the loader's own name is never a module handle.
#[test]
fn a_parameterised_loaders_name_is_not_a_handle() {
    let source = "\
import importlib.util
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    mod = importlib.util.module_from_spec(spec)
    return mod

def test_direct():
    load(\"a\", \"scripts/aws/analyse_wave21.py\").verdict()
    load(\"b\", \"scripts/aws/analyse_wave20.py\").verdict_20()
";
    let resolution = resolve(&[
        ("scripts/test_direct.py", source),
        ("scripts/aws/analyse_wave21.py", WAVE21),
        (
            "scripts/aws/analyse_wave20.py",
            "def verdict_20():\n    return 1\n",
        ),
    ]);
    // Both files are dependencies of the test.
    assert_eq!(
        import_count(
            &resolution,
            "scripts/test_direct.py",
            "scripts/aws/analyse_wave21.py"
        ),
        1
    );
    assert_eq!(
        import_count(
            &resolution,
            "scripts/test_direct.py",
            "scripts/aws/analyse_wave20.py"
        ),
        1
    );
    // But neither call is attributed through the loader's name.
    assert!(callers_of(&resolution, "scripts/aws/analyse_wave21.py::verdict").is_empty());
    assert!(callers_of(&resolution, "scripts/aws/analyse_wave20.py::verdict_20").is_empty());
}

/// A parameter that is a directory rather than the path's tail is not
/// substituted. With it opaque, the literal tail `mod.py` names two indexed
/// files, which is "one of several" and abstains; substituting `scripts/aws`
/// would have picked one of them on the strength of a rule this pass does not
/// claim.
#[test]
fn a_parameter_that_is_not_the_path_tail_is_not_substituted() {
    let source = "\
import importlib.util
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def load(name, folder):
    spec = importlib.util.spec_from_file_location(name, ROOT / folder / \"mod.py\")
    mod = importlib.util.module_from_spec(spec)
    return mod

W = load(\"w\", \"scripts/aws\")

def test_it():
    W.verdict()
";
    let resolution = resolve(&[
        ("scripts/test_folder.py", source),
        ("scripts/aws/mod.py", WAVE21),
        ("other/mod.py", WAVE21),
    ]);
    for target in ["scripts/aws/mod.py", "other/mod.py"] {
        assert_eq!(
            import_count(&resolution, "scripts/test_folder.py", target),
            0,
            "{target}"
        );
        let callee = format!("{target}::verdict");
        assert!(callers_of(&resolution, &callee).is_empty(), "{callee}");
    }
}
