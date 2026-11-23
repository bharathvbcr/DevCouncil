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

/// S2. An import the ordinary rule resolves is *not* kept when a directory this
/// file put on `sys.path` first holds a different file for the module: which
/// one runs is the inserted directory's, and binding the sibling is a
/// confident edge to what may be the wrong module.
#[test]
fn a_sys_path_directory_holding_a_different_file_makes_the_import_abstain() {
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
    for target in ["scripts/analyse_wave20.py", "scripts/aws/analyse_wave20.py"] {
        assert!(
            !imports(&resolution, "scripts/test_wave20_analyser.py", target),
            "{target}: an Imports edge was made"
        );
        assert!(
            callers_of(&resolution, &format!("{target}::spearman")).is_empty(),
            "{target}: callers {:?}",
            callers_of(&resolution, &format!("{target}::spearman"))
        );
    }
}

/// The S2 corpus with an unreadable module-level insert after the readable
/// one: the readable `aws` entry still shadows the sibling, so the import must
/// not bind either file. An unreadable insert must not withdraw the evidence.
#[test]
fn an_unreadable_insert_does_not_unshadow_the_sibling() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))
sys.path.insert(0, somewhere)
import analyse_wave20 as a20

def test_it():
    a20.spearman([], [])
";
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", source),
        ("scripts/analyse_wave20.py", WAVE20),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ]);
    for target in ["scripts/analyse_wave20.py", "scripts/aws/analyse_wave20.py"] {
        assert!(
            !imports(&resolution, "scripts/test_wave20_analyser.py", target),
            "{target}: an Imports edge was made"
        );
        assert!(
            callers_of(&resolution, &format!("{target}::spearman")).is_empty(),
            "{target}: callers {:?}",
            callers_of(&resolution, &format!("{target}::spearman"))
        );
    }
}

/// Insert order around the import. A readable entry ahead of the import
/// shadows the sibling even when an unreadable insert follows it; with only an
/// unreadable insert ahead and the readable one after, no known entry precedes
/// the import, and an unknown directory never vetoes an ordinary import.
#[test]
fn an_unreadable_insert_orders_against_the_import_like_any_entry() {
    let prelude =
        "import sys\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n";
    let aws = "sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\n";
    let import = "import analyse_wave20 as a20\n";
    let usage = "\ndef test_it():\n    a20.spearman([], [])\n";
    let files = |source: &str| {
        resolve(&[
            ("scripts/test_wave20_analyser.py", source),
            ("scripts/analyse_wave20.py", WAVE20),
            ("scripts/aws/analyse_wave20.py", WAVE20),
        ])
    };

    let before = format!("{prelude}{aws}{import}sys.path.insert(0, somewhere)\n{usage}");
    let resolution = files(&before);
    for target in ["scripts/analyse_wave20.py", "scripts/aws/analyse_wave20.py"] {
        assert!(
            !imports(&resolution, "scripts/test_wave20_analyser.py", target),
            "readable, import, unreadable: {target} was bound"
        );
    }

    let after = format!("{prelude}sys.path.insert(0, somewhere)\n{import}{aws}{usage}");
    let resolution = files(&after);
    assert!(
        imports(
            &resolution,
            "scripts/test_wave20_analyser.py",
            "scripts/analyse_wave20.py"
        ),
        "unreadable, import, readable: the sibling lost its edge"
    );
}

/// An inserted directory that holds the *same* file ordinary resolution found
/// shadows nothing, so the ordinary answer stands.
#[test]
fn a_sys_path_directory_naming_the_same_file_keeps_the_ordinary_answer() {
    let source = "\
import sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))
import analyse_wave20 as a20

def test_it():
    a20.spearman([], [])
";
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", source),
        ("scripts/analyse_wave20.py", WAVE20),
    ]);
    assert!(imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/analyse_wave20.py"
    ));
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
            "unreadable at module level",
            "sys.path.insert(0, somewhere)\n",
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

