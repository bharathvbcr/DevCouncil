//! Python imports that resolve only because the file put a directory on
//! `sys.path` first.
//!
//! ```python
//! ROOT = Path(__file__).resolve().parent.parent
//! sys.path.insert(0, str(ROOT / "scripts" / "aws"))
//! import analyse_wave20 as a20
//! ```
//!
//! is the dominant import shape left in BINN's scripts (51 files). The module
//! lives in `scripts/aws/`, which no ordinary rule searches from `scripts/`,
//! so the import bound nothing and every `a20.spearman(...)` landed in the
//! unresolved ledger: `impact analyse_wave20.py::spearman` missed its tests.
//!
//! The inserted directory is honoured only where the file itself says so, and
//! only when the source pins it down: anchored on `__file__`, inserted at
//! module level before the import, never undone, and naming exactly one file.
//! An import that resolves ordinarily keeps its ordinary answer.

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, EdgeKind, Extraction};
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

fn imports(resolution: &ResolutionResult, from: &str, to: &str) -> bool {
    resolution.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Imports && edge.source_file == from && edge.target_file == to
    })
}

fn callers_of(resolution: &ResolutionResult, callee: &str) -> Vec<String> {
    resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol == callee)
        .map(|edge| edge.source_symbol.clone())
        .collect()
}

const WAVE20: &str = "def spearman(xs, ys):\n    return 0.0\n\ndef index(roots):\n    return {}\n";

/// `BINN/scripts/test_wave20_analyser.py:25-37, 80`, in shape.
const WAVE20_TEST: &str = "\
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
import analyse_wave20 as a20  # noqa: E402


class SpearmanTest(unittest.TestCase):
    def test_ties(self):
        self.assertEqual(a20.spearman([1, 2], [1, 2]), 0.0)
";

#[test]
fn the_binn_sys_path_shape_links_import_and_callers() {
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", WAVE20_TEST),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ]);
    assert!(imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/aws/analyse_wave20.py"
    ));
    let callers = callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman");
    assert!(
        callers
            .iter()
            .any(|c| c == "scripts/test_wave20_analyser.py::SpearmanTest.test_ties"),
        "callers: {callers:?}"
    );
}

/// The other spellings of the insert, and of the import, that BINN uses.
#[test]
fn every_supported_insert_and_import_form_links() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "from-import",
            "ROOT = Path(__file__).resolve().parent.parent\nsys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\n",
            "from analyse_wave20 import spearman\n",
            "spearman([], [])",
        ),
        (
            "append with os.path.join",
            "HERE = os.path.dirname(os.path.abspath(__file__))\nsys.path.append(os.path.join(HERE, \"aws\"))\n",
            "import analyse_wave20 as a20\n",
            "a20.spearman([], [])",
        ),
        (
            "slice assignment",
            "ROOT = Path(__file__).resolve().parents[1]\nsys.path[:0] = [str(ROOT / \"scripts/aws\")]\n",
            "import analyse_wave20 as a20\n",
            "a20.spearman([], [])",
        ),
        (
            "guarded by `not in sys.path`",
            "ROOT = Path(__file__).resolve().parent.parent\nif str(ROOT / \"scripts\" / \"aws\") not in sys.path:\n    sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\n",
            "import analyse_wave20 as a20\n",
            "a20.spearman([], [])",
        ),
        (
            "inside a module-level try",
            "try:\n    sys.path.insert(0, os.fspath(Path(__file__).resolve().parent / \"aws\"))\nexcept Exception:\n    pass\n",
            "import analyse_wave20 as a20\n",
            "a20.spearman([], [])",
        ),
        (
            "`..` from the file's directory",
            "sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), \"..\", \"scripts\", \"aws\"))\n",
            "import analyse_wave20 as a20\n",
            "a20.spearman([], [])",
        ),
    ];
    for (label, insert, import, call) in cases {
        let source = format!(
            "import os\nimport sys\nfrom pathlib import Path\n{insert}{import}\ndef test_it():\n    {call}\n"
        );
        let resolution = resolve(&[
            ("scripts/test_wave20_analyser.py", &source),
            ("scripts/aws/analyse_wave20.py", WAVE20),
        ]);
        assert!(
            imports(
                &resolution,
                "scripts/test_wave20_analyser.py",
                "scripts/aws/analyse_wave20.py"
            ),
            "{label}: no Imports edge"
        );
        assert!(
            callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman")
                .iter()
                .any(|c| c == "scripts/test_wave20_analyser.py::test_it"),
            "{label}: callers {:?}",
            callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman")
        );
    }
}

