//! Liveness for Python module constants, in both directions.
//!
//! Every Python module-scope binding is a symbol now, judged by its edges as a
//! function is. The dangerous direction is reporting a *used* constant dead:
//! that is what happened to six path constants read only at module level
//! (`OUT = Path(__file__)…`, then `OUT / "summary.json"`), reported at 0.9
//! because a module-level assignment shadowed its own module-level reads. Each
//! read shape is asserted live here, alongside the finding a genuinely unused
//! constant still produces, so the fix cannot pass by exempting everything.

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
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

/// A finding for `symbol` that is not exempt: what an agent would act on.
fn finding<'a>(
    reports: &'a [DeadSymbolReport],
    file: &str,
    symbol: &str,
) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|report| {
        report.file_path == file && report.symbol_name == symbol && !report.is_exempt
    })
}

const LEDGER: &str = "\
from pathlib import Path

__all__ = [\"Ledger\", \"DECLARED_UNUSED\"]

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_LEDGER_PATH = REPO_ROOT / \"ledger\" / \"runs.jsonl\"
REQUIRED_CONTROLS: tuple[str, ...] = (\"shuffled_label\", \"privileged_hunk\")
UNUSED_CONSTANT = 7
DECLARED_UNUSED = 8
_PRIVATE_UNUSED = 9


class Ledger:
    def verdict(self):
        return list(REQUIRED_CONTROLS)
";

const TEST_LEDGER: &str = "\
from pkg.ledger import (  # noqa: E402
    DEFAULT_LEDGER_PATH,
    Ledger,
)


def test_ledger():
    assert DEFAULT_LEDGER_PATH
    Ledger().verdict()
";

const SCRIPT: &str = "\
import json
from pathlib import Path

OUT = Path(__file__).resolve().parent
rows = [json.loads(line) for line in (OUT / \"measurements.jsonl\").read_text().splitlines()]
(OUT / \"summary.json\").write_text(json.dumps(rows))
";

fn corpus() -> Vec<DeadSymbolReport> {
    reports(&[
        ("pkg/ledger.py", LEDGER),
        ("tests/test_ledger.py", TEST_LEDGER),
        ("scripts/summarize.py", SCRIPT),
    ])
}

/// The measured false positive: a constant read only at module level.
#[test]
fn a_constant_read_only_at_module_level_is_live() {
    let reports = corpus();
    assert!(
        finding(&reports, "scripts/summarize.py", "OUT").is_none(),
        "OUT is read twice at module level: {reports:?}"
    );
    assert!(
        finding(&reports, "pkg/ledger.py", "REPO_ROOT").is_none(),
        "REPO_ROOT is read by DEFAULT_LEDGER_PATH's initializer: {reports:?}"
    );
}

/// The reported constant, read in its own module, and one read only through a
/// commented import.
#[test]
fn a_constant_read_in_its_module_or_through_an_import_is_live() {
    let reports = corpus();
    for constant in ["REQUIRED_CONTROLS", "DEFAULT_LEDGER_PATH"] {
        assert!(
            finding(&reports, "pkg/ledger.py", constant).is_none(),
            "{constant} has a reader: {reports:?}"
        );
    }
}

/// A type alias written as an assignment and read only in annotations — the
/// measured case was `Purpose = Literal["heldout", "agreement"]`, read by a
/// dataclass field and a parameter and reported dead at 0.4.
#[test]
fn a_type_alias_read_only_in_annotations_is_live() {
    let reports = reports(&[(
        "pkg/session.py",
        "from typing import Literal\n\n\
         Purpose = Literal[\"heldout\", \"agreement\"]\n\
         Base = declarative_base()\n\n\n\
         class Record(Base):\n    purpose: Purpose\n\n\n\
         def open_session(purpose: Purpose = \"heldout\") -> Record:\n    return Record()\n",
    )]);
    for alias in ["Purpose", "Base"] {
        assert!(
            finding(&reports, "pkg/session.py", alias).is_none(),
            "{alias} is read in type position: {reports:?}"
        );
    }
}

/// The other direction: an unread public constant is a finding, at the tier a
/// never-called function reaches, and `__all__` or a leading underscore keep
/// one out exactly as they do for a function.
#[test]
fn an_unread_constant_is_still_reported() {
    let reports = corpus();
    let unused = finding(&reports, "pkg/ledger.py", "UNUSED_CONSTANT")
        .unwrap_or_else(|| panic!("nothing reads UNUSED_CONSTANT: {reports:?}"));
    assert!(
        unused.confidence >= 0.9,
        "no edge names it anywhere in a fully parsed corpus: {unused:?}"
    );
    assert!(
        finding(&reports, "pkg/ledger.py", "DECLARED_UNUSED").is_none(),
        "`__all__` publishes it, so it is exported surface: {reports:?}"
    );
    assert!(
        finding(&reports, "pkg/ledger.py", "_PRIVATE_UNUSED").is_none(),
        "underscore names are never dead-code candidates: {reports:?}"
    );
}
