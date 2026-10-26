//! Adversarial probes round 2 for `lane/py-residue` @ 1b42618a (delta
//! 63c32cd1..1b42618a). Each test asserts what CPython does; a failing test is
//! a BREAK. Nothing here fixes anything.

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

fn all_calls(res: &ResolutionResult) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = res
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls)
        .map(|e| (e.source_symbol.clone(), e.target_symbol.clone()))
        .collect();
    out.sort();
    out
}

const A: &str = "def helper():\n    return 1\n\nclass K:\n    def m(self):\n        return 1\n";
const B: &str = "def helper():\n    return 2\n";

#[test]
fn zz_dump_module_level_caller_name() {
    let r = resolve(&[("a.py", A), ("u.py", "from a import helper\nhelper()\n")]);
    eprintln!("DUMP {:?}", all_calls(&r));
}

// ------------------------------------------------ 1. conflicts, unknown scope

fn conflict_src(module_body: &str) -> Vec<(String, String)> {
    let src = format!(
        "from a import helper\n{module_body}\ndef g():\n    from b import helper\n    return helper()\n"
    );
    let r = resolve(&[("a.py", A), ("b.py", B), ("u.py", &src)]);
    all_calls(&r)
}

fn edge_from(edges: &[(String, String)], pred: impl Fn(&str) -> bool, target: &str) -> bool {
    edges.iter().any(|(s, t)| pred(s) && t == target)
}

#[test]
fn p1_module_level_call_with_function_conflict() {
    let e = conflict_src("helper()\n");
    assert!(edge_from(&e, |s| s == "u.py", "a.py::helper"), "{e:?}");
}

#[test]
fn p1_g_still_binds_b() {
    let e = conflict_src("helper()\n");
    assert!(edge_from(&e, |s| s == "u.py::g", "b.py::helper"), "{e:?}");
}

#[test]
fn p1_lambda_at_module_scope() {
    let e = conflict_src("f = lambda: helper()\n");
    assert!(
        e.iter().any(|(s, t)| s != "u.py::g" && t == "a.py::helper"),
        "{e:?}"
    );
}

#[test]
fn p1_class_body_call() {
    let e = conflict_src("class C:\n    x = helper()\n");
    assert!(edge_from(&e, |s| s != "u.py::g", "a.py::helper"), "{e:?}");
}

#[test]
fn p1_decorator_expression() {
    let e = conflict_src("@helper()\ndef h():\n    pass\n");
    assert!(edge_from(&e, |s| s != "u.py::g", "a.py::helper"), "{e:?}");
}

#[test]
fn p1_default_argument_expression() {
    let e = conflict_src("def h(x=helper()):\n    pass\n");
    assert!(edge_from(&e, |s| s != "u.py::g", "a.py::helper"), "{e:?}");
    assert!(!edge_from(&e, |s| s == "u.py::h", "b.py::helper"), "{e:?}");
}

#[test]
fn p1_default_argument_in_method() {
    let e = conflict_src("class C:\n    def m(self, x=helper()):\n        pass\n");
    assert!(edge_from(&e, |s| s != "u.py::g", "a.py::helper"), "{e:?}");
}

#[test]
fn p1_module_if_block() {
    let e = conflict_src("if True:\n    helper()\n");
    assert!(edge_from(&e, |s| s == "u.py", "a.py::helper"), "{e:?}");
}

#[test]
#[ignore = "pre-existing, ft-572bb69d"]
fn p1_module_attr_receiver_conflict() {
    let src = "import a as m\nm.helper()\ndef g():\n    import b as m\n    return m.helper()\n";
    let r = resolve(&[("a.py", A), ("b.py", B), ("u.py", src)]);
    let e = all_calls(&r);
    assert!(edge_from(&e, |s| s == "u.py", "a.py::helper"), "{e:?}");
    assert!(edge_from(&e, |s| s == "u.py::g", "b.py::helper"), "{e:?}");
}

#[test]
fn p1_class_base_conflict() {
    let a = "class Base:\n    def run(self):\n        return 1\n";
    let b = "class Base:\n    def run(self):\n        return 2\n";
    let src = "from a import Base\nclass C(Base):\n    def go(self):\n        return self.run()\ndef g():\n    from b import Base\n    class D(Base):\n        def go(self):\n            return self.run()\n";
    let r = resolve(&[("a.py", a), ("b.py", b), ("u.py", src)]);
    let e = all_calls(&r);
    assert!(
        edge_from(&e, |s| s == "u.py::C.go", "a.py::Base.run"),
        "{e:?}"
    );
}

