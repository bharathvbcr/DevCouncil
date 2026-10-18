//! Adversarial probes for `lane/py-residue` @ 9e8f2a26 (delta b65fec56..9e8f2a26):
//! `python_all_is_dynamic`, the scoped-import table, the function-scoped base
//! rule, the stdlib-shadow rule, the explicit `super(X, self)` read, repeated
//! class declarations, and the heritage cap ledger row. Each test states what
//! CPython does; a failing test is a BREAK (or a FINDING where its name says
//! so). Nothing here fixes anything.

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

fn has(list: &[String], target: &str) -> bool {
    list.iter().any(|t| t == target)
}

// ---------------------------------------------------------------- 1. __all__

/// `from a import helper` then `from m import *` then `helper()`. When `m`
/// exports a known list that lacks `helper`, the star import cannot rebind it
/// and the call is `a.helper`. Returns the call targets of `u.py::f`.
fn star_case(m_src: &str) -> Vec<String> {
    // Probe sources spell the exported class `helper`; the star import
    // matters where a class statement reads its base through the name.
    let m_src = m_src
        .replace("def helper():\n    return 2\n", "class Base:\n    pass\n")
        .replace("helper", "Base");
    let r = resolve(&[
        ("a.py", "class Base:\n    def run(self):\n        return 1\n"),
        ("m.py", &m_src),
        (
            "u.py",
            "from a import Base\nfrom m import *\n\nclass C(Base):\n    def go(self):\n        return self.run()\n",
        ),
    ]);
    calls(&r, "u.py::C.go")
}

fn assert_literal_all_keeps_the_binding(m_src: &str) {
    let got = star_case(m_src);
    assert!(
        has(&got, "a.py::Base.run"),
        "a literal __all__ that omits `helper` flipped to dynamic: {got:?}\n{m_src}"
    );
}

#[test]
fn all_double_quotes() {
    assert_literal_all_keeps_the_binding("__all__ = [\"x\"]\nx = 1\n");
}

#[test]
fn all_single_quotes() {
    assert_literal_all_keeps_the_binding("__all__ = ['x']\nx = 1\n");
}

#[test]
fn all_multiline_comments_trailing_comma() {
    assert_literal_all_keeps_the_binding(
        "__all__ = [\n    # the public surface\n    \"x\",  # first\n    'y',\n    # \"helper\" is deliberately absent\n]\nx = 1\ny = 2\n",
    );
}

#[test]
fn all_tuple() {
    assert_literal_all_keeps_the_binding("__all__ = (\"x\", 'y',)\nx = 1\ny = 2\n");
}

#[test]
fn all_annotated() {
    assert_literal_all_keeps_the_binding("__all__: list[str] = [\"x\"]\nx = 1\n");
}

#[test]
fn all_empty_list() {
    assert_literal_all_keeps_the_binding("__all__ = []\nx = 1\n");
}

#[test]
fn all_with_a_docstring_mentioning_a_name() {
    assert_literal_all_keeps_the_binding(
        "\"\"\"Exports x. Not helper.\"\"\"\n__all__ = [\"x\"]\nx = 1\n",
    );
}

#[test]
fn all_with_a_module_docstring_that_quotes_an_all_assignment() {
    // A docstring that shows the literal form. CPython: the module-level
    // `__all__` is the real one. The extractor text-scans the docstring too.
    assert_literal_all_keeps_the_binding(
        "\"\"\"Usage:\n\n    __all__ = ['zzz']\n\"\"\"\n__all__ = [\"x\"]\nx = 1\n",
    );
}