// ---------------------------------------------------------------------------
// Attack lane A (ft-87363ce5): the file-wide veto must block only
// `sys.path`-derived links, whatever the insert order and shape.
// ---------------------------------------------------------------------------

const ATK_PRELUDE: &str =
    "import os\nimport sys\nimport json\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\n";
const ATK_AWS: &str = "sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\n";
const ATK_UNREADABLE: &str = "sys.path.insert(0, somewhere)\n";
const ATK_IMPORT: &str = "import analyse_wave20 as a20\n";
const ATK_USE: &str = "\ndef test_it():\n    a20.spearman([], [])\n";

fn atk_resolve_pair(source: &str) -> ResolutionResult {
    resolve(&[
        ("scripts/test_wave20_analyser.py", source),
        ("scripts/analyse_wave20.py", WAVE20),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ])
}

/// `(sibling bound, aws bound)` for the importer.
fn atk_bound(resolution: &ResolutionResult) -> (bool, bool) {
    (
        imports(
            resolution,
            "scripts/test_wave20_analyser.py",
            "scripts/analyse_wave20.py",
        ),
        imports(
            resolution,
            "scripts/test_wave20_analyser.py",
            "scripts/aws/analyse_wave20.py",
        ),
    )
}

/// Ordinary package imports, relative imports, `from . import x`, stdlib and
/// package-root imports keep their edges in a file that carries both a readable
/// entry and an unreadable module-level insert.
#[test]
fn atk_the_veto_blocks_only_sys_path_links_not_ordinary_ones() {
    let main = format!(
        "{ATK_PRELUDE}{ATK_AWS}{ATK_UNREADABLE}\
from . import util\n\
from .other import thing\n\
import pkg.other as po\n\
from pkg.util import helper\n\
import sibling\n\n\
def run():\n    util.helper()\n    thing()\n    po.thing()\n    helper()\n    sibling.sib()\n"
    );
    let resolution = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", "def helper():\n    return 1\n"),
        ("pkg/other.py", "def thing():\n    return 2\n"),
        ("pkg/sibling.py", "def sib():\n    return 3\n"),
        ("pkg/main.py", &main),
    ]);
    for target in ["pkg/util.py", "pkg/other.py"] {
        assert!(imports(&resolution, "pkg/main.py", target), "{target} lost");
    }
    for (callee, label) in [
        ("pkg/util.py::helper", "util.helper"),
        ("pkg/other.py::thing", "thing"),
    ] {
        assert!(
            callers_of(&resolution, callee)
                .iter()
                .any(|caller| caller == "pkg/main.py::run"),
            "{label}: callers {:?}",
            callers_of(&resolution, callee)
        );
    }
    assert!(
        callers_of(&resolution, "pkg/sibling.py::sib")
            .iter()
            .any(|caller| caller == "pkg/main.py::run"),
        "`import sibling` lost its caller edge"
    );
}

/// Same file shape with only an unreadable insert (no readable entry): the
/// resolver stores no search path at all and every ordinary import binds.
#[test]
fn atk_an_unreadable_insert_alone_changes_nothing_for_ordinary_imports() {
    let source = format!("{ATK_PRELUDE}{ATK_UNREADABLE}{ATK_IMPORT}{ATK_USE}");
    let resolution = atk_resolve_pair(&source);
    assert_eq!(atk_bound(&resolution), (true, false));
}

