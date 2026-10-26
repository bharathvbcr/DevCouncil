//! Round-two adversarial probes for the Python heritage / MRO / field
//! initializer lane (666a960e, f2ae630a). Each test states the correct
//! behaviour; a failing test is a BREAK of the new code.

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

fn methods(res: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = res
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == caller)
        .map(|e| e.target_symbol.clone())
        .filter(|t| t.rsplit("::").next().is_some_and(|tail| tail.contains('.')))
        .collect();
    out.sort();
    out
}

const BASE: &str = "class Base:\n    def m(self):\n        return 1\n";
const T: &str = "from typing import Generic, TypeVar\nT = TypeVar('T')\n";

fn super_in(body: &str) -> Vec<String> {
    let src = format!("{T}\n{BASE}\n{body}");
    let r = resolve(&[("c.py", src.as_str())]);
    methods(&r, "c.py::C.m")
}

const CM: &str = "    def m(self):\n        return super().m()\n";

// ------------------------------------------------------------- heritage shapes

#[test]
fn generic_before_in_repo_base_still_binds_base() {
    // Python MRO: C, Generic, Base?  Generic defines no `m`; the answer is Base.m.
    // Abstaining because `Generic` is an external import ahead of Base loses
    // an edge the base commit (flat table) found.
    let got = super_in(&format!("class C(Generic[T], Base):\n{CM}"));
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn generic_after_in_repo_base_binds_base() {
    let got = super_in(&format!("class C(Base, Generic[T]):\n{CM}"));
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn star_args_before_base_does_not_bind_base() {
    // `*mixins` is unknown classes that come BEFORE Base in the MRO.
    let got = super_in(&format!("mixins = ()\nclass C(*mixins, Base):\n{CM}"));
    assert!(got.is_empty(), "dropped *mixins, bound Base.m: {got:?}");
}

#[test]
fn call_base_before_base_does_not_bind_base() {
    let got = super_in(&format!(
        "def make_base():\n    return object\nclass C(make_base(), Base):\n{CM}"
    ));
    assert!(got.is_empty(), "dropped make_base(), bound Base.m: {got:?}");
}

#[test]
fn conditional_base_before_base_does_not_bind_base() {
    let got = super_in(&format!(
        "X = 1\nclass Other:\n    def m(self):\n        return 3\nclass C(Other if X else object, Base):\n{CM}"
    ));
    assert!(
        got.is_empty(),
        "dropped ternary base, bound Base.m: {got:?}"
    );
}

#[test]
fn parenthesized_base_is_a_base() {
    let got = super_in(&format!("class C((Base)):\n{CM}"));
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn double_star_kwargs_is_not_a_base() {
    let got = super_in(&format!("kw = {{}}\nclass C(Base, **kw):\n{CM}"));
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn double_star_only_has_no_base() {
    let got = super_in(&format!("kw = {{}}\nclass C(**kw):\n{CM}"));
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn named_tuple_base_binds_no_in_repo_edge() {
    let got = super_in(&format!(
        "import typing\nclass C(typing.NamedTuple):\n    x: int\n{CM}"
    ));
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn empty_parens_and_object_have_no_in_repo_base() {
    for head in ["class C:", "class C():", "class C(object):"] {
        let got = super_in(&format!("{head}\n{CM}"));
        assert!(got.is_empty(), "{head}: {got:?}");
    }
}

#[test]
fn decorated_class_binds_base() {
    let got = super_in(&format!(
        "import dataclasses\n@dataclasses.dataclass\nclass C(Base):\n{CM}"
    ));
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn object_before_mixin_abstains() {
    let got = super_in(&format!(
        "class Mixin:\n    def m(self):\n        return 2\nclass C(object, Mixin):\n{CM}"
    ));
    assert!(got.is_empty(), "{got:?}");
}

// --------------------------------------------------------------- dotted bases

#[test]
fn dotted_base_through_package_import_binds() {
    // `import pkg.mod; class C(pkg.mod.Base)`: the flat table bound this at
    // 4a5ca10c; the new walk calls any dotted member with a dot "opaque".
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/mod.py", BASE),
        (
            "c.py",
            "import pkg.mod\n\nclass C(pkg.mod.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["pkg/mod.py::Base.m"]);
}

#[test]
fn dotted_base_from_package_import_module_binds() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/mod.py", BASE),
        (
            "c.py",
            "from pkg import mod\n\nclass C(mod.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["pkg/mod.py::Base.m"]);
}

#[test]
fn dotted_base_subscripted_binds() {
    let r = resolve(&[
        ("one.py", BASE),
        (
            "c.py",
            "import one as o\n\nclass C(o.Base[int]):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["one.py::Base.m"]);
}

#[test]
fn dotted_base_four_segments_never_binds_wrong_namesake() {
    let r = resolve(&[
        ("a/b/c.py", BASE),
        ("z.py", "class Base:\n    def m(self):\n        return 9\n"),
        (
            "c.py",
            "import a.b.c\n\nclass C(a.b.c.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    let got = methods(&r, "c.py::C.m");
    assert!(!got.contains(&"z.py::Base.m".to_string()), "{got:?}");
}

#[test]
fn two_dotted_bases_sharing_a_root_are_two_bases() {
    // class C(a.X, a.Y): X declares nothing, Y declares m. MRO C, X, Y.
    let r = resolve(&[
        ("a/__init__.py", ""),
        ("a/x.py", "class X:\n    pass\n"),
        ("a/y.py", "class Y:\n    def m(self):\n        return 1\n"),
        (
            "c.py",
            "import a.x\nimport a.y\n\nclass C(a.x.X, a.y.Y):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["a/y.py::Y.m"]);
}

#[test]
fn two_module_alias_bases_sharing_a_root() {
    let r = resolve(&[
        (
            "one.py",
            "class X:\n    pass\nclass Y:\n    def m(self):\n        return 1\n",
        ),
        (
            "c.py",
            "import one as o\n\nclass C(o.X, o.Y):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["one.py::Y.m"]);
}

#[test]
fn multiline_dotted_base() {
    let r = resolve(&[
        ("one.py", BASE),
        (
            "c.py",
            "import one as o\n\nclass C(\n    o\n    .Base\n):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["one.py::Base.m"]);
}

#[test]
fn same_base_via_two_import_spellings_dedupes() {
    // class C(o.Base, p.Base) is a TypeError in Python, but
    // class C(Base, o.Base) where both are one.py::Base is too; use a chain:
    // D(B, C) with B(o.Base) and C(Base) both one.py::Base: diamond, binds.
    let r = resolve(&[
        ("one.py", BASE),
        (
            "c.py",
            "import one as o\nfrom one import Base\n\nclass B(o.Base):\n    pass\nclass K(Base):\n    pass\nclass C(B, K):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["one.py::Base.m"]);
}

// ------------------------------------------------------------------ C3 / caps

#[test]
fn c3_failure_abstains() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\nclass B:\n    def m(self):\n        return 2\nclass X(A, B):\n    pass\nclass Y(B, A):\n    pass\nclass C(X, Y):\n    def m(self):\n        return super().m()\n",
    )]);
    assert!(
        methods(&r, "c.py::C.m").is_empty(),
        "{:?}",
        methods(&r, "c.py::C.m")
    );
}

#[test]
fn wide_600_class_fanout_never_binds_wrong() {
    let mut src = String::from("class Base:\n    def m(self):\n        return 0\n");
    for i in 0..600 {
        src.push_str(&format!("class B{i}(Base):\n    pass\n"));
    }
    let names: Vec<String> = (0..600).map(|i| format!("B{i}")).collect();
    src.push_str(&format!(
        "class C({}):\n    def m(self):\n        return super().m()\n",
        names.join(", ")
    ));
    let r = resolve(&[("c.py", src.as_str())]);
    let got = methods(&r, "c.py::C.m");
    assert!(got.is_empty() || got == ["c.py::Base.m"], "{got:?}");
}

#[test]
fn chain_of_70_abstains_rather_than_partially_binding() {
    // Only the root declares m; every middle class overrides nothing. Past the
    // depth bound the answer is abstain, never a class partway up.
    let mut src = String::from("class K0:\n    def m(self):\n        return 0\n");
    for i in 1..70 {
        src.push_str(&format!("class K{i}(K{}):\n    pass\n", i - 1));
    }
    src.push_str("class C(K69):\n    def m(self):\n        return super().m()\n");
    let r = resolve(&[("c.py", src.as_str())]);
    let got = methods(&r, "c.py::C.m");
    assert!(got.is_empty() || got == ["c.py::K0.m"], "{got:?}");
}

#[test]
fn opaque_after_declaring_class_still_binds() {
    let got = super_in(&format!("class C(Base, dict):\n{CM}"));
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn opaque_before_declaring_class_abstains() {
    let got = super_in(&format!("class C(dict, Base):\n{CM}"));
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn inherited_self_call_through_opaque_later_base() {
    // `self.m()` in a method of C resolves through the same MRO.
    let src = format!("{BASE}\nclass C(Base, dict):\n    def go(self):\n        return self.m()\n");
    let r = resolve(&[("c.py", src.as_str())]);
    assert_eq!(methods(&r, "c.py::C.go"), ["c.py::Base.m"]);
}

// ------------------------------------------------------- module-member branch

#[test]
fn module_alias_rebound_later_does_not_bind_other_modules_class() {
    let r = resolve(&[
        ("one.py", BASE),
        ("two.py", "class Base:\n    def m(self):\n        return 2\n"),
        (
            "c.py",
            "import one as o\n\nclass C(o.Base):\n    def m(self):\n        return super().m()\n\nimport two as o\n",
        ),
    ]);
    let got = methods(&r, "c.py::C.m");
    assert!(!got.contains(&"two.py::Base.m".to_string()), "{got:?}");
}

#[test]
fn module_member_reexported_by_package_init() {
    let r = resolve(&[
        ("one/__init__.py", "from .base import Base\n"),
        ("one/base.py", BASE),
        (
            "c.py",
            "import one as o\n\nclass C(o.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["one/base.py::Base.m"]);
}

#[test]
fn module_member_missing_in_module_never_uses_flat_table() {
    let r = resolve(&[
        ("one.py", "X = 1\n"),
        (
            "two.py",
            "class Base:\n    def m(self):\n        return 2\n",
        ),
        (
            "c.py",
            "import one as o\n\nclass C(o.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert!(
        methods(&r, "c.py::C.m").is_empty(),
        "{:?}",
        methods(&r, "c.py::C.m")
    );
}

#[test]
fn module_alias_submodule_base() {
    let r = resolve(&[
        ("one/__init__.py", ""),
        ("one/sub.py", BASE),
        (
            "c.py",
            "import one as o\nimport one.sub\n\nclass C(o.sub.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["one/sub.py::Base.m"]);
}

// ----------------------------------------------------------- shadow poisoning

fn field_go(init: &str) -> Vec<String> {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let h = format!(
        "from a import A\n\nclass H:\n    def __init__(self):\n{init}\n    def go(self):\n        return self.x.run()\n"
    );
    let r = resolve(&[("a.py", a), ("h.py", h.as_str())]);
    methods(&r, "h.py::H.go")
}

#[test]
fn field_binds_baseline() {
    assert_eq!(field_go("        self.x = A()"), ["a.py::A.run"]);
}

#[test]
fn callee_rebound_after_assignment_is_not_the_import() {
    let got = field_go("        self.x = A()\n        A = object");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn global_declared_callee_still_the_import_or_nothing() {
    // `global A` makes the local name the module's A (the import): binding
    // a.py::A is right, binding nothing is acceptable, anything else is wrong.
    let got = field_go("        global A\n        self.x = A()");
    assert!(got.is_empty() || got == ["a.py::A.run"], "{got:?}");
}

#[test]
fn module_level_rebinding_of_callee_poisons() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let h = "from a import A\nA = None\n\nclass H:\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("h.py", h)]);
    // `A` is None by the time __init__ runs; the field is not an `a.A`.
    assert!(
        methods(&r, "h.py::H.go").is_empty(),
        "{:?}",
        methods(&r, "h.py::H.go")
    );
}

#[test]
fn nonlocal_callee_in_closure() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let h = "from a import A\n\nclass H:\n    def __init__(self):\n        A = object\n        def inner():\n            nonlocal A\n            self.x = A()\n        inner()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("h.py", h)]);
    assert!(
        methods(&r, "h.py::H.go").is_empty(),
        "{:?}",
        methods(&r, "h.py::H.go")
    );
}

#[test]
fn comprehension_variable_named_like_callee() {
    let got = field_go("        self.ys = [A for A in (int, str)]\n        self.x = A()");
    // The comprehension variable does not leak in py3: A is still the import.
    assert!(got.is_empty() || got == ["a.py::A.run"], "{got:?}");
}

#[test]
fn method_local_import_of_same_name_shadows() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let b = "class A:\n    def run(self):\n        return 2\n";
    let h = "from a import A\n\nclass H:\n    def __init__(self):\n        from b import A\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("b.py", b), ("h.py", h)]);
    let got = methods(&r, "h.py::H.go");
    assert!(
        !got.contains(&"a.py::A.run".to_string()),
        "bound the module import, not the local: {got:?}"
    );
}

#[test]
fn with_as_and_except_as_shadow() {
    let got = field_go("        with open('f') as A:\n            pass\n        self.x = A()");
    assert!(got.is_empty(), "{got:?}");
    let got = field_go("        try:\n            pass\n        except Exception as A:\n            pass\n        self.x = A()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn for_target_shadow() {
    let got = field_go("        for A in (1, 2):\n            pass\n        self.x = A()");
    assert!(got.is_empty(), "{got:?}");
}

// ---------------------------------------------------- self is an instance

fn field_in_method(def: &str) -> Vec<String> {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let h = format!(
        "from a import A\n\ndef deco(f):\n    return f\n\nclass H:\n{def}\n    def go(self):\n        return self.x.run()\n"
    );
    let r = resolve(&[("a.py", a), ("h.py", h.as_str())]);
    methods(&r, "h.py::H.go")
}

#[test]
fn custom_decorator_over_instance_method_binds() {
    let got = field_in_method("    @deco\n    def __init__(self):\n        self.x = A()");
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn custom_decorator_wrapping_staticmethod_is_not_an_instance() {
    // @deco over @staticmethod: still a static method; `self` is a plain arg.
    let got =
        field_in_method("    @deco\n    @staticmethod\n    def make(self):\n        self.x = A()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn staticmethod_with_attribute_spelling_is_not_an_instance() {
    let got = field_in_method(
        "    @__import__('builtins').staticmethod\n    def make(self):\n        self.x = A()",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn builtins_dotted_staticmethod_is_not_an_instance() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let h = "import builtins\nfrom a import A\n\nclass H:\n    @builtins.staticmethod\n    def make(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("h.py", h)]);
    assert!(
        methods(&r, "h.py::H.go").is_empty(),
        "{:?}",
        methods(&r, "h.py::H.go")
    );
}

#[test]
fn classmethod_with_comment_or_call_decorator() {
    let got =
        field_in_method("    @classmethod  # build\n    def make(self):\n        self.x = A()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn self_with_default_is_instance() {
    let got = field_in_method("    def __init__(self=None):\n        self.x = A()");
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn positional_only_self_is_instance() {
    let got = field_in_method("    def __init__(self, /, v=1):\n        self.x = A()");
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn typed_self_is_instance() {
    let got = field_in_method("    def __init__(self: 'H'):\n        self.x = A()");
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn async_method_is_instance() {
    let got = field_in_method("    async def start(self):\n        self.x = A()");
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn setter_is_instance() {
    let got = field_in_method(
        "    @property\n    def v(self):\n        return 1\n    @v.setter\n    def v(self, value):\n        self.x = A()",
    );
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn stacked_property_and_staticmethod_is_not_instance() {
    let got = field_in_method(
        "    @property\n    @staticmethod\n    def make(self):\n        self.x = A()",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn closure_in_method_with_self_default_capture() {
    // `lambda self=self: ...` rebinds `self` as its own parameter, but the
    // value is the method's self: either bind or abstain, never a wrong type.
    let got = field_in_method(
        "    def __init__(self):\n        def inner(self=self):\n            self.x = A()\n        inner()",
    );
    assert!(got.is_empty() || got == ["a.py::A.run"], "{got:?}");
}

// ------------------------------------------------ unowned-field poisoning

#[test]
fn module_level_function_writing_obj_x_does_not_poison_class_field() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let b = "class B:\n    def run(self):\n        return 2\n";
    let h = "from a import A\nfrom b import B\n\ndef patch(obj):\n    obj.x = B()\n\nclass H:\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("b.py", b), ("h.py", h)]);
    assert_eq!(methods(&r, "h.py::H.go"), ["a.py::A.run"]);
}

#[test]
fn module_level_function_with_self_param_does_not_poison_class_field() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let b = "class B:\n    def run(self):\n        return 2\n";
    let h = "from a import A\nfrom b import B\n\ndef patch(self):\n    self.x = B()\n\nclass H:\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("b.py", b), ("h.py", h)]);
    assert_eq!(methods(&r, "h.py::H.go"), ["a.py::A.run"]);
}

#[test]
fn nested_class_in_method_writing_same_field_does_not_type_outer() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let b = "class B:\n    def run(self):\n        return 2\n";
    let h = "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        class Inner:\n            def __init__(self):\n                self.x = B()\n        self.inner = Inner()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("b.py", b), ("h.py", h)]);
    let got = methods(&r, "h.py::H.go");
    assert!(
        !got.contains(&"b.py::B.run".to_string()),
        "Inner.x typed H.x: {got:?}"
    );
}

#[test]
fn nested_class_in_method_does_not_poison_outer_field() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let b = "class B:\n    def run(self):\n        return 2\n";
    let h = "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self.x = A()\n        class Inner:\n            def __init__(self):\n                self.x = B()\n        self.inner = Inner()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("b.py", b), ("h.py", h)]);
    assert_eq!(methods(&r, "h.py::H.go"), ["a.py::A.run"]);
}

#[test]
fn dotted_callee_field_binds_via_module_alias() {
    let r = resolve(&[
        ("a.py", "class A:\n    def run(self):\n        return 1\n"),
        (
            "h.py",
            "import a as m\n\nclass H:\n    def __init__(self):\n        self.x = m.A()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_eq!(methods(&r, "h.py::H.go"), ["a.py::A.run"]);
}