#[test]
fn all_in_a_type_checking_branch() {
    assert_literal_all_keeps_the_binding(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    __all__ = [\"x\"]\nx = 1\n",
    );
}

#[test]
fn all_in_a_try_branch() {
    assert_literal_all_keeps_the_binding(
        "try:\n    __all__ = [\"x\"]\nexcept ImportError:\n    __all__ = [\"x\"]\nx = 1\n",
    );
}

#[test]
fn all_with_literal_plus_equals_literal() {
    // Precision: `+=` with a list literal is still a literal list.
    assert_literal_all_keeps_the_binding("__all__ = [\"x\"]\n__all__ += [\"y\"]\nx = 1\ny = 2\n");
}

#[test]
fn all_reassigned_to_a_different_literal_drops_the_first() {
    // CPython: the last assignment wins, so `helper` is not exported and the
    // star import cannot rebind it. The table unions both: abstaining is safe,
    // so this is a precision gap, not a wrong edge.
    assert_literal_all_keeps_the_binding("__all__ = [\"helper\"]\n__all__ = [\"x\"]\nx = 1\n");
}

#[test]
fn all_with_a_fstring_entry_is_dynamic() {
    // `f"{prefix}elper"` evaluates to `helper`: star import rebinds it.
    let got = star_case("prefix = 'B'\n__all__ = [f\"{prefix}ase\"]\ndef helper():\n    return 2\n");
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_concatenation_is_dynamic() {
    let got = star_case("__all__ = [\"x\"] + [\"helper\"]\nx = 1\ndef helper():\n    return 2\n");
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_extend_is_dynamic() {
    let got = star_case("__all__ = [\"x\"]\n__all__.extend([\"helper\"])\nx = 1\ndef helper():\n    return 2\n");
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_append_in_a_function_called_at_import_is_dynamic() {
    // `register()` runs at import and appends: `helper` is exported.
    let got = star_case(
        "__all__ = [\"x\"]\ndef register():\n    __all__.append(\"helper\")\nregister()\nx = 1\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_extended_from_another_modules_all_is_dynamic() {
    let got = star_case(
        "import sub\n__all__ = [\"x\"] + sub.__all__\nx = 1\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_mutated_through_globals_is_dynamic() {
    let got = star_case(
        "__all__ = [\"x\"]\nglobals()[\"__all__\"].append(\"helper\")\nx = 1\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_set_through_setattr_on_module_is_dynamic() {
    let got = star_case(
        "import sys\n__all__ = [\"x\"]\nsetattr(sys.modules[__name__], \"__all__\", [\"helper\"])\nx = 1\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_augmented_with_a_name_reference_is_dynamic() {
    let got = star_case(
        "_extra = [\"helper\"]\n__all__ = [\"x\"]\n__all__ += _extra\nx = 1\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_assigned_a_name_reference_is_dynamic() {
    let got = star_case(
        "_names = [\"helper\"]\n__all__ = _names\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_class_body_all_does_not_make_a_module_all() {
    // `class K: __all__ = ['helper']` is a class attribute; the module has no
    // `__all__`, so star imports every public name (helper included).
    let got = star_case(
        "class K:\n    __all__ = [\"x\"]\n\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_in_a_function_body_does_not_make_a_module_all() {
    // Module has no `__all__`: `import *` takes `helper`.
    let got = star_case(
        "def f():\n    __all__ = [\"x\"]\n\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_name_with_a_matching_module_literal_elsewhere_is_not_a_module_all() {
    // The function-body `__all__` is text-scanned. A module-level string that
    // happens to equal the scanned name satisfies the "has a module-level
    // literal" test. CPython: no module `__all__`, so `helper` is exported.
    let got = star_case(
        "TAG = \"x\"\ndef f():\n    __all__ = [\"x\"]\n\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_docstring_assignment_with_a_module_literal_is_not_a_module_all() {
    let got = star_case(
        "\"\"\"\n__all__ = ['x']\n\"\"\"\nTAG = \"x\"\n\ndef helper():\n    return 2\n",
    );
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

#[test]
fn all_literal_list_inside_a_conditional_with_a_dynamic_else_is_dynamic() {
    let got = star_case(
        "import os\nif os.name == 'nt':\n    __all__ = [\"x\"]\nelse:\n    __all__ = [\"x\", \"helper\"]\nx = 1\ndef helper():\n    return 2\n",
    );
    // Union of two literals: includes helper, so abstaining is right.
    assert!(!has(&got, "a.py::Base.run"), "{got:?}");
}

// --------------------------------------------------- 2. scoped imports table

#[test]
fn method_local_unaliased_import_types_a_self_field() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from other import A\n\nclass C:\n    def __init__(self):\n        from m import A\n        self.x = A()\n\n    def go(self):\n        return self.x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::C.go");
    assert!(!has(&got, "other.py::A.hi"), "leaked the module import: {got:?}");
}

#[test]
fn method_local_unaliased_import_binds_the_call_in_that_method() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from other import A\n\ndef go():\n    from m import A\n    x = A()\n    return x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::go");
    assert!(!has(&got, "other.py::A.hi"), "{got:?}");
}

#[test]
fn inner_function_import_does_not_shadow_the_outer_function() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from m import A\n\ndef outer():\n    def inner():\n        from other import A\n        return A()\n    x = A()\n    return x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::outer");
    assert!(
        !has(&got, "other.py::A.hi"),
        "inner's local import leaked outward: {got:?}"
    );
}

#[test]
fn outer_function_import_reaches_the_inner_function_by_closure() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from m import A\n\ndef outer():\n    from other import A\n    def inner():\n        x = A()\n        return x.hi()\n    return inner\n",
        ),
    ]);
    let got = calls(&r, "c.py::outer.inner");
    assert!(!has(&got, "m.py::A.hi"), "closure ignored: {got:?}");
}

#[test]
fn conditional_function_import_is_not_the_module_name() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from m import A\n\ndef f(flag):\n    if flag:\n        from other import A\n    x = A()\n    return x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::f");
    assert!(
        !has(&got, "m.py::A.hi"),
        "A is local to f for the whole body: {got:?}"
    );
}

#[test]
fn function_import_of_a_dotted_module_binds_its_first_name() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/sub.py", "def run():\n    return 1\n"),
        ("c.py", "def f():\n    import pkg.sub\n    return pkg.sub.run()\n"),
    ]);
    assert_eq!(calls(&r, "c.py::f"), ["pkg/sub.py::run"]);
}

#[test]
fn function_import_a_b_shadows_module_import_a_alias() {
    // Module: `import util` (util.py: run). Function: `import other.util`
    // binds `other`, not `util`: `util.run()` still reaches util.py.
    let r = resolve(&[
        ("util.py", "def run():\n    return 1\n"),
        ("other/__init__.py", ""),
        ("other/util.py", "def run():\n    return 2\n"),
        (
            "c.py",
            "import util\n\ndef f():\n    import other.util\n    return util.run()\n",
        ),
    ]);
    assert_eq!(calls(&r, "c.py::f"), ["util.py::run"]);
}

#[test]
fn method_does_not_see_a_class_body_import() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from other import A\n\nclass K:\n    from m import A\n\n    def go(self):\n        x = A()\n        return x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::K.go");
    assert!(
        !has(&got, "m.py::A.hi"),
        "a method read a class attribute as a free name: {got:?}"
    );
}

#[test]
fn class_body_import_reached_through_the_class_name() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from other import A\n\nclass K:\n    from m import A\n\n    def go(self):\n        x = K.A()\n        return x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::K.go");
    assert!(!has(&got, "other.py::A.hi"), "{got:?}");
}

#[test]
fn import_inside_a_nested_class_inside_a_method_does_not_leak_to_the_method() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from m import A\n\ndef f():\n    class Tmp:\n        from other import A\n    x = A()\n    return x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::f");
    assert!(!has(&got, "other.py::A.hi"), "{got:?}");
}