// ------------------------------------------------------------ 2. __all__

fn star_binding(m_src: &str) -> Vec<String> {
    let r = resolve(&[
        ("a.py", "class Base:\n    def run(self):\n        return 1\n"),
        ("m.py", m_src),
        (
            "u.py",
            "from a import Base\nfrom m import *\n\nclass C(Base):\n    def go(self):\n        return self.run()\n",
        ),
    ]);
    calls(&r, "u.py::C.go")
}

fn keeps(m_src: &str) {
    let got = star_binding(m_src);
    assert!(
        has(&got, "a.py::Base.run"),
        "star import went dynamic: {got:?}\n{m_src}"
    );
}

#[test]
fn p2_baseline_literal_all() {
    keeps("__all__ = [\"x\"]\nx = 1\n");
}
#[test]
fn p2_docstring_mentions_all() {
    keeps("\"\"\"The __all__ list is below.\"\"\"\n__all__ = [\"x\"]\nx = 1\n");
}
#[test]
fn p2_docstring_quoted_all_string() {
    keeps("\"\"\"Set \"__all__\" to export.\"\"\"\n__all__ = [\"x\"]\nx = 1\n");
}
#[test]
fn p2_comment_mentions_quoted_all() {
    keeps("# \"__all__\" is the export list\n__all__ = [\"x\"]\nx = 1\n");
}
#[test]
fn p2_log_message() {
    keeps("import logging\nlog = logging.getLogger()\n__all__ = [\"x\"]\ndef x():\n    log.info(\"missing __all__ in plugin\")\n");
}
#[test]
fn p2_exact_string_log_message() {
    keeps("import logging\nlog = logging.getLogger()\n__all__ = [\"x\"]\ndef x():\n    log.info(\"__all__\")\n");
}
#[test]
fn p2_test_assertion_string() {
    keeps("__all__ = [\"x\"]\ndef x():\n    assert \"__all__\" in globals()\n");
}
#[test]
fn p2_hasattr() {
    keeps("import os\n__all__ = [\"x\"]\ndef x():\n    return hasattr(os, \"__all__\")\n");
}
#[test]
fn p2_other_all_in_function() {
    keeps("import os\n__all__ = [\"x\"]\ndef x():\n    return os.__all__\n");
}
#[test]
fn p2_other_all_method_in_function() {
    keeps("import os\n__all__ = [\"x\"]\ndef x():\n    return list(os.__all__)\n");
}
#[test]
fn p2_other_all_append_in_function() {
    keeps("import os\n__all__ = [\"x\"]\ndef x():\n    os.__all__.append(\"y\")\n");
}
#[test]
fn p2_other_module_all_copy_module_level() {
    keeps("import os\n__all__ = [\"x\"]\nn = os.__all__.copy()\n");
}
#[test]
fn p2_other_module_all_len() {
    keeps("import os\n__all__ = [\"x\"]\nn = len(os.__all__)\n");
}
#[test]
fn p2_identifier_containing_all() {
    keeps("__all__ = [\"x\"]\nmy__all__ = []\nmy__all__.append(1)\n");
}
#[test]
fn p2_all_read_is_not_mutation() {
    keeps("__all__ = [\"x\"]\nn = sorted(__all__)\ndef x():\n    return __all__.__len__()\n");
}
#[test]
fn p2_fstring_mentions_all() {
    keeps("__all__ = [\"x\"]\ndef x():\n    raise ValueError(f\"bad __all__: {1}\")\n");
}

// ------------------------------------------------------ 3. K.A() class attrs

