//! Round-three adversarial probes for the Python heritage follow-up
//! (import-bind sets, `python_name_is_rebound`, AST decorator classification,
//! the typing/abc structural skip, dotted bases). Each test states the correct
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
const CM: &str = "    def m(self):\n        return super().m()\n";

/// `class C(<head>)` in `c.py` next to `extra` files; the targets of
/// `c.py::C.m`.
fn super_with(extra: &[(&str, &str)], c_source: &str) -> Vec<String> {
    let mut files: Vec<(&str, &str)> = extra.to_vec();
    files.push(("c.py", c_source));
    let r = resolve(&files);
    let mut out: Vec<String> = Vec::new();
    for name in ["m", "most_common", "move_to_end", "appendleft", "send", "_dump_registry"] {
        out.extend(methods(&r, &format!("c.py::C.{name}")));
    }
    out.sort();
    out
}

// ----------------------------------------------------------- 3. import binds

#[test]
fn import_a_and_from_x_import_a_abstains() {
    let got = super_with(
        &[
            ("a/__init__.py", BASE),
            ("x.py", "class a:\n    Base = object\n"),
        ],
        &format!("import a\nfrom x import a\n\nclass C(a.Base):\n{CM}"),
    );
    assert!(got.is_empty(), "two bindings of `a`, bound one: {got:?}");
}

#[test]
fn import_a_b_and_import_a_c_still_binds_a_b_base() {
    let extra = [
        ("a/__init__.py", ""),
        ("a/b.py", BASE),
        ("a/c.py", "X = 1\n"),
    ];
    let got = super_with(
        &extra,
        &format!("import a.b\nimport a.c\n\nclass C(a.b.Base):\n{CM}"),
    );
    assert_eq!(got, ["a/b.py::Base.m"], "{got:?}");
    let got = super_with(
        &extra,
        &format!("import a.c\nimport a.b\n\nclass C(a.b.Base):\n{CM}"),
    );
    assert_eq!(got, ["a/b.py::Base.m"], "reverse order: {got:?}");
    let got = super_with(
        &extra,
        &format!("import a\nimport a.b\n\nclass C(a.b.Base):\n{CM}"),
    );
    assert_eq!(got, ["a/b.py::Base.m"], "import a + import a.b: {got:?}");
}

#[test]
fn try_import_except_assign_none_never_binds_wrong() {
    // `a` is the module or None; binding the module is the only non-wrong
    // answer, abstaining is acceptable.
    let got = super_with(
        &[("a.py", BASE), ("z.py", BASE)],
        &format!("try:\n    import a\nexcept ImportError:\n    a = None\n\nclass C(a.Base):\n{CM}"),
    );
    assert!(got.is_empty() || got == ["a.py::Base.m"], "{got:?}");
}

#[test]
fn type_checking_import_with_else_binding_abstains() {
    let got = super_with(
        &[("a.py", BASE)],
        &format!(
            "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from a import Base\nelse:\n    Base = object\n\nclass C(Base):\n{CM}"
        ),
    );
    assert!(got.is_empty(), "Base is two things: {got:?}");
}

#[test]
fn function_scope_import_of_same_module_does_not_abstain() {
    let got = super_with(
        &[("a.py", BASE)],
        &format!("import a\n\ndef f():\n    import a\n    return a\n\nclass C(a.Base):\n{CM}"),
    );
    assert_eq!(got, ["a.py::Base.m"], "{got:?}");
}

#[test]
fn function_scope_import_of_other_module_does_not_change_module_level_reading() {
    // The class statement runs at module level: `o` is `one`, whatever a
    // function imports under the same local name.
    let got = super_with(
        &[("one.py", BASE), ("two.py", BASE)],
        &format!(
            "import one as o\n\ndef f():\n    import two as o\n    return o\n\nclass C(o.Base):\n{CM}"
        ),
    );
    assert!(got.is_empty() || got == ["one.py::Base.m"], "{got:?}");
    assert_eq!(got, ["one.py::Base.m"], "recall loss vs module-level reading");
}

// ---------------------------------------------------- 4. name is rebound

fn field_go_files(extra: &[(&str, &str)], h: &str) -> Vec<String> {
    let mut files: Vec<(&str, &str)> = vec![(
        "a.py",
        "class A:\n    def run(self):\n        return 1\n",
    )];
    files.extend_from_slice(extra);
    files.push(("h.py", h));
    let r = resolve(&files);
    methods(&r, "h.py::H.go")
}

