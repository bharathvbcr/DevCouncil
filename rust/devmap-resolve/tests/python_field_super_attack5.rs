//! Round-five (closing) adversarial probes for `lane/py-attack3-fixes` @ 5de18fd1
//! (delta bf936983..5de18fd1): the Python self-call MRO veto, the per-file fact
//! table, star import with `__all__`, `python_name_is_ambiguous`, torch through
//! imports and the iterative dotted-base check. Each test states the CPython
//! behaviour; a failing test is a BREAK of the delta.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction, WiringKind};
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

/// Every edge (any kind) out of `caller`, as its target symbol.
fn targets(res: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = res
        .edges
        .iter()
        .filter(|e| e.source_symbol == caller)
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out
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

fn hits(list: &[String], needle: &str) -> bool {
    list.iter().any(|t| t.ends_with(needle))
}

// ------------------------------------------------------------ 1. MRO veto

/// BFS from `D` finds `C.m` (depth 1); C3 is D, B, A, C, Z so CPython runs
/// `A.m`. The veto must abstain: a `C.m` edge is a wrong edge.
const DISAGREE_HEAD: &str = "class A:\n    def m(self):\n        return 1\n\nclass Z:\n    pass\n\nclass B(A):\n    pass\n\nclass C(Z):\n    def m(self):\n        return 2\n\nclass D(B, C):\n";

fn disagree(body: &str) -> String {
    format!("{DISAGREE_HEAD}{body}")
}

#[test]
fn control_self_call_single_chain_resolves() {
    let src = "class A:\n    def m(self):\n        return 1\n\nclass B(A):\n    def go(self):\n        return self.m()\n";
    let r = resolve(&[("c.py", src)]);
    assert!(hits(&calls(&r, "c.py::B.go"), "A.m"), "{:?}", calls(&r, "c.py::B.go"));
}

#[test]
fn self_call_bfs_and_c3_disagree_c3_picks_other_class() {
    let src = disagree("    def go(self):\n        return self.m()\n");
    let r = resolve(&[("c.py", &src)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "C.m"), "BFS answer kept where C3 says A.m: {t:?}");
}

#[test]
fn classmethod_cls_call_bfs_c3_disagree() {
    let src = disagree("    @classmethod\n    def go(cls):\n        return cls.m()\n");
    let r = resolve(&[("c.py", &src)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "C.m"), "{t:?}");
}

#[test]
fn type_self_call_bfs_c3_disagree() {
    let src = disagree("    def go(self):\n        return type(self).m(self)\n");
    let r = resolve(&[("c.py", &src)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "C.m"), "{t:?}");
}

#[test]
fn nested_function_self_call_bfs_c3_disagree() {
    let src = disagree("    def go(self):\n        def inner():\n            return self.m()\n        return inner()\n");
    let r = resolve(&[("c.py", &src)]);
    for caller in ["c.py::D.go", "c.py::D.go.inner", "c.py::inner"] {
        let t = targets(&r, caller);
        assert!(!hits(&t, "C.m"), "{caller}: {t:?}");
    }
}

#[test]
fn lambda_self_call_bfs_c3_disagree() {
    let src = disagree("    def go(self):\n        f = lambda: self.m()\n        return f()\n");
    let r = resolve(&[("c.py", &src)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "C.m"), "{t:?}");
}

#[test]
fn self_method_reference_not_call_bfs_c3_disagree() {
    let src = disagree("    def go(self, xs):\n        return list(map(self.m, xs))\n");
    let r = resolve(&[("c.py", &src)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "C.m"), "reference edge uses BFS answer: {t:?}");
}

#[test]
fn self_method_bare_attribute_bfs_c3_disagree() {
    let src = disagree("    def go(self):\n        handler = self.m\n        return handler\n");
    let r = resolve(&[("c.py", &src)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "C.m"), "{t:?}");
}

#[test]
fn disagreement_inside_a_function_scoped_class() {
    let src = "class A:\n    def m(self):\n        return 1\n\nclass Z:\n    pass\n\nclass C(Z):\n    def m(self):\n        return 2\n\ndef build():\n    class B(A):\n        pass\n    class D(B, C):\n        def go(self):\n            return self.m()\n    return D\n";
    let r = resolve(&[("c.py", src)]);
    let all: Vec<String> = r.edges.iter().map(|e| format!("{} -> {}", e.source_symbol, e.target_symbol)).collect();
    assert!(
        !all.iter().any(|e| e.contains("go") && e.ends_with("C.m")),
        "{all:?}"
    );
}

#[test]
fn disagreement_inside_a_class_nested_class() {
    let src = "class A:\n    def m(self):\n        return 1\n\nclass Z:\n    pass\n\nclass C(Z):\n    def m(self):\n        return 2\n\nclass Outer:\n    class B(A):\n        pass\n    class D(B, C):\n        def go(self):\n            return self.m()\n";
    let r = resolve(&[("c.py", src)]);
    let all: Vec<String> = r.edges.iter().map(|e| format!("{} -> {}", e.source_symbol, e.target_symbol)).collect();
    assert!(
        !all.iter().any(|e| e.contains("go") && e.ends_with("C.m")),
        "{all:?}"
    );
}

/// `C` reaches `D` through a star import: the file cannot bind it, so the MRO
/// walk is `NotScoped` and the veto is skipped; the BFS answer stands.
#[test]
fn disagreement_with_a_star_imported_base_is_not_scoped() {
    let a = "class A:\n    def m(self):\n        return 1\n\nclass Z:\n    pass\n\nclass C(Z):\n    def m(self):\n        return 2\n";
    let c = "from a import A\nfrom a import *\n\nclass B(A):\n    pass\n\nclass D(B, C):\n    def go(self):\n        return self.m()\n";
    let r = resolve(&[("a.py", a), ("c.py", c)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "C.m"), "{t:?}");
}

#[test]
fn opaque_base_ahead_of_declaring_class_vetoes() {
    let src = "import ext\n\nclass A:\n    def m(self):\n        return 1\n\nclass D(ext.Thing, A):\n    def go(self):\n        return self.m()\n";
    let r = resolve(&[("c.py", src)]);
    let t = targets(&r, "c.py::D.go");
    assert!(!hits(&t, "A.m"), "{t:?}");
}

// ------------------------------------------------------------ 2. file facts

fn field_recorded(source: &str) -> bool {
    extract_file("h.py", source)
        .references
        .iter()
        .any(|r| r.assigned_to.as_deref() == Some("self.x"))
}

fn static_class(prefix: &str) -> String {
    format!("{prefix}class H:\n    @staticmethod\n    def make(self):\n        self.x = A()\n")
}

#[test]
fn multibyte_text_before_decorator_keeps_builtin_reading() {
    // Rebinding AFTER the decorator, with multi-byte text in front of both.
    let src = format!(
        "# ünïcödé ✓ 日本語 🚀🚀🚀\n{}\nstaticmethod = lambda f: f\n",
        static_class("")
    );
    assert!(!field_recorded(&src), "a later rebinding leaked backwards");
}

#[test]
fn multibyte_text_before_rebinding_makes_decorator_not_builtin() {
    let src = static_class("# 日本語 ✓ ü 🚀🚀🚀🚀\nstaticmethod = lambda f: f\n");
    assert!(field_recorded(&src), "an earlier rebinding was missed");
}

#[test]
fn multibyte_on_the_same_line_before_the_rebinding() {
    let src = static_class("x = 'üüü日本語'; staticmethod = lambda f: f\n");
    assert!(field_recorded(&src));
}

#[test]
fn decorator_function_rebinds_its_own_name_after_use() {
    // `@staticmethod def staticmethod` : the decorator is evaluated before the
    // def binds, then H's decorator reads the rebound name.
    let src = format!(
        "@staticmethod\ndef staticmethod(f):\n    return f\n\n{}",
        static_class("")
    );
    assert!(field_recorded(&src), "second decorator reads the rebound name");
    let first = "class H:\n    @staticmethod\n    def make(self):\n        self.x = A()\n\n@staticmethod\ndef staticmethod(f):\n    return f\n";
    assert!(!field_recorded(first), "class body ran before the rebinding");
}

#[test]
fn if_guarded_rebinding_before_decorator() {
    let src = static_class("import sys\nif sys.platform == 'win32':\n    staticmethod = lambda f: f\n");
    assert!(field_recorded(&src), "a guarded rebinding may have run");
}

#[test]
fn try_except_importerror_rebinding() {
    let src = static_class(
        "try:\n    from compat import staticmethod\nexcept ImportError:\n    pass\n",
    );
    assert!(field_recorded(&src), "an import may have rebound staticmethod");
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97), ft-572bb69d"]
fn rebinding_in_except_importerror_branch_for_a_torch_name() {
    let src = "try:\n    import torch.nn as nn\nexcept ImportError:\n    nn = None\n\nclass M(nn.Module):\n    def forward(self, x):\n        return x\n";
    // A runtime name that is `None` on one path is not torch's; the exemption
    // should not apply (low: only suppresses a dead-code report).
    let ext = extract_file("m.py", src);
    let wired = ext.wiring.iter().any(|w| w.kind == WiringKind::RuntimeEntryPoint && w.target_symbol.contains("forward"));
    assert!(!wired, "nn rebound to None on the except path still exempted");
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97), ft-572bb69d"]
fn function_body_decorator_runs_after_later_module_rebinding() {
    // The class statement inside `build` runs when build() is called, after the
    // module-level rebinding below it. Textual order is not execution order.
    let src = "def build():\n    class H:\n        @staticmethod\n        def make(self):\n            self.x = A()\n    return H\n\nstaticmethod = lambda f: f\nH = build()\n";
    assert!(field_recorded(src), "decorator inside a function reads the rebound name");
}

// ------------------------------------------------------------ 3. star import

const BASE: &str = "class Base:\n    def run(self):\n        return 1\n";
const USER: &str = "from base import Base\nfrom m import *\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";

/// `m` can bind `Base`, so the star import may rebind it: no edge to
/// `base.py::Base.run`. True when the edge is absent.
fn star_abstains(m: &str) -> Result<(), Vec<String>> {
    let r = resolve(&[("base.py", BASE), ("m.py", m), ("c.py", USER)]);
    let t = targets(&r, "c.py::C.go");
    if hits(&t, "base.py::Base.run") {
        Err(t)
    } else {
        Ok(())
    }
}

#[test]
fn control_star_from_unrelated_module_keeps_the_edge() {
    let r = resolve(&[("base.py", BASE), ("m.py", "def other():\n    return 1\n"), ("c.py", USER)]);
    assert!(hits(&targets(&r, "c.py::C.go"), "base.py::Base.run"));
}

#[test]
fn control_star_from_module_defining_base_abstains() {
    star_abstains("class Base:\n    pass\n").unwrap();
}

#[test]
fn all_built_by_plus_equals_literal() {
    star_abstains("class Base:\n    pass\n__all__ = []\n__all__ += ['Base']\n").unwrap();
}

#[test]
fn all_extended_dynamically() {
    star_abstains("Base = object\n__all__ = []\n__all__.extend(['Base'])\n").unwrap();
}

#[test]
fn all_extended_by_a_call_after_a_literal_prefix() {
    // `__all__` is a literal prefix plus whatever `helper()` returns.
    let m = "from other import helper\n__all__ = ['x']\n__all__ += helper()\nx = 1\nclass Base:\n    pass\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_extend_call_after_a_literal_prefix_name_from_elsewhere() {
    // Base is bound by a star of its own and exported only dynamically.
    let m = "from other import *\n__all__ = ['x']\n__all__.extend(other_names())\nx = 1\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_is_base_all_plus_literal() {
    let other = "class Base:\n    pass\n__all__ = ['Base']\n";
    let m = "import other\nfrom other import *\n__all__ = other.__all__ + ['x']\nx = 1\n";
    let r = resolve(&[("base.py", BASE), ("other.py", other), ("m.py", m), ("c.py", USER)]);
    let t = targets(&r, "c.py::C.go");
    assert!(!hits(&t, "base.py::Base.run"), "m re-exports other.Base through __all__: {t:?}");
}

#[test]
fn all_is_literal_plus_base_all() {
    let other = "class Base:\n    pass\n__all__ = ['Base']\n";
    let m = "import other\nfrom other import *\n__all__ = ['x'] + other.__all__\nx = 1\n";
    let r = resolve(&[("base.py", BASE), ("other.py", other), ("m.py", m), ("c.py", USER)]);
    let t = targets(&r, "c.py::C.go");
    assert!(!hits(&t, "base.py::Base.run"), "{t:?}");
}

#[test]
fn all_is_a_tuple() {
    star_abstains("class Base:\n    pass\n__all__ = ('Base',)\n").unwrap();
}

#[test]
fn all_tuple_without_base_keeps_the_edge() {
    let m = "class Base:\n    pass\nx = 1\n__all__ = ('x',)\n";
    let r = resolve(&[("base.py", BASE), ("m.py", m), ("c.py", USER)]);
    assert!(hits(&targets(&r, "c.py::C.go"), "base.py::Base.run"));
}

#[test]
fn all_names_something_the_module_does_not_define() {
    // Conservative direction: `Base` listed, never defined (AttributeError at
    // import in CPython). Either answer is defensible; it must not panic.
    let _ = star_abstains("__all__ = ['Base']\n");
}

#[test]
fn all_annotated_assignment() {
    star_abstains("class Base:\n    pass\n__all__: list[str] = ['Base']\n").unwrap();
}

#[test]
fn all_assigned_from_a_list_comprehension() {
    star_abstains("class Base:\n    pass\n__all__ = [n for n in dir() if n[0].isupper()]\n").unwrap();
}

#[test]
fn all_in_a_docstring_or_comment_is_not_all() {
    let m = "class Base:\n    pass\n\"\"\"\n__all__ = ['nothing']\n\"\"\"\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_assigned_inside_a_function_is_not_module_all() {
    let m = "class Base:\n    pass\ndef f():\n    __all__ = ['nothing']\n    return __all__\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_assigned_in_a_class_body_is_not_module_all() {
    let m = "class Base:\n    pass\nclass K:\n    __all__ = ['nothing']\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_is_concatenated_across_if_branches() {
    let m = "import sys\nclass Base:\n    pass\nif sys.platform == 'win32':\n    __all__ = ['Base']\nelse:\n    __all__ = ['x']\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_with_a_nested_bracket_before_the_name() {
    let m = "class Base:\n    pass\n__all__ = [*sub(['a']), 'Base']\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_with_parenthesised_call_then_name() {
    let m = "class Base:\n    pass\n__all__ = list(names()) + ['Base']\n";
    star_abstains(m).unwrap();
}

#[test]
fn star_chain_a_b_c_where_c_defines_the_name() {
    let c = "class Base:\n    pass\n";
    let b = "from c import *\n";
    let r = resolve(&[("base.py", BASE), ("c_mod.py", c), ("m.py", "from c_mod import *\n"), ("c.py", USER)]);
    let _ = b;
    assert!(!hits(&targets(&r, "c.py::C.go"), "base.py::Base.run"));
}

#[test]
fn star_chain_with_middle_all_not_listing_the_name() {
    // m has __all__ = ['y']; c_mod's Base is not exported by m (correct to keep).
    let m = "from c_mod import *\n__all__ = ['y']\ny = 1\n";
    let r = resolve(&[("base.py", BASE), ("c_mod.py", "class Base:\n    pass\n"), ("m.py", m), ("c.py", USER)]);
    assert!(hits(&targets(&r, "c.py::C.go"), "base.py::Base.run"));
}

#[test]
fn star_circular_terminates() {
    let a = "from b import *\nclass Base:\n    pass\n";
    let b = "from a import *\n";
    let user = "from base import Base\nfrom a import *\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    let r = resolve(&[("base.py", BASE), ("a.py", a), ("b.py", b), ("c.py", user)]);
    assert!(!hits(&targets(&r, "c.py::C.go"), "base.py::Base.run"));
}

#[test]
fn star_from_module_with_plain_assignment_of_name() {
    star_abstains("Base = object\n").unwrap();
}

#[test]
fn star_from_module_with_guarded_class_of_name() {
    star_abstains("import sys\nif sys.platform == 'win32':\n    class Base:\n        pass\n").unwrap();
}

#[test]
fn star_from_module_with_try_def_of_name() {
    star_abstains("try:\n    from other import Base\nexcept ImportError:\n    class Base:\n        pass\n").unwrap();
}

#[test]
fn star_from_module_binding_name_by_for_target() {
    star_abstains("for Base in [1]:\n    pass\n").unwrap();
}

#[test]
fn star_from_module_binding_name_by_import_alias() {
    star_abstains("import os as Base\n").unwrap();
}

#[test]
fn star_from_module_with_global_write_in_function() {
    star_abstains("def f():\n    global Base\n    Base = 1\n").unwrap();
}

#[test]
fn star_from_package_init_reexporting_submodule_star() {
    let user = "from base import Base\nfrom pkg import *\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    let r = resolve(&[
        ("base.py", BASE),
        ("pkg/__init__.py", "from .impl import *\n"),
        ("pkg/impl.py", "class Base:\n    pass\n"),
        ("c.py", user),
    ]);
    assert!(!hits(&targets(&r, "c.py::C.go"), "base.py::Base.run"));
}