#[test]
fn import_inside_a_lambda_free_comprehension_default_scope() {
    // An import in `if TYPE_CHECKING:` at module scope is module scope, not a
    // scoped import: the module import must still type the receiver.
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        (
            "c.py",
            "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from m import A\n\ndef f():\n    x = A()\n    return x.hi()\n",
        ),
    ]);
    assert!(has(&calls(&r, "c.py::f"), "m.py::A.hi"), "{:?}", calls(&r, "c.py::f"));
}

#[test]
fn method_import_of_the_same_name_in_a_sibling_method_does_not_leak() {
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from m import A\n\nclass C:\n    def one(self):\n        from other import A\n        return A()\n\n    def two(self):\n        x = A()\n        return x.hi()\n",
        ),
    ]);
    let got = calls(&r, "c.py::C.two");
    assert!(!has(&got, "other.py::A.hi"), "{got:?}");
}

#[test]
fn module_import_after_a_function_import_of_the_same_name_still_types_other_functions() {
    // Recall: a scoped import in one function must not poison the module name
    // for every other function.
    let r = resolve(&[
        ("m.py", "class A:\n    def hi(self):\n        return 1\n"),
        ("other.py", "class A:\n    def hi(self):\n        return 2\n"),
        (
            "c.py",
            "from m import A\n\ndef one():\n    from other import A\n    return A()\n\ndef two():\n    x = A()\n    return x.hi()\n",
        ),
    ]);
    assert!(has(&calls(&r, "c.py::two"), "m.py::A.hi"), "{:?}", calls(&r, "c.py::two"));
}

// ------------------------------------- 3. function-scoped base (flat table)

#[test]
fn function_local_base_shadowing_a_module_class_is_not_the_module_class() {
    let r = resolve(&[(
        "c.py",
        "class Base:\n    def m(self):\n        return 'module'\n\ndef make():\n    class Base:\n        pass\n    class D(Base):\n        def go(self):\n            return self.m()\n    return D\n",
    )]);
    let got = calls(&r, "c.py::D.go");
    assert!(
        got.is_empty(),
        "the local Base has no m; the module's Base is shadowed: {got:?}"
    );
}