const H_TAIL: &str =
    "\nclass H:\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";

fn h_with(prefix: &str) -> Vec<String> {
    field_go_files(
        &[("b.py", "class A:\n    def run(self):\n        return 2\n\nclass B(A):\n    pass\n")],
        &format!("from a import A\n{prefix}{H_TAIL}"),
    )
}

#[test]
fn class_body_rebinding_of_callee_does_not_poison() {
    let got = field_go_files(
        &[],
        "from a import A\n\nclass H:\n    A = object\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n",
    );
    assert_eq!(got, ["a.py::A.run"], "class body is no closure scope: {got:?}");
}

#[test]
fn class_body_import_of_callee_does_not_poison_enclosed_method() {
    let got = field_go_files(
        &[("b.py", "class A:\n    def run(self):\n        return 2\n")],
        "from a import A\n\nclass H:\n    from b import A\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n",
    );
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn del_at_module_level_does_not_bind_a_run() {
    let got = h_with("del A\n");
    assert!(got.is_empty(), "A is unbound after del: {got:?}");
}

#[test]
fn type_alias_annotated_assignment_rebinds() {
    let got = h_with("from b import B\nA: type = B\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "{got:?}");
}

#[test]
fn star_import_after_named_import_rebinds() {
    let got = field_go_files(
        &[("b.py", "class A:\n    def run(self):\n        return 2\n")],
        &format!("from a import A\nfrom b import *\n{H_TAIL}"),
    );
    assert!(
        !got.contains(&"a.py::A.run".to_string()),
        "`from b import *` rebinds A, bound a.A: {got:?}"
    );
}

#[test]
fn star_import_before_named_import_keeps_the_named_one() {
    let got = field_go_files(
        &[("b.py", "class A:\n    def run(self):\n        return 2\n")],
        &format!("from b import *\nfrom a import A\n{H_TAIL}"),
    );
    assert!(got.is_empty() || got == ["a.py::A.run"], "{got:?}");
}

#[test]
fn walrus_at_module_level_rebinds() {
    let got = h_with("def make():\n    return object\nif (A := make()):\n    pass\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "{got:?}");
}

#[test]
fn module_level_for_and_with_target_rebind() {
    let got = h_with("for A in (1, 2):\n    pass\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "for: {got:?}");
    let got = h_with("with open('f') as A:\n    pass\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "with: {got:?}");
}

#[test]
fn conditional_module_level_assignment_rebinds() {
    let got = h_with("import os\nif os.environ.get('X'):\n    A = None\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "{got:?}");
}

#[test]
fn def_after_import_rebinds() {
    let got = h_with("def A():\n    return object()\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "{got:?}");
}

#[test]
fn global_assignment_in_another_function_rebinds() {
    let got = h_with("def reset():\n    global A\n    A = None\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "{got:?}");
}

#[test]
fn second_import_of_callee_under_same_name_rebinds() {
    let got = h_with("from b import A\n");
    assert!(!got.contains(&"a.py::A.run".to_string()), "{got:?}");
}

#[test]
fn enclosing_function_local_of_same_name_poisons_nested_class_method() {
    // A class inside a function: the method sees the function's local A.
    let got = field_go_files(
        &[],
        "from a import A\n\ndef build():\n    A = object\n    class H:\n        def __init__(self):\n            self.x = A()\n        def go(self):\n            return self.x.run()\n    return H\n",
    );
    assert!(got.is_empty(), "{got:?}");
}

// ----------------------------------------------------- 5. decorators

fn field_in_method(def: &str) -> Vec<String> {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let h = format!(
        "import abc\nimport functools\nfrom a import A\n\nsm = staticmethod\n\nclass H:\n{def}\n    def go(self):\n        return self.x.run()\n"
    );
    let r = resolve(&[("a.py", a), ("h.py", h.as_str())]);
    methods(&r, "h.py::H.go")
}

#[test]
fn call_decorator_staticmethod_parens_does_not_panic() {
    // `@staticmethod()` is a TypeError at runtime; only that it parses.
    let _ = field_in_method("    @staticmethod()\n    def make(self):\n        self.x = A()");
}

#[test]
fn parenthesized_staticmethod_decorator_is_not_an_instance() {
    let got = field_in_method("    @(staticmethod)\n    def make(self):\n        self.x = A()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn parenthesized_classmethod_attribute_decorator_is_not_an_instance() {
    let got = field_in_method(
        "    @(__import__('builtins').classmethod)\n    def make(self):\n        self.x = A()",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn aliased_staticmethod_decorator_is_not_an_instance() {
    let got = field_in_method("    @sm\n    def make(self):\n        self.x = A()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn abstractmethod_over_classmethod_is_not_an_instance() {
    let got = field_in_method(
        "    @abc.abstractmethod\n    @classmethod\n    def make(self):\n        self.x = A()",
    );
    assert!(got.is_empty(), "{got:?}");
    let got = field_in_method(
        "    @classmethod\n    @abc.abstractmethod\n    def make(self):\n        self.x = A()",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn functools_cache_over_instance_method_stays_an_instance() {
    let got = field_in_method("    @functools.cache\n    def start(self):\n        self.x = A()");
    assert_eq!(got, ["a.py::A.run"], "{got:?}");
}

#[test]
fn decorator_attribute_named_like_a_custom_object_stays_an_instance() {
    // `deco.staticmethod` is some other object's attribute: reading only the
    // last segment calls it the builtin.
    let a = "class A:\n    def run(self):\n        return 1\n";
    let h = "from a import A\nimport deco\n\nclass H:\n    @deco.staticmethod\n    def start(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";
    let r = resolve(&[("a.py", a), ("deco.py", "def staticmethod(f):\n    return f\n"), ("h.py", h)]);
    assert_eq!(methods(&r, "h.py::H.go"), ["a.py::A.run"]);
}

// ----------------------------------------------- 6. typing / abc skip

#[test]
fn local_typing_module_disables_the_skip() {
    let got = super_with(
        &[
            ("typing.py", "class Generic:\n    def m(self):\n        return 0\n"),
            ("base.py", BASE),
        ],
        &format!("from typing import Generic\nfrom base import Base\n\nclass C(Generic, Base):\n{CM}"),
    );
    assert_eq!(got, ["typing.py::Generic.m"], "{got:?}");
}

#[test]
fn local_abc_module_disables_the_skip() {
    let got = super_with(
        &[
            ("abc.py", "class ABC:\n    def m(self):\n        return 0\n"),
            ("base.py", BASE),
        ],
        &format!("import abc\nfrom base import Base\n\nclass C(abc.ABC, Base):\n{CM}"),
    );
    assert_eq!(got, ["abc.py::ABC.m"], "{got:?}");
}

#[test]
fn collections_abc_module_alias_is_not_the_abc_module() {
    // `collections.abc.Mapping` has concrete `get`/`keys`; it is not skipped.
    let got = super_with(
        &[],
        "from collections import abc\n\nclass Base:\n    def __contains__(self):\n        return 1\n\nclass C(abc.Mapping, Base):\n    def __contains__(self):\n        return super().__contains__()\n",
    );
    assert!(got.is_empty(), "collections.abc.Mapping.get precedes Base: {got:?}");
}

#[test]
fn typing_counter_alias_has_methods_and_precedes_base() {
    // `typing.Mapping` is `collections.abc.Mapping`: its mixin `get` is ahead
    // of Base in the MRO, so `super().__contains__()` is not Base.get.
    let got = super_with(
        &[],
        "from typing import Counter\n\nclass Base:\n    def most_common(self):\n        return 1\n\nclass C(Counter, Base):\n    def most_common(self):\n        return super().most_common()\n",
    );
    assert!(got.is_empty(), "skipped typing.Mapping, bound Base.get: {got:?}");
}

#[test]
fn typing_ordereddict_subscripted_has_methods_and_precedes_base() {
    // `typing.Dict` is `dict`: `update` is dict's, not Base's.
    let got = super_with(
        &[],
        "from typing import OrderedDict\n\nclass Base:\n    def move_to_end(self):\n        return 1\n\nclass C(OrderedDict[str, int], Base):\n    def move_to_end(self):\n        return super().move_to_end()\n",
    );
    assert!(got.is_empty(), "skipped typing.Dict, bound Base.update: {got:?}");
}

#[test]
fn typing_dot_ordereddict_dotted_has_methods_and_precedes_base() {
    let got = super_with(
        &[],
        "import typing\n\nclass Base:\n    def move_to_end(self):\n        return 1\n\nclass C(typing.OrderedDict, Base):\n    def move_to_end(self):\n        return super().move_to_end()\n",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn typing_deque_alias_has_methods_and_precedes_base() {
    let got = super_with(
        &[],
        "from typing import Deque\n\nclass Base:\n    def appendleft(self):\n        return 1\n\nclass C(Deque, Base):\n    def appendleft(self):\n        return super().appendleft()\n",
    );
    assert!(got.is_empty(), "TypedDict is a dict: {got:?}");
}

#[test]
fn abc_abcmeta_as_base_is_a_type_with_methods() {
    let got = super_with(
        &[],
        "import abc\n\nclass Base:\n    def _dump_registry(self):\n        return 1\n\nclass C(abc.ABCMeta, Base):\n    def _dump_registry(self):\n        return super()._dump_registry()\n",
    );
    assert!(got.is_empty(), "ABCMeta.register precedes Base: {got:?}");
}

#[test]
fn dotted_protocol_before_base_binds_base() {
    let got = super_with(
        &[],
        &format!("import typing\n{BASE}\nclass C(typing.Protocol, Base):\n{CM}"),
    );
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn subscripted_protocol_before_base_binds_base() {
    let got = super_with(
        &[],
        &format!(
            "from typing import Protocol, TypeVar\nT = TypeVar('T')\n{BASE}\nclass C(Protocol[T], Base):\n{CM}"
        ),
    );
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn aliased_generic_before_base_binds_base() {
    let got = super_with(
        &[],
        &format!("from typing import Generic as G, TypeVar\nT = TypeVar('T')\n{BASE}\nclass C(G[T], Base):\n{CM}"),
    );
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn typing_extensions_protocol_before_base_binds_base() {
    let got = super_with(
        &[],
        &format!("from typing_extensions import Protocol\n{BASE}\nclass C(Protocol, Base):\n{CM}"),
    );
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn abc_only_class_super_of_object_method_has_no_in_repo_edge() {
    let got = super_with(
        &[],
        "import abc\n\nclass Decoy:\n    def __repr__(self):\n        return 'd'\n\nclass C(abc.ABC):\n    def m(self):\n        return super().__repr__()\n",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn typing_import_shadowed_by_local_sibling_module_still_skips() {
    // `pkg/typing.py` exists; `from typing import Generic` in pkg/c.py is the
    // stdlib's (absolute import), so the skip must still apply.
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/typing.py", "X = 1\n"),
        (
            "pkg/c.py",
            &format!("from typing import Generic\n{BASE}\nclass C(Generic, Base):\n{CM}"),
        ),
    ]);
    assert_eq!(methods(&r, "pkg/c.py::C.m"), ["pkg/c.py::Base.m"]);
}

#[test]
fn skip_applies_per_name_not_per_module_when_name_also_rebound() {
    // `Generic = Base` after the import: not the typing marker any more.
    let got = super_with(
        &[],
        &format!(
            "from typing import Generic\n{BASE}\nclass Other:\n    def m(self):\n        return 2\nGeneric = Other\nclass C(Generic, Base):\n{CM}"
        ),
    );
    assert!(got.is_empty(), "{got:?}");
}

// ----------------------------------------------------- 7. dotted bases

#[test]
fn dotted_base_naming_a_function_abstains() {
    let got = super_with(
        &[("a.py", "def helper():\n    return object\n")],
        &format!("import a\n{BASE}\nclass C(a.helper, Base):\n{CM}"),
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn dotted_base_naming_a_variable_abstains() {
    let got = super_with(
        &[("a.py", "Alias = object\n")],
        &format!("import a\n{BASE}\nclass C(a.Alias, Base):\n{CM}"),
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn reexport_under_a_different_name_binds_the_real_class() {
    let got = super_with(
        &[
            ("pkg/__init__.py", "from .m import Base as Public\n"),
            ("pkg/m.py", BASE),
        ],
        &format!("import pkg\n\nclass C(pkg.Public):\n{CM}"),
    );
    assert_eq!(got, ["pkg/m.py::Base.m"], "{got:?}");
}

#[test]
fn from_import_of_reexport_under_a_different_name_binds_real_class() {
    let got = super_with(
        &[
            ("pkg/__init__.py", "from .m import Base as Public\n"),
            ("pkg/m.py", BASE),
        ],
        &format!("from pkg import Public\n\nclass C(Public):\n{CM}"),
    );
    assert_eq!(got, ["pkg/m.py::Base.m"], "{got:?}");
}

#[test]
fn circular_package_init_terminates_and_abstains() {
    let got = super_with(
        &[
            ("pkg/__init__.py", "from .a import Thing\n"),
            ("pkg/a.py", "from pkg import Thing\n"),
        ],
        &format!("import pkg\n\nclass C(pkg.Thing):\n{CM}"),
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn mutually_reexporting_packages_terminate() {
    let got = super_with(
        &[
            ("p/__init__.py", "from q import X\n"),
            ("q/__init__.py", "from p import X\n"),
        ],
        &format!("import p\n\nclass C(p.X):\n{CM}"),
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn relative_import_only_package_binds() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", BASE),
        (
            "pkg/child.py",
            &format!("from . import base\n\nclass C(base.Base):\n{CM}"),
        ),
    ]);
    assert_eq!(methods(&r, "pkg/child.py::C.m"), ["pkg/base.py::Base.m"]);
}

#[test]
fn parent_relative_import_module_binds() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", BASE),
        ("pkg/sub/__init__.py", ""),
        (
            "pkg/sub/child.py",
            &format!("from .. import base\n\nclass C(base.Base):\n{CM}"),
        ),
    ]);
    assert_eq!(methods(&r, "pkg/sub/child.py::C.m"), ["pkg/base.py::Base.m"]);
}

#[test]
fn relative_member_import_alias_binds() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", BASE),
        (
            "pkg/child.py",
            &format!("from .base import Base as B\n\nclass C(B):\n{CM}"),
        ),
    ]);
    assert_eq!(methods(&r, "pkg/child.py::C.m"), ["pkg/base.py::Base.m"]);
}

#[test]
fn nested_in_file_class_base_binds() {
    // `class C(Outer.Inner)`: Outer is an in-file class, not an import.
    let got = super_with(
        &[],
        &format!(
            "class Outer:\n    class Inner:\n        def m(self):\n            return 1\n\nclass C(Outer.Inner):\n{CM}"
        ),
    );
    assert!(got.is_empty() || got == ["c.py::Inner.m"], "{got:?}");
}

#[test]
fn super_inside_nested_class_method_binds_its_base() {
    // The nested class's methods are `c.py::C.m`, not `c.py::C.m`.
    let r = resolve(&[(
        "c.py",
        "class Base:\n    def m(self):\n        return 1\n\nclass Outer:\n    class C(Base):\n        def m(self):\n            return super().m()\n",
    )]);
    let nested = methods(&r, "c.py::C.m");
    assert_eq!(nested, ["c.py::Base.m"], "{nested:?}");
}

#[test]
fn import_at_module_level_inside_if_binds_once() {
    let got = super_with(
        &[("a.py", BASE)],
        &format!("import sys\nif sys.version_info >= (3,):\n    import a\n\nclass C(a.Base):\n{CM}"),
    );
    assert!(got.is_empty() || got == ["a.py::Base.m"], "{got:?}");
}

#[test]
fn dotted_base_with_comment_inside_parens_is_still_the_base() {
    let got = super_with(
        &[("a.py", BASE)],
        &format!("import a\n\nclass C(\n    a  # the module\n    .Base\n):\n{CM}"),
    );
    assert_eq!(got, ["a.py::Base.m"], "recall lost to the opaque marker: {got:?}");
}

#[test]
fn diag_typing_mapping_with_a_non_mapping_method_binds_base() {
    let got = super_with(
        &[],
        &format!("from typing import Mapping\n{BASE}\nclass C(Mapping, Base):\n{CM}"),
    );
    assert_eq!(got, ["c.py::Base.m"], "{got:?}");
}

#[test]
fn diag_unfiltered_name_control() {
    let got = super_with(
        &[],
        "class Base:\n    def most_common(self):\n        return 1\n\nclass C(Base):\n    def most_common(self):\n        return super().most_common()\n",
    );
    assert_eq!(got, ["c.py::Base.most_common"], "control: {got:?}");
}

#[test]
fn dotted_base_with_backslash_continuation_is_still_the_base() {
    let got = super_with(
        &[("a.py", BASE)],
        &format!("import a\n\nclass C(a \\\n    .Base):\n{CM}"),
    );
    assert_eq!(got, ["a.py::Base.m"], "recall lost to the opaque marker: {got:?}");
}
