//! Round-2 Python probes: `python_function_only_imports` and the M1 guard.
use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

type E = Vec<(String, String)>;

fn run(files: &[(&str, &str)], kind: EdgeKind) -> E {
    let extractions: Vec<_> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    for e in result.edges.iter().filter(|e| e.edge_kind == kind) {
        println!(
            "EDGE {} -> {} conf={:?}",
            e.source_symbol, e.target_symbol, e.confidence
        );
    }
    let mut v: E = result
        .edges
        .iter()
        .filter(|e| e.edge_kind == kind)
        .map(|e| (e.source_symbol.clone(), e.target_symbol.clone()))
        .collect();
    v.sort();
    v
}
fn calls(files: &[(&str, &str)]) -> E {
    run(files, EdgeKind::Calls)
}
fn refs(files: &[(&str, &str)]) -> E {
    run(files, EdgeKind::References)
}
fn has(e: &E, f: &str, t: &str) -> bool {
    e.iter().any(|(a, b)| a == f && b == t)
}

const LIB: (&str, &str) = ("lib.py", "def helper():\n    return 1\n\nVALUE = 3\n");
const LIB2: (&str, &str) = ("lib2.py", "def helper():\n    return 2\n\nVALUE = 4\n");
const H: &str = "lib.py::helper";
const H2: &str = "lib2.py::helper";

#[test]
fn py_module_and_function_import_both_bind() {
    let m = "import lib\n\ndef a():\n    import lib\n    return lib.helper()\n\ndef d():\n    return lib.helper()\n\nlib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::a", H), "{e:?}");
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
}

#[test]
fn py_type_checking_module_level_import_binds() {
    let m = "from typing import TYPE_CHECKING\n\nif TYPE_CHECKING:\n    import lib\n\ndef d():\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
}

#[test]
fn py_try_except_module_level_import_binds() {
    let m = "try:\n    import lib\nexcept ImportError:\n    lib = None\n\ndef d():\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
}

#[test]
fn py_try_except_two_alternatives_module_level_import() {
    let m = "try:\n    import lib as impl\nexcept ImportError:\n    import lib2 as impl\n\ndef d():\n    return impl.helper()\n";
    let e = calls(&[LIB, LIB2, ("mainx.py", m)]);
    println!("try/except alts: {e:?}");
}

#[test]
fn py_conditional_module_level_import_binds() {
    let m = "import sys\n\nif sys.version_info >= (3, 8):\n    import lib\n\ndef d():\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
}

#[test]
fn py_main_guard_module_level_import_binds() {
    let m = "def d():\n    return lib.helper()\n\nif __name__ == '__main__':\n    import lib\n    d()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    println!("main guard: {e:?}");
}

#[test]
fn py_with_and_for_module_level_import_binds() {
    let m = "for _ in range(1):\n    import lib\n\ndef d():\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
}