/// A package directory on the inserted path.
#[test]
fn a_package_on_the_inserted_path_links() {
    let source = "\
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent / \"vendor\"))
import toolkit

def test_it():
    toolkit.run()
";
    let resolution = resolve(&[
        ("scripts/test_toolkit.py", source),
        (
            "scripts/vendor/toolkit/__init__.py",
            "def run():\n    pass\n",
        ),
    ]);
    assert!(imports(
        &resolution,
        "scripts/test_toolkit.py",
        "scripts/vendor/toolkit/__init__.py"
    ));
    assert!(!callers_of(&resolution, "scripts/vendor/toolkit/__init__.py::run").is_empty());
}

/// Every way the inserted directory stops being evidence.
#[test]
fn the_inserted_directory_abstains_where_the_source_does_not_pin_it() {
    let cases: &[(&str, &str)] = &[
        (
            "relative to the working directory",
            "sys.path.insert(0, \"scripts/aws\")\nimport analyse_wave20 as a20\n",
        ),
        (
            "non-literal directory",
            "sys.path.insert(0, str(pick_directory()))\nimport analyse_wave20 as a20\n",
        ),
        (
            "import precedes the insert",
            "import analyse_wave20 as a20\nsys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\n",
        ),
        (
            "sys.path reassigned",
            "sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\nsys.path = [\"/opt/lib\"]\nimport analyse_wave20 as a20\n",
        ),
        (
            "sys.path.remove",
            "sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\nsys.path.remove(str(ROOT / \"scripts\" / \"aws\"))\nimport analyse_wave20 as a20\n",
        ),
        (
            "insert inside a function",
            "def setup():\n    sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\nimport analyse_wave20 as a20\n",
        ),
        (
            "an unreadable insert ahead of a readable one",
            "sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\nsys.path.insert(0, OTHER)\nimport analyse_wave20 as a20\n",
        ),
    ];
    for (label, body) in cases {
        let source = format!(
            "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n{body}\
             def test_it():\n    a20.spearman([], [])\n"
        );
        let resolution = resolve(&[
            ("scripts/test_wave20_analyser.py", &source),
            ("scripts/aws/analyse_wave20.py", WAVE20),
        ]);
        assert!(
            !imports(
                &resolution,
                "scripts/test_wave20_analyser.py",
                "scripts/aws/analyse_wave20.py"
            ),
            "{label}: an Imports edge was made"
        );
        assert!(
            callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman").is_empty(),
            "{label}: callers {:?}",
            callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman")
        );
    }
}

/// Two inserted directories that both hold the module: one of several.
#[test]
fn two_inserted_directories_holding_the_module_abstain() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
sys.path.insert(0, str(ROOT / \"scripts\" / \"azure\"))
import analyse_wave20 as a20

def test_it():
    a20.spearman([], [])
";
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", source),
        ("scripts/aws/analyse_wave20.py", WAVE20),
        ("scripts/azure/analyse_wave20.py", WAVE20),
    ]);
    for target in [
        "scripts/aws/analyse_wave20.py",
        "scripts/azure/analyse_wave20.py",
    ] {
        assert!(
            !imports(&resolution, "scripts/test_wave20_analyser.py", target),
            "{target}"
        );
        assert!(
            callers_of(&resolution, &format!("{target}::spearman")).is_empty(),
            "{target}"
        );
    }
}

/// An import that resolves ordinarily keeps its ordinary answer.
#[test]
fn an_ordinarily_resolved_import_is_not_overridden() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
import analyse_wave20 as a20

def test_it():
    a20.spearman([], [])
";
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", source),
        // Beside the importer: the ordinary rule finds this one.
        ("scripts/analyse_wave20.py", WAVE20),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ]);
    assert!(imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/analyse_wave20.py"
    ));
    assert!(!imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/aws/analyse_wave20.py"
    ));
    assert!(callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman").is_empty());
}