#[test]
fn star_relative_import_defining_name() {
    let user = "from base import Base\nfrom .m import *\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    let r = resolve(&[
        ("pkg/base.py", BASE),
        ("pkg/m.py", "class Base:\n    pass\n"),
        ("pkg/c.py", &user.replace("from base", "from pkg.base")),
    ]);
    assert!(!hits(&targets(&r, "pkg/c.py::C.go"), "pkg/base.py::Base.run"));
}

/// A real function named `__all__` files its locals under `{m}::__all__`,
/// the same key the literal `__all__` list uses.
#[test]
fn function_named_all_does_not_become_the_export_list() {
    let m = "class Base:\n    pass\n\ndef __all__():\n    tmp = 1\n    return tmp\n";
    star_abstains(m).unwrap();
}

#[test]
fn class_named_all_does_not_become_the_export_list() {
    let m = "class Base:\n    pass\n\nclass __all__:\n    def f(self):\n        tmp = 1\n";
    star_abstains(m).unwrap();
}

#[test]
fn all_scope_key_is_not_a_symbol_or_edge() {
    let m = "class Base:\n    pass\n__all__ = ['Base']\n";
    let ex = extract_file("m.py", m);
    assert!(
        !ex.symbols.iter().any(|s| s.qualified_name.contains("__all__") || s.name.contains("__all__")),
        "{:?}",
        ex.symbols.iter().map(|s| &s.qualified_name).collect::<Vec<_>>()
    );
    let r = resolve(&[("m.py", m)]);
    assert!(!r.edges.iter().any(|e| e.source_symbol.contains("__all__") || e.target_symbol.contains("__all__")));
}

