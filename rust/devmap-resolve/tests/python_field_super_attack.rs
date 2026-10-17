//! Adversarial probes for Python `self.field = Callee()` typing and the
//! per-file `super()` base walk. Each test states the correct behaviour; a
//! failing test is a BREAK of the lane under attack.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;
use std::sync::mpsc;
use std::time::Duration;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn resolve_owned_within(
    files: Vec<(String, String)>,
    secs: u64,
) -> Option<(ResolutionResult, Duration)> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let refs: Vec<(&str, &str)> = files
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            let started = std::time::Instant::now();
            let r = resolve(&refs);
            let _ = tx.send((r, started.elapsed()));
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

fn resolve_within(files: &[(&str, &str)], secs: u64) -> Option<ResolutionResult> {
    let owned = files
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    resolve_owned_within(owned, secs).map(|(r, _)| r)
}

fn targets(res: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = res
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == caller)
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out
}

/// Method-shaped targets only (`file::Class.method`).
fn methods(res: &ResolutionResult, caller: &str) -> Vec<String> {
    targets(res, caller)
        .into_iter()
        .filter(|t| t.rsplit("::").next().is_some_and(|tail| tail.contains('.')))
        .collect()
}

const ENG: &str = "class Engine:\n    def run(self):\n        return 1\n";
const A_SRC: &str = "class A:\n    def run(self):\n        return 1\n";
const B_SRC: &str = "class B:\n    def run(self):\n        return 2\n";
const A_RUN: &str = "a.py::A.run";
const B_RUN: &str = "b.py::B.run";

fn assert_no_wrong(res: &ResolutionResult, caller: &str, wrong: &[&str]) {
    let got = methods(res, caller);
    for w in wrong {
        assert!(!got.iter().any(|g| g == w), "bound wrong target {w}: {got:?}");
    }
}

fn is_a_or_nothing(got: &[String]) -> bool {
    got.is_empty() || got == [A_RUN.to_string()]
}

// ---------------------------------------------------------------- fields