#[test]
fn function_local_base_shadowing_with_a_module_class_that_has_the_method_and_a_local_that_overrides() {
    let r = resolve(&[(
        "c.py",
        "class Base:\n    def m(self):\n        return 'module'\n\ndef make():\n    class Base:\n        def m(self):\n            return 'local'\n    class D(Base):\n        def go(self):\n            return self.m()\n    return D\n",
    )]);
    let got = calls(&r, "c.py::D.go");
    assert!(got.len() <= 1, "{got:?}");
}

#[test]
fn function_local_class_through_an_import_resolves() {
    let r = resolve(&[
        ("lib.py", "class L:\n    def m(self):\n        return 1\n"),
        (
            "c.py",
            "from lib import L\n\ndef make():\n    class D(L):\n        def go(self):\n            return self.m()\n    return D\n",
        ),
    ]);
    let got = calls(&r, "c.py::D.go");
    assert!(got.is_empty() || got == ["lib.py::L.m"], "{got:?}");
}

#[test]
fn function_local_chain_with_an_opaque_middle_base_abstains() {
    // `Mid(make_base(), Real)`: MRO is Mid, <opaque>, Real. The opaque base is
    // ahead of `Real` and may define `m`; nothing may bind Real.m.
    let r = resolve(&[(
        "c.py",
        "class Real:\n    def m(self):\n        return 1\n\ndef make_base():\n    return object\n\ndef make():\n    class Mid(make_base(), Real):\n        pass\n    class Leaf(Mid):\n        def go(self):\n            return self.m()\n    return Leaf\n",
    )]);
    let got = calls(&r, "c.py::Leaf.go");
    assert!(
        !has(&got, "c.py::Real.m"),
        "an opaque base earlier in the MRO was skipped: {got:?}"
    );
}

#[test]
fn function_local_root_with_opaque_and_real_base_abstains() {
    let r = resolve(&[(
        "c.py",
        "class Real:\n    def m(self):\n        return 1\n\ndef make_base():\n    return object\n\ndef make():\n    class Leaf(make_base(), Real):\n        def go(self):\n            return self.m()\n    return Leaf\n",
    )]);
    let got = calls(&r, "c.py::Leaf.go");
    assert!(!has(&got, "c.py::Real.m"), "{got:?}");
}

#[test]
fn function_local_chain_with_a_diamond_in_the_middle_abstains_or_is_c3() {
    // Leaf -> Mid(X, Y) where X(Base), Y(Base) and Y overrides m: C3 gives
    // Mid, X, Y, Base: Y.m. The flat table is breadth-first: Base.m would be
    // wrong; X and Y are module-level.
    let r = resolve(&[(
        "c.py",
        "class Base:\n    def m(self):\n        return 0\n\nclass X(Base):\n    pass\n\nclass Y(Base):\n    def m(self):\n        return 2\n\ndef make():\n    class Mid(X, Y):\n        pass\n    class Leaf(Mid):\n        def go(self):\n            return self.m()\n    return Leaf\n",
    )]);
    let got = calls(&r, "c.py::Leaf.go");
    assert!(!has(&got, "c.py::Base.m"), "{got:?}");
}

#[test]
fn function_local_chain_past_the_cap_makes_no_wrong_edge() {
    let mut src = String::from("def make():\n    class C0:\n        def m(self):\n            return 1\n");
    for i in 1..80 {
        src.push_str(&format!("    class C{i}(C{}):\n        pass\n", i - 1));
    }
    src.push_str("    class Leaf(C79):\n        def go(self):\n            return self.m()\n    return Leaf\n");
    let r = resolve(&[("c.py", &src)]);
    let got = calls(&r, "c.py::Leaf.go");
    assert!(got.is_empty() || got == ["c.py::C0.m"], "{got:?}");
}

// ----------------------------------------------- 4. stdlib typing shadowing

const GEN_C: &str = "class Base:\n    def m(self):\n        return 1\n\nclass C(Generic, Base):\n    def m(self):\n        return super().m()\n";

#[test]
fn package_sibling_typing_does_not_shadow_the_stdlib_absolute_import() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        (
            "pkg/c.py",
            &format!("from typing import Generic\n\n{GEN_C}"),
        ),
    ]);
    assert_eq!(calls(&r, "pkg/c.py::C.m"), ["pkg/c.py::Base.m"]);
}

#[test]
fn package_relative_typing_import_is_the_local_module() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        (
            "pkg/c.py",
            &format!("from .typing import Generic\n\n{GEN_C}"),
        ),
    ]);
    let got = calls(&r, "pkg/c.py::C.m");
    assert!(!has(&got, "pkg/c.py::Base.m"), "{got:?}");
}