// ------------------------------------------------------------ 4. ambiguity

fn base_run_edge(c: &str) -> bool {
    let r = resolve(&[("base.py", BASE), ("c.py", c)]);
    hits(&targets(&r, "c.py::C.go"), "base.py::Base.run")
}

#[test]
fn monkeypatched_attribute_on_imported_base_still_binds() {
    let c = "from base import Base\nBase.x = 1\nBase.y = 2\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    assert!(base_run_edge(c));
}

#[test]
fn monkeypatched_attribute_on_local_base_still_binds() {
    let src = "class Base:\n    def run(self):\n        return 1\n\nBase.x = 1\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    let r = resolve(&[("c.py", src)]);
    assert!(hits(&targets(&r, "c.py::C.go"), "Base.run"), "{:?}", targets(&r, "c.py::C.go"));
}

#[test]
fn dataclass_rewrap_rebinding_characterised() {
    // `Base = dataclass(Base)` is a rebinding to (in practice) the same class.
    let src = "from dataclasses import dataclass\n\nclass Base:\n    def run(self):\n        return 1\n\nBase = dataclass(Base)\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    let r = resolve(&[("c.py", src)]);
    assert!(hits(&targets(&r, "c.py::C.go"), "Base.run"), "recall: {:?}", targets(&r, "c.py::C.go"));
}