/// `BINN/scripts/test_campaign_tooling.py`, in shape: `scripts/aws/` and
/// `scripts/` inserted at module level, `scripts/azure/` inserted inside one
/// test, and `plan_cells` / `collect` present in both `aws/` and `azure/`.
///
/// Whether that test has run before any given import is call order, which
/// the source does not fix — so `plan_cells` and `collect` may be either
/// file, and are not linked. A module only `aws/` holds is unaffected, and so
/// is an in-function insert of `aws/` itself, which names the same file.
const CAMPAIGN_TOOLING: &str = "\
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
sys.path.insert(0, str(ROOT / \"scripts\"))

import plan_cells  # noqa: E402
from plan_cells import cell, estimated_seconds  # noqa: E402


class Planner(unittest.TestCase):
    def test_block(self):
        sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
        import claim_next
        claim_next.claim()
        cell()

    def test_all_three_analysers_share_one_owner(self):
        import analyse_campaign
        sys.path.insert(0, str(ROOT / \"scripts\" / \"azure\"))
        import analyse as azure_analyse
        azure_analyse.run()
        analyse_campaign.run()


class Collect(unittest.TestCase):
    def test_collect(self):
        import collect
        collect.gather()
";

fn campaign_corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        ("scripts/test_campaign_tooling.py", CAMPAIGN_TOOLING),
        (
            "scripts/aws/plan_cells.py",
            "def cell():\n    pass\n\ndef estimated_seconds(c):\n    return 0\n",
        ),
        (
            "scripts/azure/plan_cells.py",
            "def cell():\n    pass\n\ndef estimated_seconds(c):\n    return 0\n",
        ),
        ("scripts/aws/collect.py", "def gather():\n    pass\n"),
        ("scripts/azure/collect.py", "def gather():\n    pass\n"),
        ("scripts/aws/claim_next.py", "def claim():\n    pass\n"),
        ("scripts/aws/analyse_campaign.py", "def run():\n    pass\n"),
        ("scripts/azure/analyse.py", "def run():\n    pass\n"),
    ]
}

/// Callers above the speculative floor. A bare `cell()` whose import bound
/// nothing still meets the resolver's general ambiguous-global rung — two
/// `cell`s, one speculative edge each, excluded from default walks — which
/// is not a `sys.path` link and not what the veto governs.
fn confident_callers_of(resolution: &ResolutionResult, callee: &str) -> Vec<String> {
    resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.target_symbol == callee
                && edge.confidence > Confidence::SPECULATIVE
        })
        .map(|edge| edge.source_symbol.clone())
        .collect()
}

#[test]
fn an_in_function_insert_vetoes_a_module_it_could_supply_instead() {
    let resolution = resolve(&campaign_corpus());
    let from = "scripts/test_campaign_tooling.py";
    for (module, function) in [("plan_cells", "cell"), ("collect", "gather")] {
        for dir in ["aws", "azure"] {
            let target = format!("scripts/{dir}/{module}.py");
            assert!(
                !imports(&resolution, from, &target),
                "{target}: linked although the in-function azure insert may shadow it"
            );
            assert!(
                confident_callers_of(&resolution, &format!("{target}::{function}")).is_empty(),
                "{target}: callers {:?}",
                confident_callers_of(&resolution, &format!("{target}::{function}"))
            );
        }
    }
    // The vetoing directory does not hold these: they still link.
    for target in [
        "scripts/aws/claim_next.py",
        "scripts/aws/analyse_campaign.py",
    ] {
        assert!(imports(&resolution, from, target), "{target}: not linked");
    }
    assert!(
        callers_of(&resolution, "scripts/aws/claim_next.py::claim")
            .iter()
            .any(|c| c == "scripts/test_campaign_tooling.py::Planner.test_block"),
        "{:?}",
        callers_of(&resolution, "scripts/aws/claim_next.py::claim")
    );
    // Only an in-function insert reaches `analyse`: never a link.
    assert!(!imports(&resolution, from, "scripts/azure/analyse.py"));
}

/// An in-function insert of the directory the module-level entries already
/// name vetoes nothing: both name the same file.
#[test]
fn an_in_function_insert_naming_the_same_file_does_not_veto() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
import analyse_wave20 as a20

def setup():
    sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))

def test_it():
    a20.spearman([], [])
";
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", source),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ]);
    assert!(imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/aws/analyse_wave20.py"
    ));
    assert!(!callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman").is_empty());
}