#[test]
fn package_from_dot_import_typing_is_the_local_module() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        (
            "pkg/c.py",
            "from . import typing\n\nclass Base:\n    def m(self):\n        return 1\n\nclass C(typing.Generic, Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    let got = calls(&r, "pkg/c.py::C.m");
    assert!(!has(&got, "pkg/c.py::Base.m"), "{got:?}");
}

#[test]
fn package_import_typing_attribute_is_stdlib_even_with_a_sibling() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        (
            "pkg/c.py",
            "import typing\n\nclass Base:\n    def m(self):\n        return 1\n\nclass C(typing.Generic, Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(calls(&r, "pkg/c.py::C.m"), ["pkg/c.py::Base.m"]);
}

#[test]
fn namespace_directory_typing_shadows_conservatively() {
    // No `__init__.py`: run as a script, the directory is on sys.path.
    let r = resolve(&[
        ("ns/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        ("ns/c.py", &format!("from typing import Generic\n\n{GEN_C}")),
    ]);
    let got = calls(&r, "ns/c.py::C.m");
    assert!(!has(&got, "ns/c.py::Base.m"), "{got:?}");
}

#[test]
fn root_level_typing_shadows_even_for_a_package_file() {
    let r = resolve(&[
        ("typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        ("pkg/__init__.py", ""),
        ("pkg/c.py", &format!("from typing import Generic\n\n{GEN_C}")),
    ]);
    let got = calls(&r, "pkg/c.py::C.m");
    assert!(!has(&got, "pkg/c.py::Base.m"), "{got:?}");
}

#[test]
fn src_level_typing_shadows_even_for_a_package_file() {
    let r = resolve(&[
        ("src/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/c.py", &format!("from typing import Generic\n\n{GEN_C}")),
    ]);
    let got = calls(&r, "src/pkg/c.py::C.m");
    assert!(!has(&got, "src/pkg/c.py::Base.m"), "{got:?}");
}

#[test]
fn sys_path_insert_of_the_package_dir_shadows_typing() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        (
            "pkg/c.py",
            &format!(
                "import os\nimport sys\n\nsys.path.insert(0, os.path.dirname(__file__))\nfrom typing import Generic\n\n{GEN_C}"
            ),
        ),
    ]);
    let got = calls(&r, "pkg/c.py::C.m");
    assert!(!has(&got, "pkg/c.py::Base.m"), "{got:?}");
}

#[test]
fn nested_package_dir_typing_with_init_is_stdlib_for_deep_importer() {
    let r = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
        ("a/b/c.py", &format!("from typing import Generic\n\n{GEN_C}")),
    ]);
    assert_eq!(calls(&r, "a/b/c.py::C.m"), ["a/b/c.py::Base.m"]);
}

#[test]
fn package_typing_package_directory_is_stdlib_too() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/typing/__init__.py", "class Generic:\n    def m(self):\n        return 0\n"),
        ("pkg/c.py", &format!("from typing import Generic\n\n{GEN_C}")),
    ]);
    assert_eq!(calls(&r, "pkg/c.py::C.m"), ["pkg/c.py::Base.m"]);
}

#[test]
fn abc_sibling_in_a_package_is_stdlib_for_absolute_import() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/abc.py", "class ABC:\n    def m(self):\n        return 0\n"),
        (
            "pkg/c.py",
            "from abc import ABC\n\nclass Base:\n    def m(self):\n        return 1\n\nclass C(ABC, Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(calls(&r, "pkg/c.py::C.m"), ["pkg/c.py::Base.m"]);
}

// ------------------------------------------------- 5. explicit super(X, self)

fn super_case(call: &str) -> Vec<String> {
    let src = format!(
        "class Base:\n    def m(self):\n        return 1\n\nclass Child(Base):\n    def m(self):\n        return {call}.m()\n"
    );
    calls(&resolve(&[("m.py", &src)]), "m.py::Child.m")
}