#[test]
fn field_external_import_named_like_in_repo_class_does_not_bind_in_repo() {
    let r = resolve(&[
        ("engines.py", ENG),
        (
            "h.py",
            "from extpkg import Engine\n\nclass H:\n    def __init__(self):\n        self.engine = Engine()\n    def go(self):\n        return self.engine.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &["engines.py::Engine.run"]);
}

#[test]
fn field_external_module_attr_named_like_in_repo_class() {
    let r = resolve(&[
        ("engines.py", ENG),
        (
            "h.py",
            "import extpkg\n\nclass H:\n    def __init__(self):\n        self.engine = extpkg.Engine()\n    def go(self):\n        return self.engine.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &["engines.py::Engine.run"]);
}

#[test]
fn field_unimported_in_repo_name_does_not_bind() {
    let r = resolve(&[
        ("engines.py", ENG),
        (
            "h.py",
            "class H:\n    def __init__(self):\n        self.engine = Engine()\n    def go(self):\n        return self.engine.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &["engines.py::Engine.run"]);
}

fn run_holder(init: &str) -> Vec<String> {
    let h = format!(
        "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self, c=None):\n{init}\n    def go(self):\n        return self.x.run()\n"
    );
    let r = resolve(&[("a.py", A_SRC), ("b.py", B_SRC), ("h.py", h.as_str())]);
    methods(&r, "h.py::H.go")
}

#[test]
fn field_and_or_chain_binds_nothing() {
    let got = run_holder("        self.x = c and A() or B()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn field_ternary_binds_nothing() {
    let got = run_holder("        self.x = A() if c else B()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn field_tuple_assign_binds_nothing_wrong() {
    // x is B, y is A: x must not be typed A.
    let got = run_holder("        self.x, self.y = B(), A()");
    assert!(!got.contains(&A_RUN.to_string()), "x is B, bound A: {got:?}");
}

#[test]
fn field_tuple_assign_single_call_rhs() {
    let got = run_holder("        self.x, self.y = A()");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn field_chained_assignment_second_target() {
    let got = run_holder("        self.y = self.x = B()");
    assert!(!got.contains(&A_RUN.to_string()), "{got:?}");
}

#[test]
fn field_augmented_assignment_is_not_an_initializer() {
    let got = run_holder("        self.x = B()\n        self.x += A()");
    assert!(!got.contains(&A_RUN.to_string()), "{got:?}");
}

#[test]
fn field_walrus_rhs_binds_nothing_wrong() {
    let got = run_holder("        self.x = (y := A())");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn field_attribute_call_rhs_is_not_a_constructor() {
    // `self.x = A().build()` returns whatever build returns, not A.
    let got = run_holder("        self.x = A().build()");
    assert!(!got.contains(&A_RUN.to_string()), "typed field as A: {got:?}");
}

#[test]
fn field_call_of_call_rhs() {
    let got = run_holder("        self.x = A()()");
    assert!(!got.contains(&A_RUN.to_string()), "{got:?}");
}

#[test]
fn field_subscript_rhs() {
    let got = run_holder("        self.x = A()[0]");
    assert!(!got.contains(&A_RUN.to_string()), "typed as A: {got:?}");
}

#[test]
fn field_method_call_on_constructor_arg_rhs() {
    // callee-position call inside arguments of a different callee
    let got = run_holder("        self.x = B(A())");
    assert!(!got.contains(&A_RUN.to_string()), "typed as the argument's class: {got:?}");
}

#[test]
fn field_keyword_argument_call_rhs() {
    let got = run_holder("        self.x = B(k=A())");
    assert!(!got.contains(&A_RUN.to_string()), "{got:?}");
}

#[test]
fn field_generator_or_lambda_rhs() {
    let got = run_holder("        self.x = (lambda: A())()");
    assert!(!got.contains(&A_RUN.to_string()), "{got:?}");
}

#[test]
fn field_annotation_wins_over_later_initializer() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from typing import Optional\nfrom a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self.x: Optional[A] = None\n    def reset(self):\n        self.x = B()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(!got.contains(&B_RUN.to_string()), "initializer beat annotation: {got:?}");
}

#[test]
fn field_class_level_annotation_wins_over_none_and_initializer() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass H:\n    x: A\n    def __init__(self):\n        self.x = B()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(!got.contains(&B_RUN.to_string()), "initializer beat class annotation: {got:?}");
}

#[test]
fn field_annotated_assignment_with_call_value() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self.x: A = B()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(!got.contains(&B_RUN.to_string()), "{got:?}");
}

#[test]
fn field_assigned_in_nested_function_counts_as_a_second_shape() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self.x = B()\n        def later():\n            self.x = A()\n        later()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(got.is_empty(), "field assigned two shapes (one in a closure) still typed: {got:?}");
}

#[test]
fn field_assigned_via_setattr_lambda_is_not_recorded_wrongly() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self.x = B()\n        f = lambda: setattr(self, 'x', A())\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(!got.contains(&A_RUN.to_string()), "{got:?}");
}

#[test]
fn field_on_other_object_is_not_self() {
    let r = resolve(&[
        ("a.py", A_SRC),
        (
            "h.py",
            "from a import A\n\nclass H:\n    def attach(self, other):\n        other.x = A()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn field_on_other_class_instance_param_named_self() {
    // `def attach(self, h2): h2.x = A()` inside class G must not type H.x
    let r = resolve(&[
        ("a.py", A_SRC),
        (
            "h.py",
            "from a import A\n\nclass G:\n    def attach(self, h):\n        h.x = A()\n\nclass H:\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn field_on_cls_in_classmethod() {
    let r = resolve(&[
        ("a.py", A_SRC),
        (
            "h.py",
            "from a import A\n\nclass H:\n    @classmethod\n    def setup(cls):\n        cls.x = A()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn field_on_differently_named_first_param() {
    let r = resolve(&[
        ("a.py", A_SRC),
        (
            "h.py",
            "from a import A\n\nclass H:\n    def __init__(this):\n        this.x = A()\n    def go(this):\n        return this.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn field_self_named_param_in_staticmethod_is_not_an_instance() {
    let r = resolve(&[
        ("a.py", A_SRC),
        (
            "h.py",
            "from a import A\n\nclass H:\n    @staticmethod\n    def poke(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn field_class_object_is_not_an_instance() {
    let r = resolve(&[
        ("a.py", A_SRC),
        (
            "h.py",
            "from a import A\n\nclass H:\n    def __init__(self):\n        self.e = A\n    def go(self):\n        return self.e.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(got.is_empty(), "class object typed as instance: {got:?}");
}

#[test]
fn field_deleted_after_assignment() {
    let got = run_holder("        self.x = A()\n        del self.x");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn field_subclass_override_uses_each_own_initializer() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass P:\n    def __init__(self):\n        self.e = A()\n    def go(self):\n        return self.e.run()\n\nclass C(P):\n    def __init__(self):\n        self.e = B()\n    def go2(self):\n        return self.e.run()\n",
        ),
    ]);
    assert_eq!(methods(&r, "h.py::P.go"), [A_RUN]);
    assert_eq!(methods(&r, "h.py::C.go2"), [B_RUN]);
}

#[test]
fn field_inherited_field_is_not_a_sibling_subclass_type() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass P:\n    def __init__(self):\n        self.e = A()\n\nclass C(P):\n    def go(self):\n        return self.e.run()\n\nclass D(P):\n    def __init__(self):\n        self.e = B()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::C.go", &[B_RUN]);
}

#[test]
fn field_same_name_in_nested_classes_in_one_file() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass One:\n    class Inner:\n        def __init__(self):\n            self.e = A()\n        def go(self):\n            return self.e.run()\n\nclass Two:\n    class Inner:\n        def __init__(self):\n            self.e = B()\n        def go(self):\n            return self.e.run()\n",
        ),
    ]);
    let one = methods(&r, "h.py::One.Inner.go");
    let two = methods(&r, "h.py::Two.Inner.go");
    assert!(!one.contains(&B_RUN.to_string()), "One.Inner typed B: {one:?}");
    assert!(!two.contains(&A_RUN.to_string()), "Two.Inner typed A: {two:?}");
}

#[test]
fn field_same_class_name_in_two_functions() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\ndef f():\n    class H:\n        def __init__(self):\n            self.e = A()\n        def go(self):\n            return self.e.run()\n    return H\n\ndef g():\n    class H:\n        def __init__(self):\n            self.e = B()\n        def go(self):\n            return self.e.run()\n    return H\n",
        ),
    ]);
    let edges: Vec<String> = r
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls)
        .map(|e| format!("{} -> {}", e.source_symbol, e.target_symbol))
        .collect();
    for line in &edges {
        if line.starts_with("h.py::f") {
            assert!(!line.contains("B.run"), "f's H typed B: {edges:?}");
        }
        if line.starts_with("h.py::g") {
            assert!(!line.contains("A.run"), "g's H typed A: {edges:?}");
        }
    }
}

#[test]
fn field_same_class_and_field_in_two_files_does_not_leak() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        ("h1.py", "from a import A\n\nclass H:\n    def __init__(self):\n        self.e = A()\n"),
        ("h2.py", "from b import B\n\nclass H:\n    def __init__(self):\n        self.e = B()\n    def go(self):\n        return self.e.run()\n"),
        ("h3.py", "class H:\n    def go(self):\n        return self.e.run()\n"),
    ]);
    assert_eq!(methods(&r, "h2.py::H.go"), [B_RUN]);
    assert!(methods(&r, "h3.py::H.go").is_empty(), "{:?}", methods(&r, "h3.py::H.go"));
}

#[test]
fn field_property_of_other_name_does_not_cross_type() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self.e = A()\n    @property\n    def eng(self):\n        return B()\n    def go(self):\n        return self.eng.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn field_property_same_name_as_assigned_field_is_not_the_initializer() {
    // `e` is a property returning B on the class; `self.e = A()` in __init__
    // would raise at runtime, so the property is the truth. Never bind A.
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self._e = A()\n    @property\n    def e(self) -> B:\n        return B()\n    def go(self):\n        return self.e.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn field_constructor_param_shadows_import() {
    let r = resolve(&[
        ("a.py", A_SRC),
        (
            "h.py",
            "from a import A\n\nclass H:\n    def __init__(self, A):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn field_local_rebinding_of_class_name_shadows_import() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        A = B\n        self.x = A()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn field_walrus_target_is_not_an_attribute_assignment() {
    let got = run_holder("        if (self.x := A()):\n            pass");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn field_dunder_slots_or_with_target() {
    // `with A() as self.x` binds the context manager's __enter__ result.
    let got = run_holder("        with A() as self.x:\n            pass");
    assert!(!got.contains(&A_RUN.to_string()), "with-target typed as A: {got:?}");
}

#[test]
fn field_for_target_is_not_an_initializer() {
    let got = run_holder("        for self.x in A():\n            pass");
    assert!(!got.contains(&A_RUN.to_string()), "{got:?}");
}

#[test]
fn field_poison_is_order_independent() {
    // Poison must hold whichever order the two shapes arrive in.
    let one = run_holder("        self.x = A()\n        self.x = B()");
    let two = run_holder("        self.x = B()\n        self.x = A()");
    assert!(one.is_empty(), "{one:?}");
    assert!(two.is_empty(), "{two:?}");
}

#[test]
fn field_three_writes_a_b_a_stays_poisoned() {
    // Remove-then-reinsert regression: A, B (poison), A again must not revive.
    let got = run_holder("        self.x = A()\n        self.x = B()\n        self.x = A()");
    assert!(got.is_empty(), "poison revived by third write: {got:?}");
}

#[test]
fn field_poison_across_files_is_not_shared() {
    // Poisoning H.x in h1.py must not blind the same-named class in h2.py.
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        ("h1.py", "from a import A\nfrom b import B\n\nclass H:\n    def __init__(self):\n        self.e = A()\n        self.e = B()\n"),
        ("h2.py", "from a import A\n\nclass H:\n    def __init__(self):\n        self.e = A()\n    def go(self):\n        return self.e.run()\n"),
    ]);
    assert_eq!(methods(&r, "h2.py::H.go"), [A_RUN]);
}

// --------------------------------------------------------------- factories

fn factory_case(def: &str, extra_import: &str) -> Vec<String> {
    let src = format!(
        "{extra_import}from a import A\n\n{def}\n\nclass H:\n    def __init__(self):\n        self.x = make()\n    def go(self):\n        return self.x.run()\n"
    );
    let r = resolve(&[("a.py", A_SRC), ("h.py", src.as_str())]);
    methods(&r, "h.py::H.go")
}

#[test]
fn factory_string_annotation_binds_or_abstains() {
    let got = factory_case("def make() -> \"A\":\n    ...", "");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn factory_optional_binds_or_abstains() {
    let got = factory_case("def make() -> Optional[A]:\n    ...", "from typing import Optional\n");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn factory_union_none_binds_or_abstains() {
    let got = factory_case("def make() -> A | None:\n    ...", "");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn factory_list_return_does_not_type_as_element() {
    let got = factory_case("def make() -> list[A]:\n    ...", "");
    assert!(got.is_empty(), "list[A] factory typed as A: {got:?}");
}

#[test]
fn factory_type_of_return_does_not_type_as_instance() {
    let got = factory_case("def make() -> type[A]:\n    ...", "");
    assert!(got.is_empty(), "type[A] factory typed as instance A: {got:?}");
}

#[test]
fn factory_overloaded_does_not_bind_wrong() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        (
            "h.py",
            "from typing import overload\nfrom a import A\nfrom b import B\n\n@overload\ndef make(k: int) -> A: ...\n@overload\ndef make(k: str) -> B: ...\ndef make(k):\n    return None\n\nclass H:\n    def __init__(self):\n        self.x = make(1)\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(got.is_empty(), "overloaded factory typed: {got:?}");
}

#[test]
fn factory_method_on_self_is_not_a_module_factory() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("f.py", "from a import A\n\ndef make() -> A:\n    return A()\n"),
        (
            "h.py",
            "class H:\n    def make(self):\n        return 3\n    def __init__(self):\n        self.x = make()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

#[test]
fn factory_imported_from_another_module() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("f.py", "from a import A\n\ndef make() -> A:\n    return A()\n"),
        (
            "h.py",
            "from f import make\n\nclass H:\n    def __init__(self):\n        self.x = make()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(is_a_or_nothing(&got), "{got:?}");
}

#[test]
fn factory_name_colliding_across_files_uses_the_imported_one() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("b.py", B_SRC),
        ("fa.py", "from a import A\n\ndef make() -> A:\n    return A()\n"),
        ("fb.py", "from b import B\n\ndef make() -> B:\n    return B()\n"),
        (
            "h.py",
            "from fa import make\n\nclass H:\n    def __init__(self):\n        self.x = make()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    let got = methods(&r, "h.py::H.go");
    assert!(!got.contains(&B_RUN.to_string()), "bound the namesake factory's type: {got:?}");
}

#[test]
fn factory_external_import_named_like_in_repo_factory() {
    let r = resolve(&[
        ("a.py", A_SRC),
        ("f.py", "from a import A\n\ndef make() -> A:\n    return A()\n"),
        (
            "h.py",
            "from extpkg import make\n\nclass H:\n    def __init__(self):\n        self.x = make()\n    def go(self):\n        return self.x.run()\n",
        ),
    ]);
    assert_no_wrong(&r, "h.py::H.go", &[A_RUN]);
}

// ----------------------------------------------------------------- super()

const BASE: &str = "class Base:\n    def m(self):\n        return 1\n";
const BASE2: &str = "class Base:\n    def m(self):\n        return 2\n";

#[test]
fn super_two_arg_form_binds_base() {
    let r = resolve(&[(
        "m.py",
        "class Base:\n    def m(self):\n        return 1\n\nclass Child(Base):\n    def m(self):\n        return super(Child, self).m()\n",
    )]);
    assert_eq!(methods(&r, "m.py::Child.m"), ["m.py::Base.m"]);
}

#[test]
fn super_two_arg_form_naming_another_class_does_not_read_enclosing_bases() {
    let r = resolve(&[(
        "m.py",
        "class B1:\n    def m(self):\n        return 1\n\nclass B2:\n    def m(self):\n        return 2\n\nclass Child(B1):\n    def m(self):\n        return super(B2, self).m()\n",
    )]);
    assert_no_wrong(&r, "m.py::Child.m", &["m.py::B1.m"]);
}

#[test]
fn super_via_module_alias_base_collision_refuses_entirely() {
    let r = resolve(&[
        ("one.py", BASE),
        ("two.py", BASE2),
        (
            "c.py",
            "import one as o\n\nclass C(o.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert!(methods(&r, "c.py::C.m").is_empty(), "{:?}", methods(&r, "c.py::C.m"));
}

#[test]
fn super_via_dotted_module_alias_base_collision() {
    let r = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/mod.py", BASE),
        ("other/mod.py", BASE2),
        (
            "c.py",
            "import pkg.mod as m\n\nclass C(m.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    let got = methods(&r, "c.py::C.m");
    assert!(!got.contains(&"other/mod.py::Base.m".to_string()), "bound wrong namesake: {got:?}");
}

#[test]
fn super_external_base_named_like_in_repo_class_does_not_bind_in_repo() {
    let r = resolve(&[
        ("lib.py", BASE),
        (
            "c.py",
            "from extpkg import Base\n\nclass C(Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert!(
        methods(&r, "c.py::C.m").is_empty(),
        "external Base resolved to the in-repo namesake: {:?}",
        methods(&r, "c.py::C.m")
    );
}

#[test]
fn super_external_dotted_base_named_like_in_repo_class() {
    let r = resolve(&[
        ("lib.py", BASE),
        (
            "c.py",
            "import extpkg\n\nclass C(extpkg.Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert!(
        methods(&r, "c.py::C.m").is_empty(),
        "external Base resolved to the in-repo namesake: {:?}",
        methods(&r, "c.py::C.m")
    );
}

#[test]
fn inherited_call_external_base_named_like_in_repo_class() {
    let r = resolve(&[
        ("lib.py", "class Base:\n    def quack(self):\n        return 1\n"),
        (
            "c.py",
            "from extpkg import Base\n\nclass C(Base):\n    pass\n\ndef run():\n    return C().quack()\n",
        ),
    ]);
    assert!(
        methods(&r, "c.py::run").is_empty(),
        "external Base resolved to the in-repo namesake: {:?}",
        methods(&r, "c.py::run")
    );
}

#[test]
fn super_circular_inheritance_across_files_terminates() {
    let r = resolve_within(
        &[
            ("a.py", "from b import B\n\nclass A(B):\n    def m(self):\n        return super().m()\n"),
            ("b.py", "from a import A\n\nclass B(A):\n    def m(self):\n        return super().m()\n"),
        ],
        20,
    )
    .expect("circular inheritance hung the resolver");
    assert!(methods(&r, "a.py::A.m").len() <= 1);
}

#[test]
fn super_self_inheritance_terminates() {
    let r = resolve_within(
        &[("a.py", "class A(A):\n    def m(self):\n        return super().m()\n")],
        20,
    )
    .expect("self inheritance hung");
    assert!(methods(&r, "a.py::A.m").is_empty(), "{:?}", methods(&r, "a.py::A.m"));
}

fn deep_chain(n: usize) -> Vec<(String, String)> {
    let mut files = vec![(
        "c0.py".to_string(),
        "class C0:\n    def m(self):\n        return 1\n".to_string(),
    )];
    for i in 1..=n {
        let p = i - 1;
        files.push((
            format!("c{i}.py"),
            format!(
                "from c{p} import C{p}\n\nclass C{i}(C{p}):\n    def other(self):\n        return super().m()\n"
            ),
        ));
    }
    files
}

#[test]
fn super_deep_chain_200_levels_terminates_without_wrong_edge() {
    let (r, took) = resolve_owned_within(deep_chain(200), 90).expect("deep chain hung");
    assert!(took < Duration::from_secs(30), "deep chain took {took:?}");
    for got in methods(&r, "c200.py::C200.other") {
        assert_eq!(got, "c0.py::C0.m");
    }
}

#[test]
fn super_deep_chain_within_bound_binds_root() {
    let (r, _) = resolve_owned_within(deep_chain(7), 60).expect("hung");
    assert_eq!(methods(&r, "c7.py::C7.other"), ["c0.py::C0.m"]);
}

#[test]
fn super_wide_fanout_terminates() {
    // 300 bases of one class: the walk must stay bounded.
    let mut src = String::new();
    let mut names = Vec::new();
    for i in 0..300 {
        src.push_str(&format!("class B{i}:\n    pass\n\n"));
        names.push(format!("B{i}"));
    }
    src.push_str(&format!(
        "class C({}):\n    def m(self):\n        return super().m()\n",
        names.join(", ")
    ));
    let (r, took) = resolve_owned_within(vec![("c.py".to_string(), src)], 60).expect("hung");
    assert!(took < Duration::from_secs(30), "{took:?}");
    assert!(methods(&r, "c.py::C.m").is_empty());
}

#[test]
fn super_star_import_base_with_namesake_does_not_bind_namesake() {
    let r = resolve(&[
        ("one.py", BASE),
        ("two.py", BASE2),
        (
            "c.py",
            "from two import *\n\nclass C(Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    let got = methods(&r, "c.py::C.m");
    assert!(!got.contains(&"one.py::Base.m".to_string()), "star import bound the wrong Base: {got:?}");
}

#[test]
fn super_base_reassigned_alias() {
    let r = resolve(&[(
        "c.py",
        "class Base:\n    def m(self):\n        return 1\n\nclass Other:\n    def m(self):\n        return 2\n\nBase = Other\n\nclass C(Base):\n    def m(self):\n        return super().m()\n",
    )]);
    let got = methods(&r, "c.py::C.m");
    assert!(!got.contains(&"c.py::Base.m".to_string()), "base was rebound to Other: {got:?}");
}

#[test]
fn super_generic_base_binds() {
    let r = resolve(&[(
        "c.py",
        "from typing import Generic, TypeVar\nT = TypeVar('T')\n\nclass Base(Generic[T]):\n    def m(self):\n        return 1\n\nclass C(Base[int]):\n    def m(self):\n        return super().m()\n",
    )]);
    assert_eq!(methods(&r, "c.py::C.m"), ["c.py::Base.m"]);
}

#[test]
fn super_metaclass_keyword_is_not_a_base() {
    let r = resolve(&[(
        "c.py",
        "class M(type):\n    def m(cls):\n        return 9\n\nclass Base:\n    def m(self):\n        return 1\n\nclass C(Base, metaclass=M):\n    def m(self):\n        return super().m()\n",
    )]);
    assert_eq!(methods(&r, "c.py::C.m"), ["c.py::Base.m"]);
}

#[test]
fn super_metaclass_only_class_has_no_base() {
    let r = resolve(&[(
        "c.py",
        "class M(type):\n    def m(cls):\n        return 9\n\nclass C(metaclass=M):\n    def m(self):\n        return super().m()\n",
    )]);
    assert!(methods(&r, "c.py::C.m").is_empty(), "{:?}", methods(&r, "c.py::C.m"));
}

#[test]
fn super_conditional_class_definitions_abstain() {
    let r = resolve(&[(
        "c.py",
        "import os\nif os.name == 'nt':\n    class Base:\n        def m(self):\n            return 1\nelse:\n    class Base:\n        def m(self):\n            return 2\n\nclass C(Base):\n    def m(self):\n        return super().m()\n",
    )]);
    assert!(methods(&r, "c.py::C.m").is_empty(), "{:?}", methods(&r, "c.py::C.m"));
}

#[test]
fn super_diamond_only_apex_declares_binds() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\n\nclass B(A):\n    pass\n\nclass C(A):\n    pass\n\nclass D(B, C):\n    def m(self):\n        return super().m()\n",
    )]);
    assert_eq!(methods(&r, "c.py::D.m"), ["c.py::A.m"]);
}

#[test]
fn super_diamond_one_branch_declares_binds_that_branch() {
    let r = resolve(&[(
        "c.py",
        "class A:\n    def m(self):\n        return 1\n\nclass B(A):\n    def m(self):\n        return 2\n\nclass C(A):\n    pass\n\nclass D(B, C):\n    def m(self):\n        return super().m()\n",
    )]);
    assert_eq!(methods(&r, "c.py::D.m"), ["c.py::B.m"]);
}

#[test]
fn super_bfs_level_order_disagrees_with_c3_mro() {
    // D(B, C); B(X); X(Y); Y declares m; C declares m.
    // C3 MRO: D, B, X, Y, C -> Y.m. BFS reaches C.m (level 1) before Y.m (level 3).
    let r = resolve(&[(
        "c.py",
        "class Y:\n    def m(self):\n        return 1\n\nclass X(Y):\n    pass\n\nclass B(X):\n    pass\n\nclass C:\n    def m(self):\n        return 3\n\nclass D(B, C):\n    def m(self):\n        return super().m()\n",
    )]);
    let got = methods(&r, "c.py::D.m");
    assert!(!got.contains(&"c.py::C.m".to_string()), "BFS picked C.m but Python's MRO reaches Y.m first: {got:?}");
}

#[test]
fn super_builtin_base_makes_no_in_repo_edge() {
    let r = resolve(&[
        ("lib.py", "class Reg:\n    def get(self, k):\n        return k\n"),
        (
            "c.py",
            "class C(dict):\n    def get(self, k):\n        return super().get(k)\n",
        ),
    ]);
    assert!(methods(&r, "c.py::C.get").is_empty(), "{:?}", methods(&r, "c.py::C.get"));
}

#[test]
fn super_builtin_base_alongside_in_repo_base() {
    // class C(dict, Mixin): super().get -> dict.get first in MRO, not Mixin.get.
    let r = resolve(&[(
        "c.py",
        "class Mixin:\n    def get(self, k):\n        return k\n\nclass C(dict, Mixin):\n    def get(self, k):\n        return super().get(k)\n",
    )]);
    assert!(methods(&r, "c.py::C.get").is_empty(), "dict precedes Mixin in the MRO: {:?}", methods(&r, "c.py::C.get"));
}

#[test]
fn super_external_base_alongside_in_repo_base() {
    let r = resolve(&[(
        "c.py",
        "from extpkg import Ext\n\nclass Mixin:\n    def get(self, k):\n        return k\n\nclass C(Ext, Mixin):\n    def get(self, k):\n        return super().get(k)\n",
    )]);
    assert!(
        methods(&r, "c.py::C.get").is_empty(),
        "unindexed external base precedes Mixin in the MRO: {:?}",
        methods(&r, "c.py::C.get")
    );
}

#[test]
fn super_nested_same_name_classes_do_not_merge_bases() {
    let r = resolve(&[(
        "c.py",
        "class BaseA:\n    def m(self):\n        return 1\n\nclass BaseB:\n    pass\n\nclass One:\n    class Inner(BaseA):\n        pass\n\nclass Two:\n    class Inner(BaseB):\n        def m(self):\n            return super().m()\n",
    )]);
    let got = methods(&r, "c.py::Two.Inner.m");
    assert!(got.is_empty(), "Two.Inner(BaseB) reached BaseA through the other Inner: {got:?}");
}

#[test]
fn super_same_class_name_twice_via_conditional_unions_bases() {
    let r = resolve(&[(
        "c.py",
        "import os\nclass BaseA:\n    def m(self):\n        return 1\n\nclass BaseB:\n    pass\n\nif os.name == 'nt':\n    class C(BaseA):\n        pass\nelse:\n    class C(BaseB):\n        def m(self):\n            return super().m()\n",
    )]);
    let got = methods(&r, "c.py::C.m");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn super_child_name_collision_across_files_uses_own_file_bases() {
    let r = resolve(&[
        ("one.py", "class Base:\n    def m(self):\n        return 1\n\nclass C(Base):\n    def m(self):\n        return super().m()\n"),
        ("two.py", "class Other:\n    pass\n\nclass C(Other):\n    def m(self):\n        return super().m()\n"),
    ]);
    assert_eq!(methods(&r, "one.py::C.m"), ["one.py::Base.m"]);
    assert!(methods(&r, "two.py::C.m").is_empty(), "{:?}", methods(&r, "two.py::C.m"));
}

#[test]
fn super_in_class_defined_inside_function_with_namesake_elsewhere() {
    let r = resolve(&[
        ("other.py", "class Base:\n    def m(self):\n        return 9\n"),
        (
            "c.py",
            "def f():\n    class Base:\n        def m(self):\n            return 1\n    class C(Base):\n        def m(self):\n            return super().m()\n    return C\n",
        ),
    ]);
    let got = methods(&r, "c.py::f.C.m");
    assert!(!got.contains(&"other.py::Base.m".to_string()), "local class resolved to the namesake: {got:?}");
}

#[test]
fn super_depth_bound_does_not_fall_through_to_flat_table_wrongly() {
    // A chain deeper than the walk bound with a same-named class elsewhere:
    // scoped walk exhausts, flat fallback must not mint a namesake edge.
    let mut files = deep_chain(60);
    files.push((
        "z.py".to_string(),
        "class C0:\n    def m(self):\n        return 7\n".to_string(),
    ));
    let (r, _) = resolve_owned_within(files, 60).expect("hung");
    for got in methods(&r, "c60.py::C60.other") {
        assert_eq!(got, "c0.py::C0.m");
    }
}

#[test]
fn super_dead_edge_target_is_declared_in_target_file() {
    // base method declared in a *different* file than the one the namesake sits in
    let r = resolve(&[
        ("one.py", BASE),
        ("two.py", "class Base:\n    pass\n"),
        (
            "c.py",
            "from two import Base\n\nclass C(Base):\n    def m(self):\n        return super().m()\n",
        ),
    ]);
    assert!(
        methods(&r, "c.py::C.m").is_empty(),
        "two.Base has no m; one.Base.m is a namesake: {:?}",
        methods(&r, "c.py::C.m")
    );
}

fn stress_time(n: usize, prefix: &str) -> Duration {
    let mut src = String::from("from a import A\n\nclass Big:\n    def __init__(self):\n");
    for i in 0..n {
        src.push_str(&format!("        {prefix}f{i} = A()\n"));
    }
    src.push_str(&format!("    def go(self):\n        return {prefix}f{}.run()\n", n - 1));
    (0..3)
        .map(|_| {
            let owned = vec![
                ("a.py".to_string(), A_SRC.to_string()),
                ("big.py".to_string(), src.clone()),
            ];
            resolve_owned_within(owned, 300).expect("stress input hung").1
        })
        .min()
        .unwrap()
}

#[test]
fn field_stress_scales_no_worse_than_the_same_number_of_locals() {
    let fields_small = stress_time(2_500, "self.");
    let fields_big = stress_time(10_000, "self.");
    let locals_big = stress_time(10_000, "");
    eprintln!("fields 2.5k {fields_small:?} 10k {fields_big:?} locals 10k {locals_big:?}");
    assert!(
        fields_big.as_secs_f64() < 3.0 * locals_big.as_secs_f64() + 0.5,
        "10k fields {fields_big:?} vs 10k locals {locals_big:?}"
    );
    assert!(
        fields_big.as_secs_f64() < 8.0 * fields_small.as_secs_f64() + 0.5,
        "4x the fields took {fields_big:?} vs {fields_small:?}: super-linear"
    );
}

// ------------------------------------------------------------- determinism

#[test]
fn resolution_is_order_independent() {
    let files = [
        ("a.py", A_SRC),
        ("b.py", "class B(A2):\n    pass\nclass A2:\n    def m(self):\n        return 1\n"),
        ("h.py", "from a import A\n\nclass H(Missing):\n    def __init__(self):\n        self.e = A()\n        self.e = A()\n    def go(self):\n        return self.e.run()\n"),
        ("c.py", "class Base:\n    def m(self):\n        return 1\n\nclass C(Base):\n    def m(self):\n        return super().m()\n"),
        ("d.py", "from a import A\nfrom b import B\n\nclass D:\n    def __init__(self):\n        self.e = A()\n        self.e = B()\n"),
    ];
    let fwd = resolve(&files);
    let mut rev = files;
    rev.reverse();
    let bwd = resolve(&rev);
    let key = |r: &ResolutionResult| {
        let mut v: Vec<String> = r
            .edges
            .iter()
            .map(|e| format!("{:?} {} {}", e.edge_kind, e.source_symbol, e.target_symbol))
            .collect();
        v.sort();
        v
    };
    assert_eq!(key(&fwd), key(&bwd));
}