#[test]
#[ignore = "recall gap, ft-572bb69d: `Base = dec(Base)` rebinds Base to whatever dec returns, so abstaining is the sound reading; the probe asserts recall CPython does not promise"]
fn imported_base_rewrapped_characterised() {
    let c = "from base import Base\nBase = dec(Base)\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    assert!(base_run_edge(c), "recall");
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97), ft-572bb69d"]
fn annotation_only_name_does_not_make_it_ambiguous() {
    let c = "from base import Base\nBase: type\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    assert!(base_run_edge(c));
}

#[test]
fn name_bound_twice_by_import_and_class_is_unknown_base() {
    let c = "try:\n    from base import Base\nexcept ImportError:\n    class Base:\n        pass\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    assert!(!base_run_edge(c));
}

#[test]
fn name_bound_twice_by_import_and_assignment_is_unknown_base() {
    let c = "try:\n    from base import Base\nexcept ImportError:\n    Base = object\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    assert!(!base_run_edge(c), "assignment fallback makes Base ambiguous");
}

#[test]
fn same_name_bound_in_function_scope_does_not_make_module_base_ambiguous() {
    let c = "from base import Base\n\ndef f():\n    Base = 1\n    return Base\n\nclass C(Base):\n    def go(self):\n        return self.run()\n";
    assert!(base_run_edge(c));
}