#[test]
fn super_type_self_self_is_not_the_enclosing_class() {
    // CPython: `super(type(self), self)` is the next class after the *runtime*
    // class: infinite recursion for a subclass. Not Child's bases.
    let got = super_case("super(type(self), self)");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn super_self_class_self_is_not_the_enclosing_class() {
    let got = super_case("super(self.__class__, self)");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn super_dotted_outer_inner() {
    let src = "class Base:\n    def m(self):\n        return 1\n\nclass Outer:\n    class Inner(Base):\n        def m(self):\n            return super(Outer.Inner, self).m()\n";
    let got = calls(&resolve(&[("m.py", src)]), "m.py::Outer.Inner.m");
    // Correct (Inner's bases) or abstain; never anything else.
    assert!(got.is_empty() || got == ["m.py::Base.m"], "{got:?}");
}

#[test]
fn super_cls_in_a_classmethod() {
    let src = "class Base:\n    @classmethod\n    def make(cls):\n        return 1\n\nclass Child(Base):\n    @classmethod\n    def make(cls):\n        return super(Child, cls).make()\n";
    let got = calls(&resolve(&[("m.py", src)]), "m.py::Child.make");
    assert!(got.is_empty() || got == ["m.py::Base.make"], "{got:?}");
}

#[test]
fn super_with_an_alias_of_another_class_is_not_the_enclosing_class() {
    // `Other = Mid`: super(Other, self) starts after Mid, not after Child.
    let src = "class Base:\n    def m(self):\n        return 'base'\n\nclass Mid(Base):\n    def m(self):\n        return 'mid'\n\nclass Child(Mid):\n    def m(self):\n        Other = Base\n        return super(Other, self).m()\n";
    let got = calls(&resolve(&[("m.py", src)]), "m.py::Child.m");
    assert!(!has(&got, "m.py::Mid.m"), "{got:?}");
}

#[test]
fn super_with_an_imported_alias_of_the_enclosing_name_in_another_file() {
    // `from b import Child as K`: b.py's own Child; c.py's class is also named
    // Child. `super(K, self)` starts after b.Child, not c.Child's bases.
    let r = resolve(&[
        ("b.py", "class Child:\n    def m(self):\n        return 1\n"),
        ("base.py", "class Base:\n    def m(self):\n        return 2\n"),
        (
            "c.py",
            "from b import Child as K\nfrom base import Base\n\nclass Child(Base, K):\n    def m(self):\n        return super(K, self).m()\n",
        ),
    ]);
    let got = calls(&r, "c.py::Child.m");
    assert!(!has(&got, "base.py::Base.m"), "{got:?}");
}

#[test]
fn super_naming_a_module_level_class_that_shares_the_nested_classs_bare_name() {
    // `Wrapper.Child` is nested; a method's free name `Child` is the
    // module-level `Child`, so `super(Child, self)` is not Wrapper.Child's.
    let src = "class Base:\n    def m(self):\n        return 1\n\nclass Child:\n    pass\n\nclass Wrapper:\n    class Child(Base):\n        def m(self):\n            return super(Child, self).m()\n";
    let got = calls(&resolve(&[("m.py", src)]), "m.py::Wrapper.Child.m");
    assert!(!has(&got, "m.py::Base.m"), "{got:?}");
}

#[test]
fn super_with_starred_second_argument() {
    let got = super_case("super(Child, *args)");
    assert!(got.is_empty() || got == ["m.py::Base.m"], "{got:?}");
}

#[test]
fn super_with_a_keyword_like_second_argument() {
    let got = super_case("super(Child, (self))");
    assert!(got.is_empty() || got == ["m.py::Base.m"], "{got:?}");
}

#[test]
fn super_enclosing_name_then_attribute_second_argument() {
    // `super(Child, self.owner)` is `Child` and an attribute read: 3 names.
    let got = super_case("super(Child, self.owner)");
    assert!(got.is_empty() || got == ["m.py::Base.m"], "{got:?}");
}

#[test]
fn super_enclosing_name_with_a_call_second_argument() {
    let got = super_case("super(Child, make())");
    assert!(got.is_empty() || got == ["m.py::Base.m"], "{got:?}");
}

#[test]
fn super_zero_arg_inside_a_nested_function_has_no_enclosing_class_cell() {
    // Zero-arg super in a nested def: `__class__` cell exists only for the
    // method itself; in the inner function `super()` has no first argument and
    // raises RuntimeError. It must not bind.
    let src = "class Base:\n    def m(self):\n        return 1\n\nclass Child(Base):\n    def m(self):\n        def inner():\n            return super().m()\n        return inner()\n";
    let got = calls(&resolve(&[("m.py", src)]), "m.py::Child.m.inner");
    assert!(got.is_empty() || got == ["m.py::Base.m"], "{got:?}");
}

#[test]
fn super_with_a_nested_super_in_the_arguments() {
    let src = "class Base:\n    def m(self):\n        return 1\n    def n(self, x):\n        return x\n\nclass Child(Base):\n    def m(self):\n        return super(Child, self).n(super(Child, self).m())\n";
    let got = calls(&resolve(&[("m.py", src)]), "m.py::Child.m");
    assert!(
        got.iter().all(|t| t == "m.py::Base.m" || t == "m.py::Base.n"),
        "{got:?}"
    );
}

#[test]
fn super_chain_call_on_a_super_result() {
    // `super(Child, self).a().m()`: `.m` is on the result of `a()`, not super.
    let src = "class Base:\n    def a(self):\n        return Other()\n\nclass Other:\n    def m(self):\n        return 1\n\nclass Child(Base):\n    def m(self):\n        return super(Child, self).a().m()\n";
    let got = calls(&resolve(&[("m.py", src)]), "m.py::Child.m");
    assert!(!has(&got, "m.py::Base.m"), "{got:?}");
}

// ------------------------------------------- 6. class declared twice / mixed

#[test]
fn class_redefined_without_the_method_binds_no_method() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\n\nclass A:\n    pass\n\nclass B(A):\n    def go(self):\n        return self.m()\n",
    )]);
    let got = calls(&r, "c.py::B.go");
    assert!(got.is_empty(), "CPython binds the second A, which has no m: {got:?}");
}