#[test]
fn p3_nested_class_not_module_a() {
    let a = "class A:\n    def __init__(self):\n        pass\n";
    let src = "import a\nclass K:\n    class A:\n        def __init__(self):\n            pass\n\ndef f():\n    return K.A()\n";
    let r = resolve(&[("a.py", a), ("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(
        !got.iter().any(|t| t.starts_with("a.py")),
        "K.A() bound the module's A: {got:?}"
    );
}

#[test]
#[ignore = "pre-existing, ft-572bb69d"]
fn p3_nested_class_binds_inner() {
    let src = "class K:\n    class A:\n        def __init__(self):\n            pass\n\ndef f():\n    return K.A()\n";
    let r = resolve(&[("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(
        has(&got, "u.py::A.__init__") || has(&got, "u.py::A"),
        "{got:?}"
    );
}

#[test]
fn p3_nested_class_method_through_instance() {
    let src = "class K:\n    class A:\n        def run(self):\n            return 1\n\ndef f():\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(has(&got, "u.py::A.run"), "{got:?}");
}

#[test]
fn p3_class_attr_alias_wrong_module_a() {
    let a = "class A:\n    def run(self):\n        return 1\n";
    let c = "class SomeClass:\n    def run(self):\n        return 2\n";
    let src = "import a\nfrom c import SomeClass\nclass K:\n    A = SomeClass\n\ndef f():\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("a.py", a), ("c.py", c), ("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(
        !has(&got, "a.py::A.run"),
        "K.A() typed as module a's A: {got:?}"
    );
}

#[test]
#[ignore = "pre-existing, ft-572bb69d"]
fn p3_class_attr_alias_recall() {
    let c = "class SomeClass:\n    def run(self):\n        return 2\n";
    let src = "from c import SomeClass\nclass K:\n    A = SomeClass\n\ndef f():\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("c.py", c), ("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(has(&got, "c.py::SomeClass.run"), "{got:?}");
}

#[test]
fn p3_imported_k_does_not_bind_module_level_a() {
    let k = "class A:\n    def run(self):\n        return 0\nclass K:\n    class A:\n        def run(self):\n            return 1\n";
    let src = "from k import K\n\ndef f():\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("k.py", k), ("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(
        !has(&got, "k.py::A.run"),
        "imported K.A() bound the module-level A: {got:?}"
    );
}

#[test]
fn p3_imported_k_binds_nested() {
    let k = "class K:\n    class A:\n        def run(self):\n            return 1\n";
    let src = "from k import K\n\ndef f():\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("k.py", k), ("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(has(&got, "k.py::A.run"), "{got:?}");
}

#[test]
fn p3_module_alias_k_still_binds() {
    let m = "class A:\n    def run(self):\n        return 0\n";
    let src = "import m as K\n\ndef f():\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("m.py", m), ("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(has(&got, "m.py::A.run"), "{got:?}");
}

#[test]
fn p3_function_local_module_shadows_same_file_class() {
    let m = "class A:\n    def run(self):\n        return 0\n";
    let src =
        "class K:\n    pass\n\ndef g():\n    import m as K\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("m.py", m), ("u.py", src)]);
    let got = calls(&r, "u.py::g");
    assert!(
        has(&got, "m.py::A.run"),
        "function-local module K shadows the class: {got:?}"
    );
}

#[test]
fn p3_module_level_class_k_and_module_k_param() {
    let m = "class A:\n    def run(self):\n        return 0\n";
    let src = "import m\nclass m:\n    pass\n\ndef g():\n    x = m.A()\n    return x.run()\n";
    let r = resolve(&[("m.py", m), ("u.py", src)]);
    let got = calls(&r, "u.py::g");
    assert!(
        !has(&got, "m.py::A.run"),
        "class m rebinds the name: {got:?}"
    );
}

#[test]
fn p3_self_a_does_not_bind_module_a() {
    let src = "class A:\n    def run(self):\n        return 0\nclass K:\n    class A:\n        def run(self):\n            return 1\n    def m(self):\n        x = self.A()\n        return x.run()\n";
    let r = resolve(&[("u.py", src)]);
    let got = calls(&r, "u.py::K.m");
    assert!(
        !has(&got, "u.py::A.run"),
        "self.A() bound module A: {got:?}"
    );
}

#[test]
fn p3_self_a_binds_inner() {
    let src = "class K:\n    class A:\n        def run(self):\n            return 1\n    def m(self):\n        x = self.A()\n        return x.run()\n";
    let r = resolve(&[("u.py", src)]);
    let got = calls(&r, "u.py::K.m");
    assert!(has(&got, "u.py::A.run"), "{got:?}");
}

#[test]
fn p3_k_a_inherited_nested_class_from_imported_base() {
    let base = "class Base:\n    class A:\n        def run(self):\n            return 1\n";
    let a = "class A:\n    def run(self):\n        return 0\n";
    let src = "import a\nfrom base import Base\nclass K(Base):\n    pass\n\ndef f():\n    x = K.A()\n    return x.run()\n";
    let r = resolve(&[("a.py", a), ("base.py", base), ("u.py", src)]);
    let got = calls(&r, "u.py::f");
    assert!(!has(&got, "a.py::A.run"), "{got:?}");
}

// ------------------------------------------------------------ 4. scope walk

fn run2(src: &str) -> ResolutionResult {
    resolve(&[("a.py", A), ("b.py", B), ("u.py", src)])
}

#[test]
fn p4_comprehension_binding() {
    let r = run2("from a import helper\ndef g():\n    from b import helper\n    return [helper() for _ in range(2)]\n");
    let got = calls(&r, "u.py::g");
    assert!(
        has(&got, "b.py::helper") && !has(&got, "a.py::helper"),
        "{got:?}"
    );
}

#[test]
fn p4_comprehension_variable_shadows_import() {
    let r = run2("from a import helper\ndef g(fs):\n    return [helper() for helper in fs]\n");
    let got = calls(&r, "u.py::g");
    assert!(
        !has(&got, "a.py::helper"),
        "comprehension var shadows: {got:?}"
    );
}

#[test]
fn p4_generator_expression() {
    let r = run2("from a import helper\ndef g():\n    from b import helper\n    return sum(helper() for _ in range(2))\n");
    let got = calls(&r, "u.py::g");
    assert!(
        has(&got, "b.py::helper") && !has(&got, "a.py::helper"),
        "{got:?}"
    );
}

#[test]
fn p4_lambda_in_function_sees_function_import() {
    let r = run2("from a import helper\ndef g():\n    from b import helper\n    f = lambda: helper()\n    return f\n");
    let got = calls(&r, "u.py::g");
    assert!(!has(&got, "a.py::helper"), "{got:?}");
}

#[test]
fn p4_lambda_default_capturing() {
    let r = run2("from a import helper\ndef g():\n    from b import helper\n    f = lambda h=helper: h()\n    return f\n");
    let got = calls(&r, "u.py::g");
    assert!(!has(&got, "a.py::helper"), "{got:?}");
}

#[test]
fn p4_nested_class_in_function_in_class() {
    let r = run2("from a import helper\nclass Outer:\n    def f(self):\n        from b import helper\n        class Inner:\n            def m(self):\n                return helper()\n        return Inner\n");
    let e = all_calls(&r);
    assert!(
        !e.iter()
            .any(|(s, t)| s.contains("Inner") && t == "a.py::helper"),
        "{e:?}"
    );
    assert!(
        e.iter()
            .any(|(s, t)| s.contains("Inner") && t == "b.py::helper"),
        "{e:?}"
    );
}

#[test]
fn p4_class_body_name_not_visible_in_method() {
    let r = run2("from a import helper\nclass C:\n    from b import helper\n    def m(self):\n        return helper()\n");
    let got = calls(&r, "u.py::C.m");
    assert!(
        has(&got, "a.py::helper") && !has(&got, "b.py::helper"),
        "{got:?}"
    );
}

#[test]
fn p4_class_body_import_visible_in_class_body() {
    let r = run2("from a import helper\nclass C:\n    from b import helper\n    x = helper()\n");
    let e = all_calls(&r);
    assert!(
        !e.iter().any(|(_, t)| t == "a.py::helper"),
        "class body reads its own import: {e:?}"
    );
}

#[test]
fn p4_nonlocal_interplay() {
    let r = run2("from a import helper\ndef outer():\n    from b import helper\n    def inner():\n        nonlocal helper\n        return helper()\n    return inner\n");
    let got = calls(&r, "u.py::outer.inner");
    assert!(
        !has(&got, "a.py::helper"),
        "nonlocal reads outer's import: {got:?}"
    );
}

#[test]
fn p4_global_rebinds_module_name() {
    let r = run2("from a import helper\ndef g():\n    global helper\n    from b import helper\n    return helper()\n");
    let got = calls(&r, "u.py::g");
    assert!(
        has(&got, "b.py::helper") && !has(&got, "a.py::helper"),
        "{got:?}"
    );
}

#[test]
#[ignore = "pre-existing, ft-572bb69d"]
fn p4_global_declared_in_inner_reads_module() {
    let r = run2("from a import helper\ndef g():\n    from b import helper\n    def inner():\n        global helper\n        return helper()\n    return inner\n");
    let got = calls(&r, "u.py::g.inner");
    assert!(
        has(&got, "a.py::helper") && !has(&got, "b.py::helper"),
        "{got:?}"
    );
}

#[test]
fn p4_try_import_with_except_fallback() {
    let r = run2("from a import helper as fallback\ndef g():\n    try:\n        from b import helper\n    except ImportError:\n        helper = fallback\n    return helper()\n");
    let got = calls(&r, "u.py::g");
    assert!(
        !(has(&got, "b.py::helper") && !has(&got, "a.py::helper")),
        "only one branch bound: {got:?}"
    );
}

#[test]
fn p4_try_import_except_import() {
    let r = run2("def g():\n    try:\n        from b import helper\n    except ImportError:\n        from a import helper\n    return helper()\n");
    let got = calls(&r, "u.py::g");
    assert!(
        !(has(&got, "b.py::helper") && !has(&got, "a.py::helper")),
        "only first import bound: {got:?}"
    );
    assert!(
        !(has(&got, "a.py::helper") && !has(&got, "b.py::helper")),
        "only last import bound: {got:?}"
    );
}

#[test]
fn p4_parameter_shadows_module_import() {
    let r = run2("from a import helper\ndef g(helper):\n    return helper()\n");
    let got = calls(&r, "u.py::g");
    assert!(!has(&got, "a.py::helper"), "{got:?}");
}

#[test]
fn p4_sibling_functions_same_import() {
    let r = run2("def f():\n    from a import helper\n    return helper()\ndef g():\n    from a import helper\n    return helper()\n");
    assert!(has(&calls(&r, "u.py::f"), "a.py::helper"));
    assert!(has(&calls(&r, "u.py::g"), "a.py::helper"));
}

#[test]
fn p4_sibling_functions_different_imports() {
    let r = run2("def f():\n    from a import helper\n    return helper()\ndef g():\n    from b import helper\n    return helper()\n");
    assert!(
        has(&calls(&r, "u.py::f"), "a.py::helper"),
        "{:?}",
        all_calls(&r)
    );
    assert!(
        has(&calls(&r, "u.py::g"), "b.py::helper"),
        "{:?}",
        all_calls(&r)
    );
}

#[test]
fn p4_deep_enclosure_import_in_middle() {
    let r = run2("from a import helper\ndef f1():\n    from b import helper\n    def f2():\n        def f3():\n            return helper()\n        return f3\n    return f2\n");
    let got = calls(&r, "u.py::f1.f2.f3");
    assert!(
        has(&got, "b.py::helper") && !has(&got, "a.py::helper"),
        "{got:?}"
    );
}

#[test]
fn p4_method_in_class_in_function() {
    let r = run2("from a import helper\ndef f():\n    from b import helper\n    class C:\n        def m(self):\n            return helper()\n    return C\n");
    let e = all_calls(&r);
    assert!(
        e.iter()
            .any(|(s, t)| s.contains("C.m") && t == "b.py::helper"),
        "{e:?}"
    );
}

// ------------------------------------------------- 6. dotted import in func

#[test]
#[ignore = "pre-existing, ft-572bb69d"]
fn p6_dotted_import_in_function_a_b_f() {
    let src = "def g():\n    import a.b\n    return a.b.f()\n";
    let r = resolve(&[
        ("a/__init__.py", ""),
        ("a/b.py", "def f():\n    return 1\n"),
        ("u.py", src),
    ]);
    let got = calls(&r, "u.py::g");
    assert!(has(&got, "a/b.py::f"), "{got:?}");
}

#[test]
#[ignore = "pre-existing, ft-572bb69d"]
fn p6_dotted_import_module_scope_baseline() {
    let src = "import a.b\ndef g():\n    return a.b.f()\n";
    let r = resolve(&[
        ("a/__init__.py", ""),
        ("a/b.py", "def f():\n    return 1\n"),
        ("u.py", src),
    ]);
    let got = calls(&r, "u.py::g");
    assert!(has(&got, "a/b.py::f"), "{got:?}");
}

#[test]
fn p6_module_import_a_then_function_dotted() {
    let src = "import a\ndef g():\n    import a.b\n    return a.x()\n";
    let r = resolve(&[
        ("a/__init__.py", "def x():\n    return 1\n"),
        ("a/b.py", "def f():\n    return 1\n"),
        ("u.py", src),
    ]);
    let got = calls(&r, "u.py::g");
    assert!(has(&got, "a/__init__.py::x"), "a.x() abstained: {got:?}");
}

#[test]
fn p6_dotted_in_function_a_x() {
    let src = "def g():\n    import a.b\n    return a.x()\n";
    let r = resolve(&[
        ("a/__init__.py", "def x():\n    return 1\n"),
        ("a/b.py", "def f():\n    return 1\n"),
        ("u.py", src),
    ]);
    let got = calls(&r, "u.py::g");
    assert!(
        has(&got, "a/__init__.py::x"),
        "a.x() after dotted import: {got:?}"
    );
}

#[test]
fn p6_other_function_unaffected() {
    let src = "import a\ndef g():\n    import a.b\n    return 1\ndef h():\n    return a.x()\n";
    let r = resolve(&[
        ("a/__init__.py", "def x():\n    return 1\n"),
        ("a/b.py", "def f():\n    return 1\n"),
        ("u.py", src),
    ]);
    let got = calls(&r, "u.py::h");
    assert!(
        has(&got, "a/__init__.py::x"),
        "h's a.x() abstained: {got:?}"
    );
}

#[test]
fn p6_module_level_call_unaffected() {
    let src = "import a\na.x()\ndef g():\n    import a.b\n    return 1\n";
    let r = resolve(&[
        ("a/__init__.py", "def x():\n    return 1\n"),
        ("a/b.py", "def f():\n    return 1\n"),
        ("u.py", src),
    ]);
    let e = all_calls(&r);
    assert!(edge_from(&e, |s| s == "u.py", "a/__init__.py::x"), "{e:?}");
}

// ------------------------------------------------------------ 5. performance

fn wide_extractions(n: usize) -> Vec<Extraction> {
    let mut files: Vec<(String, String)> = Vec::new();
    for m in 0..10 {
        let mut s = String::new();
        for k in 0..10 {
            s.push_str(&format!("def n{m}_{k}():\n    return {k}\n"));
        }
        files.push((format!("m{m}.py"), s));
    }
    let mut big = String::new();
    for i in 0..n {
        let m = i % 10;
        let names: Vec<String> = (0..10).map(|k| format!("n{m}_{k}")).collect();
        big.push_str(&format!(
            "def f{i}():\n    from m{m} import {}\n    return 1\n",
            names.join(", ")
        ));
    }
    files.push(("big.py".to_string(), big));
    files.iter().map(|(p, s)| extract_file(p, s)).collect()
}

fn index_time(n: usize) -> std::time::Duration {
    let ex = wide_extractions(n);
    let mut best = std::time::Duration::MAX;
    for _ in 0..3 {
        let mut resolver = Resolver::new();
        let t = std::time::Instant::now();
        resolver.index_extractions(&ex);
        best = best.min(t.elapsed());
    }
    best
}

#[test]
fn p5_index_is_linear_in_function_count() {
    let small = index_time(1000);
    let large = index_time(4000);
    // 4x the functions: linear is ~4x, quadratic ~16x.
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-6);
    assert!(
        ratio < 8.0,
        "index_extractions 1000 fns {small:?}, 4000 fns {large:?}, ratio {ratio:.1}"
    );
}

fn chain_src(depth: usize) -> String {
    let mut s = String::from("def top():\n    from m0 import helper\n");
    for d in 1..=depth {
        s.push_str(&format!("{}def f{d}():\n", "    ".repeat(d)));
    }
    s.push_str(&format!("{}return helper()\n", "    ".repeat(depth + 1)));
    s
}

#[test]
fn p5_deep_chain_50_binds() {
    let r = resolve(&[
        ("m0.py", "def helper():\n    return 1\n"),
        ("u.py", &chain_src(50)),
    ]);
    let e = all_calls(&r);
    assert!(
        e.iter().any(|(_, t)| t == "m0.py::helper"),
        "50-deep chain lost the import: {e:?}"
    );
}

#[test]
fn p5_deep_chain_70_binds() {
    let r = resolve(&[
        ("m0.py", "def helper():\n    return 1\n"),
        ("u.py", &chain_src(70)),
    ]);
    let e = all_calls(&r);
    assert!(
        e.iter().any(|(_, t)| t == "m0.py::helper"),
        "70-deep chain lost the import: {e:?}"
    );
}