/// An insert the source does not pin — unreadable inside a function, in a
/// class body, or above the repository root — could supply any module: every
/// `sys.path`-derived link in the file abstains. An import the ordinary rules
/// answer never depended on `sys.path` and keeps its edge.
#[test]
fn an_unpinned_insert_vetoes_every_sys_path_link_but_not_ordinary_imports() {
    for (label, veto) in [
        (
            "unreadable in a function",
            "def later(where):\n    sys.path.insert(0, where)\n",
        ),
        (
            "in a class body",
            "class Holder:\n    sys.path.insert(0, str(ROOT / \"vendor\"))\n",
        ),
        (
            "above the repository root",
            "sys.path.insert(0, str(Path(__file__).resolve().parents[4] / \"lib\"))\n",
        ),
    ] {
        let source = format!(
            "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n\
             sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\n{veto}\
             import analyse_wave20 as a20\nimport helper\n\n\
             def test_it():\n    a20.spearman([], [])\n    helper.assist()\n"
        );
        let resolution = resolve(&[
            ("scripts/test_wave20_analyser.py", &source),
            ("scripts/aws/analyse_wave20.py", WAVE20),
            ("scripts/helper.py", "def assist():\n    pass\n"),
        ]);
        assert!(
            !imports(
                &resolution,
                "scripts/test_wave20_analyser.py",
                "scripts/aws/analyse_wave20.py"
            ),
            "{label}: the sys.path link was made"
        );
        assert!(
            callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman").is_empty(),
            "{label}"
        );
        assert!(
            imports(
                &resolution,
                "scripts/test_wave20_analyser.py",
                "scripts/helper.py"
            ),
            "{label}: the ordinary import lost its edge"
        );
    }
}

/// An insert in one file says nothing about another file's imports.
#[test]
fn an_inserted_directory_does_not_leak_into_other_files() {
    // The docstring puts the import past every offset of the other file's
    // insert, so only the per-file boundary — not the before-the-insert rule —
    // can keep the directory out.
    let other = "\
\"\"\"A test module whose import sits well past the byte offset at which the
other file inserted its directory: an offset comparison alone would let a
leaked entry through, so this checks the per-file boundary on its own.
Padding padding padding padding padding padding padding padding padding.
Padding padding padding padding padding padding padding padding padding.
Padding padding padding padding padding padding padding padding padding.
\"\"\"
import analyse_wave20 as a20

def test_other():
    a20.spearman([], [])
";
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", WAVE20_TEST),
        ("scripts/test_other.py", other),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ]);
    assert!(imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/aws/analyse_wave20.py"
    ));
    assert!(!imports(
        &resolution,
        "scripts/test_other.py",
        "scripts/aws/analyse_wave20.py"
    ));
    assert!(
        !callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman")
            .iter()
            .any(|c| c.starts_with("scripts/test_other.py")),
        "{:?}",
        callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman")
    );
}

/// Thousands of imports behind the maximum number of inserted directories:
/// resolution stays linear in the imports and links every one.
#[test]
fn thousands_of_imports_behind_many_inserts_resolve_in_linear_time() {
    let corpus = |imports: usize| -> Vec<(String, String)> {
        let mut source = String::from(
            "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n",
        );
        for d in 0..64 {
            source.push_str(&format!("sys.path.insert(0, str(ROOT / \"lib{d}\"))\n"));
        }
        for i in 0..imports {
            source.push_str(&format!("import m{i}\n"));
        }
        let mut files = vec![("scripts/many.py".to_string(), source)];
        for i in 0..imports {
            files.push((
                format!("lib{}/m{i}.py", i % 64),
                "def f():\n    pass\n".to_string(),
            ));
        }
        files
    };
    let timed = |files: &[(String, String)]| {
        let extractions: Vec<Extraction> = files
            .iter()
            .map(|(path, source)| extract_file(path, source))
            .collect();
        let mut best = std::time::Duration::MAX;
        let mut linked = 0;
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let mut resolver = Resolver::new();
            resolver.index_extractions(&extractions);
            let resolution = resolver.resolve_all(&extractions).unwrap();
            best = best.min(start.elapsed());
            linked = resolution
                .edges
                .iter()
                .filter(|edge| {
                    edge.edge_kind == EdgeKind::Imports && edge.source_file == "scripts/many.py"
                })
                .count();
        }
        (best, linked)
    };
    let (small, small_linked) = timed(&corpus(300));
    let (large, large_linked) = timed(&corpus(3_000));
    assert_eq!(small_linked, 300);
    assert_eq!(large_linked, 3_000);
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < 40.0,
        "10x the imports took {ratio:.1}x the time ({small:?} -> {large:?})"
    );
}