#[test]
fn class_redefined_binds_the_latest_declaration_or_nothing() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\n\nclass A:\n    def m(self):\n        return 2\n\nclass B(A):\n    def go(self):\n        return self.m()\n",
    )]);
    let got = calls(&r, "c.py::B.go");
    // Both declarations may share one symbol id; either nothing or one edge.
    assert!(got.len() <= 1, "{got:?}");
}

#[test]
fn class_redefined_typed_receiver_does_not_bind_the_first_declaration() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\n\nclass A:\n    pass\n\ndef f():\n    x = A()\n    return x.m()\n",
    )]);
    let got = calls(&r, "c.py::f");
    assert!(!has(&got, "c.py::A.m"), "the live A has no m: {got:?}");
}

#[test]
fn class_and_function_with_one_name_as_a_base() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\n\ndef A():\n    return object\n\nclass B(A):\n    def go(self):\n        return self.m()\n",
    )]);
    let got = calls(&r, "c.py::B.go");
    assert!(got.is_empty(), "A is the function at class-statement time: {got:?}");
}

#[test]
fn class_in_an_if_else_binds_neither() {
    let r = resolve(&[(
        "c.py",
        "import os\nif os.name == 'nt':\n    class A:\n        def m(self):\n            return 1\nelse:\n    class A:\n        def n(self):\n            return 2\n\nclass B(A):\n    def go(self):\n        return self.m()\n",
    )]);
    let got = calls(&r, "c.py::B.go");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn super_through_a_twice_declared_base() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\n\nclass A:\n    pass\n\nclass B(A):\n    def m(self):\n        return super().m()\n",
    )]);
    let got = calls(&r, "c.py::B.m");
    assert!(got.is_empty(), "{got:?}");
}

// ------------------------------------------ 7. flat collision vs single C3

#[test]
fn two_files_declare_base_and_a_dynamic_star_module_supplies_neither_name() {
    // `dyn` computes its exports, so the star import may supply `Base` from
    // anywhere. Two `Base` classes exist: neither may be chosen for `C`.
    let r = resolve(&[
        ("a.py", "class Base:\n    def m(self):\n        return 1\n"),
        ("b.py", "class Base:\n    def m(self):\n        return 2\n"),
        (
            "dyn.py",
            "from b import Base\n__all__ = [\"Base\"] + []\n",
        ),
        (
            "c.py",
            "from dyn import *\n\nclass C(Base):\n    def go(self):\n        return self.m()\n",
        ),
    ]);
    let got = calls(&r, "c.py::C.go");
    assert!(!has(&got, "a.py::Base.m"), "wrong file: {got:?}");
}

#[test]
fn single_declared_base_but_the_star_module_rebinds_it_dynamically() {
    // One `Base` in the corpus (a.py). `dyn` is dynamic and star-imported
    // after `from a import Base`: the name may now be anything.
    let r = resolve(&[
        ("a.py", "class Base:\n    def m(self):\n        return 1\n"),
        (
            "dyn.py",
            "import sys\n__all__ = []\n__all__.extend(['Base'])\nBase = type('Base', (), {})\n",
        ),
        (
            "c.py",
            "from a import Base\nfrom dyn import *\n\nclass C(Base):\n    def go(self):\n        return self.m()\n",
        ),
    ]);
    let got = calls(&r, "c.py::C.go");
    assert!(!has(&got, "a.py::Base.m"), "{got:?}");
}