// ------------------------------------------------------------ 5. torch

fn forward_wired(src: &str) -> bool {
    extract_file("m.py", src)
        .wiring
        .iter()
        .any(|w| w.kind == WiringKind::RuntimeEntryPoint && w.target_symbol.contains("forward"))
}

const MODULE_BODY: &str = "\nclass M(nn.Module):\n    def forward(self, x):\n        return x\n";

#[test]
fn torch_import_as_nn_is_wired() {
    assert!(forward_wired(&format!("import torch.nn as nn\n{MODULE_BODY}")));
}

#[test]
fn torch_from_import_nn_is_wired() {
    assert!(forward_wired(&format!("from torch import nn\n{MODULE_BODY}")));
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97), ft-572bb69d"]
fn torch_from_import_nn_then_rebound_is_not_wired() {
    assert!(!forward_wired(&format!(
        "from torch import nn\nfrom mylib import fake\nnn = fake\n{MODULE_BODY}"
    )));
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97), ft-572bb69d"]
fn torch_nn_rebound_by_plain_assignment_is_not_wired() {
    assert!(!forward_wired(&format!("import torch.nn as nn\nnn = object()\n{MODULE_BODY}")));
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97), ft-572bb69d"]
fn torch_nn_rebound_by_def_is_not_wired() {
    assert!(!forward_wired(&format!("import torch.nn as nn\ndef nn():\n    pass\n{MODULE_BODY}")));
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97), ft-572bb69d"]
fn torch_imported_only_under_type_checking_is_not_runtime_wired() {
    assert!(!forward_wired(&format!(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import torch.nn as nn\n{MODULE_BODY}"
    )));
}