#[test]
fn py_class_body_import_read_in_class_body() {
    let m = "class C:\n    import lib\n    x = lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    println!("class body import, class body call: {e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_class_body_import_not_visible_in_method() {
    let m = "class C:\n    import lib\n    def m(self):\n        return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(
        !has(&e, "mainx.py::C.m", H),
        "methods do not see class scope: {e:?}"
    );
}

#[test]
fn py_init_reexport() {
    let init = ("pkg/__init__.py", "from .impl import helper\n");
    let imp = ("pkg/impl.py", "def helper():\n    return 1\n");
    let m = "from pkg import helper\n\ndef d():\n    return helper()\n";
    let e = calls(&[init, imp, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::d", "pkg/impl.py::helper"), "{e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_init_reexport_function_level_import() {
    let init = ("pkg/__init__.py", "from .impl import helper\n");
    let imp = ("pkg/impl.py", "def helper():\n    return 1\n");
    let m = "def a():\n    from pkg import helper\n    return helper()\n\ndef d():\n    return helper()\n";
    let e = calls(&[init, imp, ("mainx.py", m)]);
    // `a`'s own import binds only through the call ladder's function-local
    // alias rung (recall, recorded); `d` imports nothing and binds nothing.
    assert!(
        !has(&e, "mainx.py::d", "pkg/impl.py::helper"),
        "d does not import: {e:?}"
    );
}

#[test]
fn py_same_name_two_functions_different_modules() {
    let m = "def a():\n    import lib as m\n    return m.helper()\n\ndef b():\n    import lib2 as m\n    return m.helper()\n\ndef d():\n    return m.helper()\n";
    let e = calls(&[LIB, LIB2, ("mainx.py", m)]);
    // Function-local aliases are recall only for calls (not bound; extract-v98).
    assert!(
        !has(&e, "mainx.py::a", H2) && !has(&e, "mainx.py::b", H),
        "cross-wise: {e:?}"
    );
    assert!(
        !e.iter().any(|(f, _)| f == "mainx.py::d"),
        "d binds nothing: {e:?}"
    );
}

#[test]
fn py_same_name_two_functions_same_module() {
    let m = "def a():\n    import lib\n    return lib.helper()\n\ndef b():\n    import lib\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(!e.iter().any(|(_, t)| t != H), "{e:?}");
}

#[test]
fn py_closure_uses_parent_function_import() {
    let m = "def a():\n    import lib\n    def inner():\n        return lib.helper()\n    return inner()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::a.inner", H), "{e:?}");
}

#[test]
fn py_lambda_and_comprehension_in_importing_function() {
    let m = "def a():\n    import lib\n    f = lambda: lib.helper()\n    return [lib.helper() for _ in range(2)], f\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::a", H), "{e:?}");
}

#[test]
fn py_decorator_on_nested_def() {
    let deco = ("lib3.py", "def deco(f):\n    return f\n");
    let m = "def a():\n    import lib3\n    @lib3.deco\n    def inner():\n        return 1\n    return inner()\n";
    let e = calls(&[deco, ("mainx.py", m)]);
    let r = refs(&[deco, ("mainx.py", m)]);
    println!("deco calls={e:?} refs={r:?}");
    assert!(
        has(&e, "mainx.py::a", "lib3.py::deco")
            || has(&e, "mainx.py::a.inner", "lib3.py::deco")
            || has(&r, "mainx.py::a", "lib3.py::deco")
            || has(&r, "mainx.py::a.inner", "lib3.py::deco"),
        "decorator edge lost: calls={e:?} refs={r:?}"
    );
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_function_only_import_read_at_module_scope_is_unbound() {
    let m = "def a():\n    import lib\n    return lib.helper()\n\nlib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::a", H), "{e:?}");
    assert!(
        !e.iter().any(|(f, t)| f == "mainx.py" && t == H),
        "module scope unbound: {e:?}"
    );
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_value_reference_function_import_other_function() {
    let m = "def a():\n    import lib\n    return lib.VALUE\n\ndef d():\n    return lib.VALUE\n";
    let r = refs(&[LIB, ("mainx.py", m)]);
    assert!(has(&r, "mainx.py::a", "lib.py::VALUE"), "{r:?}");
    assert!(!has(&r, "mainx.py::d", "lib.py::VALUE"), "{r:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_from_import_value_function_only() {
    let m = "def a():\n    from lib import helper\n    return helper()\n\ndef d():\n    return helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(!has(&e, "mainx.py::d", H), "{e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_from_import_value_function_only_and_global_unique_name() {
    // d calls `helper` and no one else in the repo is named helper but lib's:
    // a bare call with no import in d must not bind.
    let m = "def a():\n    from lib import helper\n    return helper()\n\ndef d():\n    return helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(
        !e.iter().any(|(f, _)| f == "mainx.py::d"),
        "d bound something: {e:?}"
    );
}

#[test]
fn py_module_import_after_function_import_same_name() {
    let m = "def a():\n    import lib\n    return lib.helper()\n\nimport lib\n\ndef d():\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(
        has(&e, "mainx.py::a", H) && has(&e, "mainx.py::d", H),
        "{e:?}"
    );
}

#[test]
fn py_import_dotted_module_level_and_function_root() {
    let sub = ("pkg/sub.py", "def helper():\n    return 1\n");
    let init = ("pkg/__init__.py", "");
    let m = "import pkg.sub\n\ndef a():\n    import pkg\n    return pkg\n\ndef d():\n    return pkg.sub.helper()\n";
    let e = calls(&[init, sub, ("mainx.py", m)]);
    println!("dotted: {e:?}");
    assert!(
        has(&e, "mainx.py::d", "pkg/sub.py::helper"),
        "module-level `import pkg.sub` lost: {e:?}"
    );
}

#[test]
fn py_import_dotted_module_level_only() {
    let sub = ("pkg/sub.py", "def helper():\n    return 1\n");
    let init = ("pkg/__init__.py", "");
    let m = "import pkg.sub\n\ndef d():\n    return pkg.sub.helper()\n";
    let e = calls(&[init, sub, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::d", "pkg/sub.py::helper"), "{e:?}");
}

#[test]
fn py_import_dotted_function_and_other_function_plain_module_level() {
    let sub = ("pkg/sub.py", "def helper():\n    return 1\n");
    let init = ("pkg/__init__.py", "");
    let m = "import pkg.sub\n\ndef a():\n    import pkg.sub\n    return pkg.sub.helper()\n\ndef d():\n    return pkg.sub.helper()\n";
    let e = calls(&[init, sub, ("mainx.py", m)]);
    // `a`'s function-local `import pkg.sub` is not bound for calls (recall,
    // extract-v98: the extraction cannot show the name was not rebound); `d`
    // reads the module-level import.
    assert!(has(&e, "mainx.py::d", "pkg/sub.py::helper"), "{e:?}");
}

#[test]
fn py_aliased_module_level_alias_function_other() {
    let m = "import lib as L\n\ndef a():\n    import lib2 as L\n    return L.helper()\n\ndef d():\n    return L.helper()\n";
    let e = calls(&[LIB, LIB2, ("mainx.py", m)]);
    // `a`'s function-local alias is recall only (not bound for calls).
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
    assert!(!has(&e, "mainx.py::d", H2), "{e:?}");
    assert!(!has(&e, "mainx.py::a", H), "{e:?}");
}

#[test]
fn py_relative_import_module_level_and_function() {
    let m = (
        "pkg/mainx.py",
        "from . import lib\n\ndef d():\n    return lib.helper()\n",
    );
    let lib = ("pkg/lib.py", "def helper():\n    return 1\n");
    let init = ("pkg/__init__.py", "");
    let e = calls(&[init, lib, m]);
    assert!(has(&e, "pkg/mainx.py::d", "pkg/lib.py::helper"), "{e:?}");
}

#[test]
fn py_nested_class_method_inside_function_with_import() {
    let m = "def a():\n    import lib\n    class K:\n        def run(self):\n            return lib.helper()\n    return K\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    println!("class in func: {e:?}");
}

#[test]
fn py_default_argument_and_decorator_at_module_scope_with_function_only_import() {
    // decorator/default evaluated at module level: a function-only import must NOT bind.
    let m = "def a():\n    import lib\n    return lib.helper()\n\n@lib.helper\ndef d(x=lib.VALUE):\n    return x\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(
        !e.iter()
            .any(|(f, t)| (f == "mainx.py" || f == "mainx.py::d") && t == H),
        "{e:?}"
    );
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_async_def_and_method_importing() {
    let m = "class C:\n    def a(self):\n        import lib\n        return lib.helper()\n    def d(self):\n        return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::C.a", H), "{e:?}");
    assert!(!has(&e, "mainx.py::C.d", H), "{e:?}");
}

#[test]
fn py_from_imports_same_name_two_functions_different_modules() {
    let m = "def a():\n    from lib import helper\n    return helper()\n\ndef b():\n    from lib2 import helper\n    return helper()\n\ndef d():\n    return 0\n";
    let e = calls(&[LIB, LIB2, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::a", H), "{e:?}");
    assert!(has(&e, "mainx.py::b", H2), "{e:?}");
    assert!(
        !has(&e, "mainx.py::a", H2) && !has(&e, "mainx.py::b", H),
        "cross-wise: {e:?}"
    );
}

#[test]
fn py_from_import_function_and_module_level_other_function_same_name() {
    // module imports helper from lib; function a imports helper from lib2.
    let m = "from lib import helper\n\ndef a():\n    from lib2 import helper\n    return helper()\n\ndef d():\n    return helper()\n";
    let e = calls(&[LIB, LIB2, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::a", H2), "{e:?}");
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
    assert!(
        !has(&e, "mainx.py::a", H) && !has(&e, "mainx.py::d", H2),
        "{e:?}"
    );
}

#[test]
fn py_module_level_import_in_function_scoped_flat_table_ordering() {
    // function import appears BEFORE the module-level import in file order.
    let m = "def a():\n    from lib2 import helper\n    return helper()\n\nfrom lib import helper\n\ndef d():\n    return helper()\n";
    let e = calls(&[LIB, LIB2, ("mainx.py", m)]);
    assert!(
        has(&e, "mainx.py::a", H2) && has(&e, "mainx.py::d", H),
        "{e:?}"
    );
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_module_level_import_inside_nested_if_in_class_at_module() {
    // import inside `if` inside a *class body*: class-scoped, methods do not see it.
    let m = "import sys\nclass K:\n    if sys.platform == 'x':\n        import lib\n    def run(self):\n        return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(!has(&e, "mainx.py::K.run", H), "{e:?}");
}

#[test]
fn py_module_level_import_after_def_used_by_global_decl() {
    let m = "def d():\n    global lib\n    return lib.helper()\n\nimport lib\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::d", H), "{e:?}");
}

#[test]
fn py_function_import_used_via_global_by_other_function() {
    // a declares `global lib; import lib` => binds module-level name for d too.
    let m = "def a():\n    global lib\n    import lib\n\ndef d():\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    println!("global import: {e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn py_module_level_unresolved_import_then_function_import() {
    // module-level `import missing as lib` (unresolved); function a imports real lib.
    let m = "import missing as lib\n\ndef a():\n    import lib\n    return lib.helper()\n\ndef d():\n    return lib.helper()\n";
    let e = calls(&[LIB, ("mainx.py", m)]);
    assert!(has(&e, "mainx.py::a", H), "{e:?}");
    assert!(
        !has(&e, "mainx.py::d", H),
        "d's lib is the external one: {e:?}"
    );
}
