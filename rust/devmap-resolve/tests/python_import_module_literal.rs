//! `mod = importlib.import_module("pkg.util")` with a literal module name is
//! `import pkg.util as mod` written as a call: the same module, bound to the
//! same local, at the same place. A name that is not one literal — a variable,
//! an f-string, a relative name with `package=` — names nothing certain.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

/// (source, target, confidence millis) per edge.
type Edges = Vec<(String, String, i64)>;
/// (source, callee) per unresolved row.
type Unresolved = Vec<(String, String)>;

fn edges(files: &[(&str, &str)]) -> (Edges, Unresolved) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).expect("resolution");
    (
        result
            .edges
            .into_iter()
            .map(|e| (e.source_symbol, e.target_symbol, e.confidence.to_millis()))
            .collect(),
        result
            .unresolved
            .into_iter()
            .map(|row| (row.source_symbol, row.callee_name))
            .collect(),
    )
}

const UTIL: &str = "def helper(x):\n    return x\n";
const DUP: &str = "def helper(x):\n    return -x\n";

#[test]
fn a_literal_import_module_binds_its_local_to_that_module() {
    let caller = "import importlib\n\n\ndef by_import():\n    mod = importlib.import_module(\"pkg.util\")\n    return mod.helper(1)\n";
    let (edges, unresolved) = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", UTIL),
        ("pkg/dyn.py", caller),
        ("other/__init__.py", ""),
        ("other/dup.py", DUP),
    ]);
    assert!(
        edges
            .iter()
            .any(|(s, t, _)| s == "pkg/dyn.py::by_import" && t == "pkg/util.py::helper"),
        "{edges:?} unresolved={unresolved:?}"
    );
    assert!(
        !edges
            .iter()
            .any(|(s, t, _)| s == "pkg/dyn.py::by_import" && t == "other/dup.py::helper"),
        "{edges:?}"
    );
}

#[test]
fn a_from_import_of_import_module_is_the_same_call() {
    let caller = "from importlib import import_module\n\n\ndef by_import():\n    mod = import_module(\"pkg.util\")\n    return mod.helper(1)\n";
    let (edges, unresolved) = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", UTIL),
        ("pkg/dyn.py", caller),
    ]);
    assert!(
        edges
            .iter()
            .any(|(s, t, _)| s == "pkg/dyn.py::by_import" && t == "pkg/util.py::helper"),
        "{edges:?} unresolved={unresolved:?}"
    );
}

#[test]
fn a_non_literal_or_relative_import_module_binds_nothing_certain() {
    let caller = "import importlib\n\n\ndef by_var(name):\n    mod = importlib.import_module(name)\n    return mod.helper(1)\n\n\n\
def by_relative():\n    mod = importlib.import_module(\".util\", package=\"pkg\")\n    return mod.helper(1)\n\n\n\
def by_fstring(n):\n    mod = importlib.import_module(f\"pkg.{n}\")\n    return mod.helper(1)\n";
    let (edges, _) = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", UTIL),
        ("pkg/dyn.py", caller),
    ]);
    for caller in ["by_var", "by_relative", "by_fstring"] {
        let source = format!("pkg/dyn.py::{caller}");
        assert!(
            !edges
                .iter()
                .any(|(s, t, millis)| *s == source && t == "pkg/util.py::helper" && *millis >= 900),
            "{caller}: {edges:?}"
        );
    }
}

#[test]
fn import_module_on_some_other_receiver_is_not_importlib() {
    let caller = "class Loader:\n    def import_module(self, name):\n        return name\n\n\n\
def by_method(loader):\n    mod = loader.import_module(\"pkg.util\")\n    return mod.helper(1)\n";
    let (edges, _) = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", UTIL),
        ("pkg/dyn.py", caller),
    ]);
    assert!(
        !edges
            .iter()
            .any(|(s, t, millis)| s == "pkg/dyn.py::by_method"
                && t == "pkg/util.py::helper"
                && *millis >= 900),
        "{edges:?}"
    );
}

#[test]
fn a_rebound_handle_or_a_sibling_scopes_local_binds_nothing_certain() {
    let caller = "import importlib\n\n\ndef rebound(other):\n    mod = importlib.import_module(\"pkg.util\")\n    mod = other\n    return mod.helper(1)\n\n\n\
def loads():\n    mod = importlib.import_module(\"pkg.util\")\n    return mod\n\n\n\
def sibling(mod):\n    return mod.helper(1)\n";
    let (edges, _) = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", UTIL),
        ("pkg/dyn.py", caller),
    ]);
    for caller in ["rebound", "sibling"] {
        let source = format!("pkg/dyn.py::{caller}");
        assert!(
            !edges
                .iter()
                .any(|(s, t, millis)| *s == source && t == "pkg/util.py::helper" && *millis >= 900),
            "{caller}: {edges:?}"
        );
    }
}