/// Insert order against the import, across every block shape the extractor
/// counts as module level.
#[test]
fn atk_insert_order_matrix() {
    // (label, source body, sibling bound, aws bound)
    let cases: Vec<(&str, String, bool, bool)> = vec![
        ("aws, unreadable, import", format!("{ATK_AWS}{ATK_UNREADABLE}{ATK_IMPORT}"), false, false),
        ("unreadable, aws, import", format!("{ATK_UNREADABLE}{ATK_AWS}{ATK_IMPORT}"), false, false),
        (
            "unreadable, unreadable, aws, import",
            format!("{ATK_UNREADABLE}{ATK_UNREADABLE}{ATK_AWS}{ATK_IMPORT}"),
            false,
            false,
        ),
        ("import, aws, unreadable", format!("{ATK_IMPORT}{ATK_AWS}{ATK_UNREADABLE}"), true, false),
        ("import, unreadable", format!("{ATK_IMPORT}{ATK_UNREADABLE}"), true, false),
        (
            "unreadable, import, aws",
            format!("{ATK_UNREADABLE}{ATK_IMPORT}{ATK_AWS}"),
            true,
            false,
        ),
        (
            "aws in __main__ guard, unreadable in try, import",
            format!(
                "if __name__ == \"__main__\":\n    {ATK_AWS}try:\n    {ATK_UNREADABLE}except Exception:\n    pass\n{ATK_IMPORT}"
            ),
            false,
            false,
        ),
        (
            "aws in with, unreadable in for, import",
            format!("with ctx():\n    {ATK_AWS}for p in ps:\n    sys.path.insert(0, p)\n{ATK_IMPORT}"),
            false,
            false,
        ),
        (
            "aws, import, unreadable only inside a function",
            format!("{ATK_AWS}{ATK_IMPORT}def later(w):\n    sys.path.insert(0, w)\n"),
            false,
            false,
        ),
        (
            "function-scoped readable + module unreadable, no module entry",
            format!(
                "{ATK_UNREADABLE}{ATK_IMPORT}def later():\n    sys.path.insert(0, str(ROOT / \"scripts\" / \"aws\"))\n"
            ),
            true,
            false,
        ),
    ];
    for (label, body, sibling, aws) in cases {
        let source = format!("{ATK_PRELUDE}{body}{ATK_USE}");
        let resolution = atk_resolve_pair(&source);
        assert_eq!(atk_bound(&resolution), (sibling, aws), "{label}");
    }
}

/// An import that only `sys.path` can supply, with the readable entry ahead of
/// it and the unreadable insert AFTER it. At run time the import ran before the
/// unknown insert, so the link is true; the veto is file-wide and ignores
/// order. CHARACTERISATION: this equals v99 (nothing was linked then either),
/// so it is a known over-veto, not a regression.
#[test]
fn atk_characterise_a_late_unreadable_insert_still_vetoes_an_earlier_link() {
    let source = format!("{ATK_PRELUDE}{ATK_AWS}{ATK_IMPORT}{ATK_UNREADABLE}{ATK_USE}");
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", &source),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ]);
    assert!(!imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/aws/analyse_wave20.py"
    ));
}

/// A `..` escape above the repository root vetoes `sys.path` links but the
/// readable entry still makes the sibling abstain, and ordinary imports bind.
#[test]
fn atk_an_escape_above_the_root_vetoes_links_but_keeps_ordinary_imports() {
    let source = format!(
        "{ATK_PRELUDE}{ATK_AWS}sys.path.insert(0, str(ROOT / \"..\" / \"..\" / \"lib\"))\n{ATK_IMPORT}import helper\n{ATK_USE}"
    );
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", &source),
        ("scripts/analyse_wave20.py", WAVE20),
        ("scripts/aws/analyse_wave20.py", WAVE20),
        ("scripts/helper.py", "def assist():\n    pass\n"),
    ]);
    assert_eq!(atk_bound(&resolution), (false, false));
    assert!(imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/helper.py"
    ));
}

