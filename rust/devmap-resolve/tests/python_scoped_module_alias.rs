//! A Python module alias imported *inside a function* and read there as a
//! value: `cr.CONST` after `from . import m as cr` (ft-c666ba7c, fixture row
//! `py.AR1`; MLSystemsLab reads twelve `*_ARMS` constants this way).
//!
//! The import also records `cr` as an untyped local of the function, and the
//! reference ladder refuses any read of a local. The import is the local's
//! value only when nothing else could be, so each case below that could mean
//! something else abstains.
//!
//! Calls through such an alias (`cr.fn()`) take the call ladder, which has the
//! same refusal and is not changed here.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

fn reads(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::References)
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect()
}

fn reaches(found: &[(String, String)], from: &str, to: &str) -> bool {
    found.iter().any(|(f, t)| f == from && t == to)
}

const M: &str = "CONST_DEAD = 1\nCONST_READ = 2\n";

#[test]
fn a_function_local_alias_read_binds_to_the_module_constant() {
    for import in [
        "from . import m as cr",
        "from pkg import m as cr",
        "import pkg.m as cr",
    ] {
        let user = format!("def check():\n    {import}\n\n    return cr.CONST_READ\n");
        let found = reads(&[
            ("pkg/__init__.py", ""),
            ("pkg/m.py", M),
            ("pkg/use.py", &user),
        ]);
        assert!(
            reaches(&found, "pkg/use.py::check", "pkg/m.py::CONST_READ"),
            "{import}: {found:?}"
        );
        assert!(
            !reaches(&found, "pkg/use.py::check", "pkg/m.py::CONST_DEAD"),
            "{import}: {found:?}"
        );
    }
}

#[test]
fn two_functions_with_one_alias_for_two_modules_each_bind_their_own() {
    let user = "def one():\n    from . import m as cr\n\n    return cr.CONST_READ\n\n\ndef two():\n    from . import n as cr\n\n    return cr.CONST_READ\n";
    let found = reads(&[
        ("pkg/__init__.py", ""),
        ("pkg/m.py", M),
        ("pkg/n.py", "CONST_READ = 3\n"),
        ("pkg/use.py", user),
    ]);
    assert!(
        reaches(&found, "pkg/use.py::one", "pkg/m.py::CONST_READ"),
        "{found:?}"
    );
    assert!(
        reaches(&found, "pkg/use.py::two", "pkg/n.py::CONST_READ"),
        "{found:?}"
    );
    assert!(
        !reaches(&found, "pkg/use.py::one", "pkg/n.py::CONST_READ"),
        "{found:?}"
    );
    assert!(
        !reaches(&found, "pkg/use.py::two", "pkg/m.py::CONST_READ"),
        "{found:?}"
    );
}

fn abstains(user: &str, module: &str, extra: &[(&str, &str)]) {
    let mut files = vec![
        ("pkg/__init__.py", ""),
        ("pkg/m.py", module),
        ("pkg/use.py", user),
    ];
    files.extend_from_slice(extra);
    let found = reads(&files);
    assert!(
        !reaches(&found, "pkg/use.py::check", "pkg/m.py::CONST_READ"),
        "must abstain for:\n{user}\n{found:?}"
    );
}

#[test]
fn the_alias_rebound_in_the_function_abstains() {
    abstains(
        "def other():\n    return 1\n\n\ndef check():\n    from . import m as cr\n\n    cr = other()\n    return cr.CONST_READ\n",
        M,
        &[],
    );
}

#[test]
fn a_parameter_of_the_same_name_without_an_import_abstains() {
    abstains("def check(cr):\n    return cr.CONST_READ\n", M, &[]);
}

#[test]
fn the_same_alias_imported_from_two_places_in_one_function_abstains() {
    abstains(
        "def check():\n    from . import m as cr\n    from . import n as cr\n\n    return cr.CONST_READ\n",
        M,
        &[("pkg/n.py", "CONST_READ = 3\n")],
    );
}

#[test]
fn a_module_level_name_the_alias_shadows_abstains() {
    abstains(
        "cr = 5\n\n\ndef check():\n    from . import m as cr\n\n    return cr.CONST_READ\n",
        M,
        &[],
    );
    abstains(
        "from . import n as cr\n\n\ndef check():\n    from . import m as cr\n\n    return cr.CONST_READ\n",
        M,
        &[("pkg/n.py", "CONST_READ = 3\n")],
    );
}

#[test]
fn a_target_module_with_a_module_getattr_abstains() {
    abstains(
        "def check():\n    from . import m as cr\n\n    return cr.CONST_READ\n",
        "CONST_READ = 2\n\n\ndef __getattr__(name):\n    return 0\n",
        &[],
    );
}