#[test]
fn two_files_declare_base_and_c_imports_neither_but_has_its_own_star() {
    let r = resolve(&[
        ("a.py", "class Base:\n    def m(self):\n        return 1\n"),
        ("b.py", "class Base:\n    def m(self):\n        return 2\n"),
        ("c.py", "from nowhere import *\n\nclass C(Base):\n    def go(self):\n        return self.m()\n"),
    ]);
    let got = calls(&r, "c.py::C.go");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn same_class_name_two_files_super_zero_arg_no_import() {
    let r = resolve(&[
        ("a.py", "class Base:\n    def m(self):\n        return 1\n"),
        ("b.py", "class Base:\n    def m(self):\n        return 2\n"),
        ("c.py", "class C(Base):\n    def m(self):\n        return super().m()\n"),
    ]);
    let got = calls(&r, "c.py::C.m");
    assert!(got.is_empty(), "{got:?}");
}

// ---------------------------------------------------- 9. cap ledger row

fn chain80() -> String {
    let mut src = String::from("class C0:\n    def m(self):\n        return 1\n\n");
    for i in 1..80 {
        src.push_str(&format!("class C{i}(C{}):\n    pass\n\n", i - 1));
    }
    src.push_str("class Leaf(C79):\n    def m(self):\n        return super().m()\n\n    def go(self):\n        return self.m()\n\ndef typed():\n    x = Leaf()\n    return x.m()\n");
    src
}

fn rows_naming_the_cap(r: &ResolutionResult) -> Vec<String> {
    r.unresolved
        .iter()
        .filter(|row| format!("{:?}", row.resolution).contains("python heritage walk stopped"))
        .map(|row| format!("{}|{}|{:?}", row.source_symbol, row.callee_name, row.resolution))
        .collect()
}

#[test]
fn finding_super_call_on_a_capped_chain_records_the_cap() {
    let r = resolve(&[("c.py", &chain80())]);
    assert!(
        rows_naming_the_cap(&r).iter().any(|row| row.starts_with("c.py::Leaf.m|m|")),
        "super().m() abstains with no ledger row naming the cap: {:?}",
        rows_naming_the_cap(&r)
    );
}

#[test]
fn finding_typed_receiver_on_a_capped_chain_records_the_cap() {
    let r = resolve(&[("c.py", &chain80())]);
    assert!(
        rows_naming_the_cap(&r).iter().any(|row| row.starts_with("c.py::typed|m|")),
        "x.m() on a typed receiver abstains with no ledger row naming the cap: {:?}",
        rows_naming_the_cap(&r)
    );
}

#[test]
fn cap_row_text_is_stable_across_runs_and_chain_length() {
    let a = resolve(&[("c.py", &chain80())]);
    let b = resolve(&[("c.py", &chain80())]);
    assert_eq!(rows_naming_the_cap(&a), rows_naming_the_cap(&b));
    let mut src = String::from("class C0:\n    def m(self):\n        return 1\n\n");
    for i in 1..200 {
        src.push_str(&format!("class C{i}(C{}):\n    pass\n\n", i - 1));
    }
    src.push_str("class Leaf(C199):\n    def go(self):\n        return self.m()\n");
    let long = resolve(&[("c.py", &src)]);
    let a_row: Vec<_> = rows_naming_the_cap(&a)
        .into_iter()
        .filter(|row| row.starts_with("c.py::Leaf.go|m|"))
        .collect();
    let long_row = rows_naming_the_cap(&long);
    assert_eq!(a_row.len(), 1, "{a_row:?}");
    assert_eq!(long_row.len(), 1, "{long_row:?}");
    assert_eq!(
        a_row[0].split_once('|').unwrap().1,
        long_row[0].split_once('|').unwrap().1,
        "row text varies with chain length"
    );
}

#[test]
fn a_wide_hierarchy_past_the_node_cap_names_that_cap_not_the_depth() {
    let mut src = String::new();
    let mut names = Vec::new();
    for i in 0..600 {
        src.push_str(&format!("class P{i}:\n    pass\n\n"));
        names.push(format!("P{i}"));
    }
    src.push_str("class Z:\n    def m(self):\n        return 1\n\n");
    src.push_str(&format!(
        "class Leaf(Z, {}):\n    def go(self):\n        return self.m()\n",
        names.join(", ")
    ));
    let r = resolve(&[("c.py", &src)]);
    let rows = rows_naming_the_cap(&r);
    // CPython: Z.m. Any answer is correct only if it is Z.m; a silent
    // abstention is a finding, a depth-named row is wrong.
    assert!(
        !rows.iter().any(|row| row.contains("bases deep")),
        "{rows:?}"
    );
    let got = calls(&r, "c.py::Leaf.go");
    assert!(got.is_empty() || got == ["c.py::Z.m"], "{got:?}");
}
