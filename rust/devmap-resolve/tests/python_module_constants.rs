//! A Python module constant is reached by every read of it.
//!
//! The extractor used to keep a module-level binding only when `__all__` named
//! it, so `REQUIRED_CONTROLS` — imported by name from a test, omitted from its
//! module's `__all__` — had no node, and every read of it had no target. Now
//! that every module-scope binding is a symbol, liveness judges a constant by
//! its edges exactly as it judges a function, which is only sound if each way a
//! constant is read produces one. A read the resolver drops turns a used
//! constant into a confident dead finding, which is how six path constants
//! read only at module level (`OUT = Path(__file__)…`, then `OUT / "x"`) were
//! reported dead at 0.9 when an earlier rule let them through.
//!
//! Each read shape is asserted on its own, so a regression names the shape.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
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

/// Every non-structural, resolved edge that reaches `target`: what liveness
/// counts as a use.
fn readers_of(resolution: &ResolutionResult, target: &str) -> Vec<String> {
    let mut readers: Vec<String> = resolution
        .edges
        .iter()
        .filter(|edge| edge.target_symbol == target)
        .filter(|edge| {
            !matches!(
                edge.edge_kind,
                EdgeKind::Contains | EdgeKind::Defines | EdgeKind::MemberOf
            )
        })
        .filter(|edge| {
            !matches!(
                edge.resolution.as_deref(),
                Some(
                    Resolution::Unresolved { .. }
                        | Resolution::AmbiguousGlobal { .. }
                        | Resolution::LanguageServerDispatch { .. }
                )
            )
        })
        .map(|edge| format!("{:?}<-{}", edge.edge_kind, edge.source_symbol))
        .collect();
    readers.sort();
    readers.dedup();
    readers
}

const LEDGER: &str = "\
from pathlib import Path

__all__ = [\"Ledger\"]

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_LEDGER_PATH = REPO_ROOT / \"ledger\" / \"runs.jsonl\"
REQUIRED_CONTROLS: tuple[str, ...] = (
    \"shuffled_label\",
    \"privileged_hunk\",
    \"transfer_gate\",
)


class Ledger:
    def verdict(self):
        return [control for control in REQUIRED_CONTROLS]
";

const TEST_LEDGER: &str = "\
from pkg.ledger import (  # noqa: E402
    DEFAULT_LEDGER_PATH,  # the shared path
    REQUIRED_CONTROLS,
)


def test_controls():
    assert DEFAULT_LEDGER_PATH
    for control in REQUIRED_CONTROLS:
        assert control
";

const SCRIPT: &str = "\
from pathlib import Path