#[test]
fn torch_alias_is_a_local_package_is_not_wired() {
    assert!(!forward_wired(&format!("from torchlike import nn\n{MODULE_BODY}")));
    assert!(!forward_wired(&format!("import torch_geometric.nn as nn\n{MODULE_BODY}")));
}

// ------------------------------------------- 3b. star import, dotted base
//
// The plain-name self-call path abstains on any later star import, so the
// effective-binds filter only matters for a dotted base `mod.Base`.

const DOT_MOD: &str = "class Base:\n    def run(self):\n        return 1\n";
const DOT_USER: &str = "import mod\nfrom m import *\n\nclass C(mod.Base):\n    def go(self):\n        return self.run()\n";

fn dotted_star_edge(m: &str) -> bool {
    let r = resolve(&[("mod.py", DOT_MOD), ("m.py", m), ("c.py", DOT_USER)]);
    hits(&targets(&r, "c.py::C.go"), "mod.py::Base.run")
}

#[test]
fn dotted_control_star_from_unrelated_module_keeps_the_edge() {
    assert!(dotted_star_edge("def other():\n    return 1\n"));
}

#[test]
fn dotted_control_star_module_defining_root_abstains() {
    assert!(!dotted_star_edge("mod = None\n"));
}

#[test]
fn dotted_star_all_literal_without_root_keeps_edge() {
    assert!(dotted_star_edge("mod = None\nx = 1\n__all__ = ['x']\n"));
}

#[test]
fn dotted_star_all_literal_plus_other_all_must_abstain() {
    let other = "mod = None\n__all__ = ['mod']\n";
    let m = "import other\nfrom other import *\n__all__ = ['x'] + other.__all__\nx = 1\n";
    let r = resolve(&[("mod.py", DOT_MOD), ("other.py", other), ("m.py", m), ("c.py", DOT_USER)]);
    assert!(!hits(&targets(&r, "c.py::C.go"), "mod.py::Base.run"), "m exports `mod` through other.__all__");
}

#[test]
fn dotted_star_all_prefix_then_call_extension_must_abstain() {
    let m = "from other import helper\n__all__ = ['x']\n__all__ += helper()\nx = 1\n";
    assert!(!dotted_star_edge(m), "`__all__ += helper()` may add `mod`");
}

#[test]
fn dotted_star_all_prefix_then_extend_call_must_abstain() {
    let m = "from other import helper\n__all__ = ['x']\n__all__.extend(helper())\nx = 1\n";
    assert!(!dotted_star_edge(m));
}

#[test]
fn dotted_star_all_prefix_then_setattr_style_update_must_abstain() {
    let m = "__all__ = ['x']\nx = 1\nglobals().update(mod=1)\n__all__.append('mod')\n";
    assert!(!dotted_star_edge(m), "`__all__.append('mod')` exports mod");
}