/// Pathological, mixed and over-cap files never panic and never link the
/// unreadable-insert path. Within the 64-row cap the readable entry shadows
/// the sibling; past the cap the whole search path is withdrawn, so the
/// sibling binds (the over-cap gap kept out of ft-87363ce5's scope).
#[test]
fn atk_cap_boundary_shadows_within_the_cap_and_withdraws_past_it() {
    for (readable, unreadable) in [
        (62usize, 1usize),
        (63, 1),
        (63, 2),
        (64, 1),
        (65, 1),
        (1, 200),
    ] {
        let mut inserts = String::from(ATK_AWS);
        for i in 0..readable.saturating_sub(1) {
            inserts.push_str(&format!("sys.path.insert(0, str(ROOT / \"d{i}\"))\n"));
        }
        for _ in 0..unreadable {
            inserts.push_str(ATK_UNREADABLE);
        }
        let source = format!("{ATK_PRELUDE}{inserts}{ATK_IMPORT}{ATK_USE}");
        let resolution = atk_resolve_pair(&source);
        let (sibling, aws) = atk_bound(&resolution);
        // Within the 64-row cap the aws entry stays and shadows the sibling.
        // Beyond it the file's whole search path is withdrawn (v99 behaviour),
        // so the sibling binds; the aws link is never made either way.
        let total = readable + usize::from(unreadable > 0);
        let expect_sibling = total > 64;
        assert!(!aws, "{readable}+{unreadable}: aws linked");
        assert_eq!(sibling, expect_sibling, "{readable}+{unreadable}");
    }
}

/// `from X import name` and an `X.Base` heritage read: a known out-of-scope
/// gap (index.rs `resolve_import_path` direct binding, pytypes.rs heritage
/// fallback). CHARACTERISATION of today's behaviour for a sibling that a
/// readable entry shadows, with and without an unreadable insert: the
/// symbol-level edges (Calls, Extends, References) still bind the sibling
/// although the `Imports` edge abstains. If a later change closes the gap,
/// flip these assertions.
#[test]
fn atk_characterise_from_import_and_heritage_against_a_shadowed_sibling() {
    let sibling = "class Base:\n    def run(self):\n        return 1\n\ndef spearman(xs, ys):\n    return 0.0\n";
    for (label, extra) in [
        ("readable only", ""),
        ("readable + unreadable", ATK_UNREADABLE),
    ] {
        let source = format!(
            "{ATK_PRELUDE}{ATK_AWS}{extra}from analyse_wave20 import spearman, Base\n\nclass Child(Base):\n    pass\n\ndef test_it():\n    spearman([], [])\n"
        );
        let resolution = resolve(&[
            ("scripts/test_wave20_analyser.py", &source),
            ("scripts/analyse_wave20.py", sibling),
            ("scripts/aws/analyse_wave20.py", sibling),
        ]);
        let sib_imports = imports(
            &resolution,
            "scripts/test_wave20_analyser.py",
            "scripts/analyse_wave20.py",
        );
        let sib_callers = callers_of(&resolution, "scripts/analyse_wave20.py::spearman");
        let extends = resolution.edges.iter().any(|e| {
            e.edge_kind == EdgeKind::Extends && e.target_file == "scripts/analyse_wave20.py"
        });
        let aws_edges = resolution
            .edges
            .iter()
            .filter(|e| {
                e.target_file == "scripts/aws/analyse_wave20.py"
                    && e.edge_kind != EdgeKind::Contains
            })
            .count();
        assert!(!sib_imports, "{label}: the Imports edge abstains today");
        assert!(
            !sib_callers.is_empty(),
            "{label}: the known gap closed? {sib_callers:?}"
        );
        assert!(extends, "{label}: the heritage gap closed?");
        assert_eq!(aws_edges, 0, "{label}: a vetoed/shadowing aws link leaked");
    }
}

/// The module only the inserted directory holds, read with `from X import`
/// in a file whose unreadable insert vetoes the link: no edge to it.
#[test]
fn atk_from_import_through_a_vetoed_search_path_binds_nothing() {
    let source = format!(
        "{ATK_PRELUDE}{ATK_AWS}{ATK_UNREADABLE}from analyse_wave20 import spearman\n\ndef test_it():\n    spearman([], [])\n"
    );
    let resolution = resolve(&[
        ("scripts/test_wave20_analyser.py", &source),
        ("scripts/aws/analyse_wave20.py", WAVE20),
    ]);
    assert!(!imports(
        &resolution,
        "scripts/test_wave20_analyser.py",
        "scripts/aws/analyse_wave20.py"
    ));
    assert!(
        callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman").is_empty(),
        "{:?}",
        callers_of(&resolution, "scripts/aws/analyse_wave20.py::spearman")
    );
}