OUT = Path(__file__).resolve().parent
rows = (OUT / \"m.jsonl\").read_text()
print(OUT)
";

const ATTRIBUTE_READER: &str = "\
import pkg.ledger as ledger


def main():
    return ledger.REQUIRED_CONTROLS
";

fn corpus() -> ResolutionResult {
    resolve(&[
        ("pkg/ledger.py", LEDGER),
        ("tests/test_ledger.py", TEST_LEDGER),
        ("scripts/summarize.py", SCRIPT),
        ("scripts/attribute_reader.py", ATTRIBUTE_READER),
    ])
}

/// A function in the declaring module reads the constant.
#[test]
fn a_same_file_function_read_reaches_the_constant() {
    let readers = readers_of(&corpus(), "pkg/ledger.py::REQUIRED_CONTROLS");
    assert!(
        readers
            .iter()
            .any(|reader| reader.ends_with("<-pkg/ledger.py::Ledger.verdict")),
        "readers: {readers:?}"
    );
}

/// The reported shape: a parenthesized from-import with lint pragmas, then a
/// read inside a test function.
#[test]
fn an_imported_read_reaches_the_constant_through_a_commented_import() {
    let resolution = corpus();
    for constant in ["REQUIRED_CONTROLS", "DEFAULT_LEDGER_PATH"] {
        let target = format!("pkg/ledger.py::{constant}");
        let readers = readers_of(&resolution, &target);
        assert!(
            readers
                .iter()
                .any(|reader| reader.ends_with("<-tests/test_ledger.py::test_controls")),
            "{constant} readers: {readers:?}"
        );
    }
}

/// A read at module level, with no enclosing function — every line of a
/// script, and the right-hand side of another constant.
#[test]
fn a_module_level_read_reaches_the_constant() {
    let resolution = corpus();
    let script = readers_of(&resolution, "scripts/summarize.py::OUT");
    assert!(
        !script.is_empty(),
        "OUT is read twice at module level: {script:?}"
    );
    let root = readers_of(&resolution, "pkg/ledger.py::REPO_ROOT");
    assert!(
        !root.is_empty(),
        "REPO_ROOT is read by DEFAULT_LEDGER_PATH's value: {root:?}"
    );
}

/// `module.CONSTANT` through an aliased module import.
#[test]
fn a_module_attribute_read_reaches_the_constant() {
    let readers = readers_of(&corpus(), "pkg/ledger.py::REQUIRED_CONTROLS");
    assert!(
        readers
            .iter()
            .any(|reader| reader.ends_with("<-scripts/attribute_reader.py::main")),
        "readers: {readers:?}"
    );
}

/// A Python type is whatever a module-scope name is bound to, so the aliases a
/// typed codebase writes most — `Literal`, `TypeVar`, `NewType`, a
/// union — are assignments, and a base class can be one too
/// (`Base = declarative_base()`). Python has one namespace: the name an
/// annotation reads is the binding lexical scope finds, whatever made it.
///
/// The same-file rung only accepted a class-like kind in type and heritage
/// position, which is right for TypeScript and Rust, where a `const Foo` and a
/// `type Foo` coexist and the annotation means the type. Applied to Python it
/// refused every one of these, so `Purpose = Literal["heldout", "agreement"]`,
/// read by a dataclass field and a parameter, was reported dead.
const TYPED: &str = "\
from typing import Generic, Literal, NewType, TypeVar

Purpose = Literal[\"heldout\", \"agreement\"]
T = TypeVar(\"T\")
UserId = NewType(\"UserId\", int)
Base = declarative_base()


class Record:
    purpose: Purpose


class Box(Generic[T]):
    def get(self) -> T:
        raise NotImplementedError


class User(Base):
    pass


def lookup(user: UserId, purpose: Purpose = \"heldout\") -> None:
    return None
";

#[test]
fn a_type_alias_assignment_is_reached_by_its_annotations() {
    let resolution = resolve(&[("pkg/typed.py", TYPED)]);
    for (alias, reader) in [
        ("Purpose", "pkg/typed.py::Record"),
        ("Purpose", "pkg/typed.py::lookup"),
        ("UserId", "pkg/typed.py::lookup"),
        ("T", "pkg/typed.py::Box.get"),
    ] {
        let readers = readers_of(&resolution, &format!("pkg/typed.py::{alias}"));
        assert!(
            readers
                .iter()
                .any(|edge| edge.ends_with(&format!("<-{reader}"))),
            "{alias} is annotated in {reader}: {readers:?}"
        );
    }
}

#[test]
fn a_base_class_bound_by_assignment_is_reached_by_its_subclass() {
    let resolution = resolve(&[("pkg/typed.py", TYPED)]);
    let readers = readers_of(&resolution, "pkg/typed.py::Base");
    assert!(
        readers
            .iter()
            .any(|edge| edge.ends_with("<-pkg/typed.py::User")),
        "`class User(Base)` names Base: {readers:?}"
    );
}

/// The proof the same-file rung relies on is lexical scope, and it stops at
/// the file. An annotation naming a binding another module declares, with no
/// import of it, has no proof — the name is a builtin, a star import, or a
/// mistake — so the global tier keeps refusing a variable there.
#[test]
fn an_unimported_alias_in_another_module_is_not_a_target() {
    let resolution = resolve(&[
        ("pkg/typed.py", TYPED),
        (
            "pkg/other.py",
            "def describe(purpose: Purpose) -> str:\n    return purpose\n",
        ),
    ]);
    let readers = readers_of(&resolution, "pkg/typed.py::Purpose");
    assert!(
        readers.iter().all(|edge| !edge.contains("pkg/other.py")),
        "pkg/other.py never imports Purpose: {readers:?}"
    );
}

/// An import is proof too, and the import rung never filtered by kind: the
/// alias imported by name is reached by the annotation that uses it.
#[test]
fn an_imported_alias_is_reached_by_its_annotations() {
    let resolution = resolve(&[
        ("pkg/typed.py", TYPED),
        (
            "pkg/other.py",
            "from pkg.typed import Purpose\n\n\ndef describe(purpose: Purpose) -> str:\n    return purpose\n",
        ),
    ]);
    let readers = readers_of(&resolution, "pkg/typed.py::Purpose");
    assert!(
        readers
            .iter()
            .any(|edge| edge.ends_with("<-pkg/other.py::describe")),
        "readers: {readers:?}"
    );
}