#[test]
fn dotted_star_all_is_tuple_with_root() {
    assert!(!dotted_star_edge("mod = None\n__all__ = ('mod',)\n"));
}

#[test]
fn dotted_star_all_concatenated_two_literals() {
    assert!(!dotted_star_edge("mod = None\n__all__ = ['x'] + ['mod']\nx = 1\n"));
}

#[test]
fn dotted_star_all_in_a_conditional_second_assignment() {
    let m = "import sys\nmod = None\n__all__ = ['x']\nx = 1\nif sys.platform == 'win32':\n    __all__ = ['mod']\n";
    assert!(!dotted_star_edge(m), "the guarded `__all__` may be the live one");
}

#[test]
fn dotted_star_all_unlisted_name_defined_in_module_is_not_exported() {
    // `mod` defined but `__all__` omits it: kept edge is CPython-correct.
    assert!(dotted_star_edge("mod = None\nx = 1\n__all__ = ['x']\n"));
}

#[test]
fn dotted_star_function_named_all_collides_with_the_export_key() {
    let m = "mod = None\n\ndef __all__():\n    tmp = 1\n    return tmp\n";
    assert!(!dotted_star_edge(m), "`def __all__` locals read as the export list");
}

#[test]
fn dotted_star_function_named_all_with_local_named_like_root() {
    let m = "mod = None\n\ndef __all__():\n    mod = 1\n    return mod\n";
    assert!(!dotted_star_edge(m));
}

#[test]
fn dotted_star_chain_a_b_c() {
    let b = "from c_mod import *\n";
    let c = "mod = None\n";
    let r = resolve(&[("mod.py", DOT_MOD), ("c_mod.py", c), ("m.py", b), ("c.py", DOT_USER)]);
    assert!(!hits(&targets(&r, "c.py::C.go"), "mod.py::Base.run"));
}

#[test]
fn dotted_star_chain_through_all_bearing_middle_reexport() {
    // m lists `mod` in __all__ and gets it from c_mod by star.
    let m = "from c_mod import *\n__all__ = ['mod']\n";
    let r = resolve(&[("mod.py", DOT_MOD), ("c_mod.py", "mod = None\n"), ("m.py", m), ("c.py", DOT_USER)]);
    assert!(!hits(&targets(&r, "c.py::C.go"), "mod.py::Base.run"));
}

#[test]
fn dotted_star_circular_terminates_and_abstains() {
    let r = resolve(&[
        ("mod.py", DOT_MOD),
        ("a.py", "from b import *\nmod = None\n"),
        ("b.py", "from a import *\n"),
        ("c.py", "import mod\nfrom a import *\n\nclass C(mod.Base):\n    def go(self):\n        return self.run()\n"),
    ]);
    assert!(!hits(&targets(&r, "c.py::C.go"), "mod.py::Base.run"));
}

// ------------------------------------------------------------ 6. dotted base

fn dotted(n: usize) -> String {
    let mut base = String::from("a");
    for _ in 1..n {
        base.push_str(".a");
    }
    format!("class C({base}):\n    def m(self):\n        return 1\n")
}

#[test]
fn dotted_base_100k_segments_does_not_crash_or_go_quadratic() {
    let big = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let time = |n: usize| {
                let src = dotted(n);
                let start = std::time::Instant::now();
                let ex = extract_file("c.py", &src);
                (start.elapsed(), ex.symbols.len())
            };
            let (t25, _) = time(25_000);
            let (t100, _) = time(100_000);
            (t25, t100)
        })
        .unwrap()
        .join()
        .expect("extractor panicked/overflowed");
    let (t25, t100) = big;
    // Linear would be ~4x; quadratic ~16x. Allow generous slack plus a floor.
    assert!(
        t100.as_secs_f64() < 12.0 * t25.as_secs_f64().max(0.05),
        "t25={t25:?} t100={t100:?}"
    );
}

#[test]
fn dotted_base_100k_parens_does_not_crash() {
    let depth = 100_000;
    let src = format!("class C({}a{}):\n    pass\n", "(".repeat(depth), ")".repeat(depth));
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let _ = extract_file("c.py", &src);
        })
        .unwrap()
        .join()
        .expect("extractor panicked/overflowed");
}

#[test]
fn dotted_base_resolves_100k_through_the_resolver() {
    let src = dotted(100_000);
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let _ = resolve(&[("c.py", &src)]);
        })
        .unwrap()
        .join()
        .expect("resolver panicked/overflowed");
}
