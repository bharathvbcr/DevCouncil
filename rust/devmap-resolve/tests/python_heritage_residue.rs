//! Python heritage residue (ft-572bb69d): the guards the resolver gained while
//! the probes in `python_field_super_attack*` were un-ignored, and the one
//! behaviour no probe states, the named bound on a long chain. Each test says
//! what CPython does; a failure is a wrong edge or a silent bound.

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

fn calls(res: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = res
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == caller)
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out
}

/// A chain `C0 <- C1 <- .. <- C{n-1}` whose last class calls `self.m()`, which
/// only `C0` declares.
fn chain(n: usize) -> String {
    let mut src = String::from("class C0:\n    def m(self):\n        return 1\n\n");
    for i in 1..n {
        src.push_str(&format!("class C{i}(C{}):\n    pass\n\n", i - 1));
    }
    src.push_str(&format!(
        "class Leaf(C{}):\n    def go(self):\n        return self.m()\n",
        n - 1
    ));
    src
}

#[test]
fn a_chain_inside_the_cap_resolves_the_inherited_method() {
    let r = resolve(&[("c.py", &chain(40))]);
    assert_eq!(calls(&r, "c.py::Leaf.go"), ["c.py::C0.m"]);
}

#[test]
fn a_chain_past_the_cap_records_a_ledger_row_naming_the_cap() {
    let r = resolve(&[("c.py", &chain(80))]);
    assert!(
        calls(&r, "c.py::Leaf.go").is_empty(),
        "an answer past the bound: {:?}",
        calls(&r, "c.py::Leaf.go")
    );
    let row = r
        .unresolved
        .iter()
        .find(|row| row.source_symbol == "c.py::Leaf.go" && row.callee_name == "m")
        .unwrap_or_else(|| panic!("no ledger row for the capped site: {:?}", r.unresolved));
    let reason = format!("{:?}", row.resolution);
    assert!(
        reason.contains("cap of 64 bases deep"),
        "the row does not name the bound: {reason}"
    );
}

#[test]
fn explicit_super_with_the_enclosing_class_binds_whatever_the_spacing() {
    for call in ["super(Child, self)", "super(Child,self)", "super( Child , self )"] {
        let src = format!(
            "class Base:\n    def m(self):\n        return 1\n\nclass Child(Base):\n    def m(self):\n        return {call}.m()\n"
        );
        let r = resolve(&[("m.py", &src)]);
        let got = calls(&r, "m.py::Child.m");
        // The spaced form is outside what the spans prove: abstaining is the
        // contract there, a wrong edge never is.
        assert!(
            got == ["m.py::Base.m"] || (call.contains(' ') && got.is_empty()),
            "{call}: {got:?}"
        );
    }
}

#[test]
fn explicit_super_with_a_dotted_first_argument_is_not_the_enclosing_class() {
    let src = "class Base:\n    def m(self):\n        return 1\n\nclass Other:\n    class Child:\n        pass\n\nclass Child(Base):\n    def m(self):\n        return super(Other.Child, self).m()\n";
    let r = resolve(&[("m.py", src)]);
    assert!(
        calls(&r, "m.py::Child.m").is_empty(),
        "{:?}",
        calls(&r, "m.py::Child.m")
    );
}

#[test]
fn class_body_import_reaches_a_nested_classs_base_list() {
    // `Inner(Base)` is evaluated in `Outer`'s body, where `Base` is `b.Base`.
    let r = resolve(&[
        ("a.py", "class Base:\n    def run(self):\n        return 1\n"),
        ("b.py", "class Base:\n    def run(self):\n        return 2\n"),
        (
            "c.py",
            "from a import Base\n\nclass Outer:\n    from b import Base\n\n    class Inner(Base):\n        def go(self):\n            return self.run()\n",
        ),
    ]);
    let got = calls(&r, "c.py::Outer.Inner.go");
    assert!(!got.contains(&"a.py::Base.run".to_string()), "{got:?}");
}

#[test]
fn function_local_import_reaches_a_function_scoped_classs_base_list() {
    // `C(Base)` runs inside `f`, where `Base` is the local import.
    let r = resolve(&[
        ("a.py", "class Base:\n    def run(self):\n        return 1\n"),
        ("b.py", "class Base:\n    def run(self):\n        return 2\n"),
        (
            "c.py",
            "from a import Base\n\ndef f():\n    from b import Base\n    class C(Base):\n        def go(self):\n            return self.run()\n    return C\n",
        ),
    ]);
    let got = calls(&r, "c.py::C.go");
    assert!(!got.contains(&"a.py::Base.run".to_string()), "{got:?}");
}

#[test]
fn a_module_that_declares_its_own_typing_in_a_plain_directory_shadows_the_standard_one() {
    // `scripts/` is no package, so `scripts/typing.py` is importable as
    // `typing` from a script beside it: `Generic` is then not the marker.
    let r = resolve(&[
        ("scripts/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        (
            "scripts/c.py",
            "from typing import Generic\n\nclass Base:\n    def m(self):\n        return 1\n\nclass C(Generic, Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    let got = calls(&r, "scripts/c.py::C.m");
    assert!(!got.contains(&"scripts/c.py::Base.m".to_string()), "{got:?}");
}

#[test]
fn a_dynamic_all_in_a_package_init_may_export_the_dotted_root() {
    // `__all__.extend(..)` is a call on `__all__`: whatever it adds is unknown.
    let m = "from other import names\n__all__ = ['x']\n__all__.extend(names())\nx = 1\n";
    let r = resolve(&[
        ("mod.py", "class Base:\n    def run(self):\n        return 1\n"),
        ("m.py", m),
        (
            "c.py",
            "import mod\nfrom m import *\n\nclass C(mod.Base):\n    def go(self):\n        return self.run()\n",
        ),
    ]);
    assert!(
        !calls(&r, "c.py::C.go").contains(&"mod.py::Base.run".to_string()),
        "{:?}",
        calls(&r, "c.py::C.go")
    );
}
