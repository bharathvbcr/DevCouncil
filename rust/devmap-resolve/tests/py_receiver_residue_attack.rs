//! Adversarial probes for lane/py-receiver-residue (ft-c0afdfef). Each probe
//! asserts the correct Python behaviour; a failing probe is a finding.
//! Precision probes assert "no wrong edge"; recall probes are labelled.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;
use std::time::Instant;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn calls_from(res: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = res
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == caller)
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out
}

fn reaches(res: &ResolutionResult, caller: &str, target: &str) -> bool {
    calls_from(res, caller).iter().any(|t| t == target)
}

fn no_edge(res: &ResolutionResult, caller: &str, target: &str, what: &str) {
    assert!(
        !reaches(res, caller, target),
        "{what}: wrong edge to {target}; edges = {:?}",
        calls_from(res, caller)
    );
}

fn edge(res: &ResolutionResult, caller: &str, target: &str, what: &str) {
    assert!(
        reaches(res, caller, target),
        "{what}: missing edge to {target}; edges = {:?}",
        calls_from(res, caller)
    );
}

const A_RUN: &str =
    "class A:\n    def run(self):\n        return 1\n    def append(self, x):\n        return 2\n";
const B_RUN: &str = "class B:\n    def run(self):\n        return 2\n";

// =================== (a) call-result receivers ===========================

#[test]
fn a01_namesake_factories_import_picks_the_imported_one() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        (
            "b.py",
            &format!("{B_RUN}\ndef make() -> B:\n    return B()\n"),
        ),
        (
            "app.py",
            "from b import make\n\ndef use():\n    make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a01");
    edge(&res, "app.py::use", "b.py::B.run", "a01");
}

#[test]
fn a02_star_import_last_binding_wins() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        (
            "b.py",
            &format!("{B_RUN}\ndef make() -> B:\n    return B()\n"),
        ),
        (
            "app.py",
            "from a import *\nfrom b import *\n\ndef use():\n    make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a02");
}

#[test]
fn a03_reexport_through_init_recall() {
    let res = resolve(&[
        ("pkg/__init__.py", "from .impl import make\n"),
        (
            "pkg/impl.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        (
            "app.py",
            "from pkg import make\n\ndef use():\n    make().run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "pkg/impl.py::A.run", "a03 (recall)");
}

#[test]
fn a04_union_return_does_not_pick_one_member() {
    let res = resolve(&[
        (
            "a.py",
            &format!("from typing import Union\n{A_RUN}\n{B_RUN}\ndef make() -> Union[A, B]:\n    return A()\n"),
        ),
        ("app.py", "from a import make\n\ndef use():\n    make().run()\n"),
    ]);
    let a = reaches(&res, "app.py::use", "a.py::A.run");
    let b = reaches(&res, "app.py::use", "a.py::B.run");
    assert_eq!(
        a,
        b,
        "a04: Union[A, B] picked one member: {:?}",
        calls_from(&res, "app.py::use")
    );
}

#[test]
fn a04b_list_return_is_not_the_element() {
    for ann in [
        "List[A]",
        "list[A]",
        "Iterator[A]",
        "Type[A]",
        "Callable[[], A]",
        "Awaitable[A]",
        "Dict[str, A]",
    ] {
        let res = resolve(&[
            (
                "a.py",
                &format!("from typing import *\n{A_RUN}\ndef make() -> {ann}:\n    return []\n"),
            ),
            (
                "app.py",
                "from a import make\n\ndef use():\n    make().append(1)\n",
            ),
        ]);
        no_edge(
            &res,
            "app.py::use",
            "a.py::A.append",
            &format!("a04b `-> {ann}`"),
        );
    }
}

#[test]
fn a05_forward_ref_two_configs_caller_imports_the_other() {
    let res = resolve(&[
        (
            "a.py",
            "class Config:\n    def run(self):\n        return 1\n\ndef make() -> \"Config\":\n    return Config()\n",
        ),
        ("b.py", "class Config:\n    def run(self):\n        return 2\n"),
        (
            "app.py",
            "from a import make\nfrom b import Config\n\ndef use():\n    make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "b.py::Config.run", "a05");
}

#[test]
fn a05b_forward_ref_two_configs_caller_declares_its_own() {
    let res = resolve(&[
        (
            "a.py",
            "class Config:\n    def run(self):\n        return 1\n\ndef make() -> \"Config\":\n    return Config()\n",
        ),
        (
            "app.py",
            "from a import make\n\nclass Config:\n    def run(self):\n        return 2\n\ndef use():\n    make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "app.py::Config.run", "a05b");
}

#[test]
fn a06_overloaded_factory_binds_no_single_return() {
    let res = resolve(&[
        (
            "a.py",
            &format!(
                "from typing import overload\n{A_RUN}\n{B_RUN}\n@overload\ndef make(x: int) -> A: ...\n@overload\ndef make(x: str) -> B: ...\ndef make(x):\n    return None\n"
            ),
        ),
        ("app.py", "from a import make\n\ndef use():\n    make('s').run()\n"),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a06 (make('s') is B)");
}

#[test]
fn a07_self_return_on_a_subclass_factory() {
    let res = resolve(&[(
        "m.py",
        "from typing import Self\n\nclass A:\n    @classmethod\n    def make(cls) -> Self:\n        return cls()\n    def run(self):\n        return 1\n\nclass B(A):\n    def run(self):\n        return 2\n\ndef use():\n    B.make().run()\n",
    )]);
    no_edge(&res, "m.py::use", "m.py::A.run", "a07 (B.make() is a B)");
}

#[test]
fn a08_static_factory_recall() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\nclass Factory:\n    @staticmethod\n    def build() -> A:\n        return A()\n"),
        ),
        ("app.py", "from a import Factory\n\ndef use():\n    Factory.build().run()\n"),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "a08 (recall)");
}

#[test]
fn a09_local_lambda_shadows_factory() {
    let res = resolve(&[
        ("a.py", &format!("{A_RUN}\ndef make() -> A:\n    return A()\n")),
        ("b.py", B_RUN),
        (
            "app.py",
            "from a import make\nfrom b import B\n\ndef use():\n    make = lambda: B()\n    make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a09");
}

#[test]
fn a10_factory_name_is_a_parameter() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        (
            "app.py",
            "from a import make\n\ndef use(make):\n    make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a10");
}

#[test]
fn a10b_factory_name_is_a_comprehension_or_lambda_param() {
    let res = resolve(&[
        ("a.py", &format!("{A_RUN}\ndef make() -> A:\n    return A()\n")),
        (
            "app.py",
            "from a import make\n\ndef use(fs):\n    return [make().run() for make in fs]\n",
        ),
        (
            "app2.py",
            "from a import make\n\ndef use2(fs):\n    return list(map(lambda make: make().run(), fs))\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a10b comprehension var");
    no_edge(&res, "app2.py::use2", "a.py::A.run", "a10b lambda param");
}

#[test]
fn a11_module_level_reassignment_of_factory() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        (
            "b.py",
            &format!("{B_RUN}\ndef other() -> B:\n    return B()\n"),
        ),
        (
            "app.py",
            "from a import make\nimport b\n\nmake = b.other\n\ndef use():\n    make().run()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "a.py::A.run",
        "a11 (make rebound to b.other)",
    );
}

#[test]
fn a12_try_except_import_two_factories() {
    let res = resolve(&[
        ("fast.py", &format!("{A_RUN}\ndef make() -> A:\n    return A()\n")),
        ("slow.py", &format!("{B_RUN}\ndef make() -> B:\n    return B()\n")),
        (
            "app.py",
            "try:\n    from fast import make\nexcept ImportError:\n    from slow import make\n\ndef use():\n    make().run()\n",
        ),
    ]);
    let a = reaches(&res, "app.py::use", "fast.py::A.run");
    let b = reaches(&res, "app.py::use", "slow.py::B.run");
    assert_eq!(
        a,
        b,
        "a12: conditional import picked one: {:?}",
        calls_from(&res, "app.py::use")
    );
}

#[test]
fn a13_nested_call_in_args_types_the_outer_factory_only() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make(x) -> A:\n    return A()\n"),
        ),
        (
            "b.py",
            &format!("{B_RUN}\ndef other(x) -> B:\n    return B()\n"),
        ),
        (
            "app.py",
            "from a import make\nfrom b import other\n\ndef use():\n    make(other(1)).run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "a13");
    no_edge(&res, "app.py::use", "b.py::B.run", "a13");
}

#[test]
fn a14_call_of_the_result_is_not_the_result() {
    let res = resolve(&[
        (
            "a.py",
            &format!(
                "{B_RUN}\nclass A:\n    def run(self):\n        return 1\n    def __call__(self, y) -> B:\n        return B()\n\ndef make(x) -> A:\n    return A()\n"
            ),
        ),
        ("app.py", "from a import make\n\ndef use():\n    make(1)(2).run()\n"),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a14 make(1)(2) is B");
}

#[test]
fn a15_subscript_of_the_result_is_not_the_result() {
    let res = resolve(&[
        (
            "a.py",
            &format!(
                "{B_RUN}\nclass A:\n    def run(self):\n        return 1\n    def __getitem__(self, i) -> B:\n        return B()\n\ndef make(x) -> A:\n    return A()\n"
            ),
        ),
        ("app.py", "from a import make\n\ndef use():\n    make(1)[0].run()\n"),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a15 make(1)[0] is B");
}

#[test]
fn a16_or_expression_receiver() {
    let res = resolve(&[
        ("a.py", &format!("{A_RUN}\ndef make() -> A:\n    return A()\n")),
        ("b.py", &format!("{B_RUN}\ndef other() -> B:\n    return B()\n")),
        (
            "app.py",
            "from a import make\nfrom b import other\n\ndef use():\n    (make() or other()).run()\n",
        ),
    ]);
    let a = reaches(&res, "app.py::use", "a.py::A.run");
    let b = reaches(&res, "app.py::use", "b.py::B.run");
    assert_eq!(
        a,
        b,
        "a16: `(f() or g()).m()` picked one side: {:?}",
        calls_from(&res, "app.py::use")
    );
}

#[test]
fn a17_chain_second_hop_is_the_first_hops_return() {
    let res = resolve(&[(
        "m.py",
        "class B:\n    def b(self):\n        return 1\n\nclass A:\n    def a(self) -> B:\n        return B()\n    def b(self):\n        return 2\n\ndef make() -> A:\n    return A()\n\ndef use():\n    make().a().b()\n",
    )]);
    no_edge(&res, "m.py::use", "m.py::A.b", "a17 make().a() is a B");
}

#[test]
fn a18_split_lines() {
    let res = resolve(&[
        ("a.py", &format!("{A_RUN}\ndef make(x, y) -> A:\n    return A()\n")),
        (
            "app.py",
            "from a import make\n\ndef use():\n    return make(\n        1,\n        2,\n    ).run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "a18 (recall)");
}

#[test]
fn a19_constructor_receiver_still_binds() {
    let res = resolve(&[
        ("a.py", A_RUN),
        ("app.py", "from a import A\n\ndef use():\n    A().run()\n"),
    ]);
    edge(
        &res,
        "app.py::use",
        "a.py::A.run",
        "a19 A().run() constructor chain regression",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn a19b_module_qualified_constructor_receiver() {
    let res = resolve(&[
        ("a.py", A_RUN),
        ("app.py", "import a\n\ndef use():\n    a.A().run()\n"),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "a19b a.A().run()");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn a20_self_method_call_result() {
    let res = resolve(&[(
        "m.py",
        "class B:\n    def run(self):\n        return 1\n\nclass C:\n    def make(self) -> B:\n        return B()\n    def go(self):\n        return self.make().run()\n",
    )]);
    edge(
        &res,
        "m.py::C.go",
        "m.py::B.run",
        "a20 self.make().run() regression",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn a21_typed_param_method_call_result() {
    let res = resolve(&[(
        "m.py",
        "class B:\n    def run(self):\n        return 1\n\nclass C:\n    def make(self) -> B:\n        return B()\n\ndef go(c: C):\n    return c.make().run()\n",
    )]);
    edge(
        &res,
        "m.py::go",
        "m.py::B.run",
        "a21 c.make().run() regression",
    );
}

#[test]
fn a22_module_qualified_factory() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        ("app.py", "import a\n\ndef use():\n    a.make().run()\n"),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "a22 (recall)");
}

#[test]
fn a23_module_qualified_factory_local_shadow() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        (
            "b.py",
            &format!("{B_RUN}\ndef make() -> B:\n    return B()\n"),
        ),
        (
            "app.py",
            "import a\nimport b\n\ndef use():\n    a = b\n    a.make().run()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "a.py::A.run",
        "a23 (a rebound to module b)",
    );
}

#[test]
fn a24_instance_variable_named_like_function_is_not_a_call_result() {
    // `make` is an instance of a class with __call__ -> not the module function
    let res = resolve(&[
        ("a.py", &format!("{A_RUN}\ndef make() -> A:\n    return A()\n")),
        (
            "app.py",
            "from a import make\n\ndef use():\n    try:\n        make = int\n    except Exception:\n        pass\n    make().run()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "a.py::A.run",
        "a24 local rebinding in try",
    );
}

#[test]
fn a25_decorated_factory_that_wraps() {
    // decorator returns a wrapper whose result is a list; the annotation is on the inner def
    let res = resolve(&[(
        "m.py",
        "from typing import List\n\nclass A:\n    def run(self):\n        return 1\n    def append(self, x):\n        return 2\n\ndef many(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n\n@many\ndef make() -> A:\n    return A()\n\ndef use():\n    make().append(1)\n",
    )]);
    no_edge(
        &res,
        "m.py::use",
        "m.py::A.append",
        "a25 @many turns make() into List[A]",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn a26_property_is_not_a_factory_call() {
    let res = resolve(&[(
        "m.py",
        "class A:\n    def run(self):\n        return 1\n\nclass H:\n    @property\n    def a(self) -> A:\n        return A()\n\ndef use(h: H):\n    h.a.run()\n",
    )]);
    edge(
        &res,
        "m.py::use",
        "m.py::A.run",
        "a26 (recall; property read)",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn a27_async_factory_awaited_receiver() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\nasync def make() -> A:\n    return A()\n"),
        ),
        (
            "app.py",
            "from a import make\n\nasync def use():\n    return (await make()).run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "a27 (recall)");
}

#[test]
fn a28_async_factory_unawaited_is_a_coroutine() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\nasync def make() -> A:\n    return A()\n"),
        ),
        (
            "app.py",
            "from a import make\n\nasync def use():\n    c = make()\n    return c.close()\n",
        ),
        (
            "app2.py",
            "from a import make\n\ndef use2():\n    return make().close()\n",
        ),
    ]);
    no_edge(
        &res,
        "app2.py::use2",
        "a.py::A.run",
        "a28 unawaited coroutine is no A",
    );
}

#[test]
fn a29_factory_returning_a_class_namesake_of_param_scope() {
    // return annotation names a class the caller file also declares differently, factory in 3rd file
    let res = resolve(&[
        (
            "f.py",
            "class Job:\n    def run(self):\n        return 1\n\ndef make() -> Job:\n    return Job()\n",
        ),
        ("g.py", "class Job:\n    def run(self):\n        return 2\n"),
        (
            "app.py",
            "from f import make\nfrom g import Job as Other\n\ndef use():\n    make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "g.py::Job.run", "a29");
}

#[test]
fn a30_aliased_return_type_collides_with_unrelated_unique_class() {
    // factory.py: `from real import Log as L`; `-> L`. other.py has an unrelated unique `L`.
    let res = resolve(&[
        (
            "real.py",
            "class Log:\n    def write(self):\n        return 1\n",
        ),
        (
            "other.py",
            "class L:\n    def write(self):\n        return 2\n",
        ),
        (
            "factory.py",
            "from real import Log as L\n\ndef make() -> L:\n    return L()\n",
        ),
        (
            "app.py",
            "from factory import make\n\ndef use():\n    make().write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "other.py::L.write",
        "a30 `-> L` is real.Log, not other.L",
    );
}

// =================== (b) class-body field annotations ====================

fn holder(log_ann: &str) -> String {
    format!("from typing import *\nfrom logs import Log\n\nclass Holder:\n    log: {log_ann}\n")
}
const LOGS: &str = "class Log:\n    def write(self):\n        return 1\n    def append(self, x):\n        return 2\n";

#[test]
fn b01_field_read_through_other_file_recall() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef use(h: Holder):\n    h.log.write()\n",
        ),
    ]);
    edge(&res, "app.py::use", "logs.py::Log.write", "b01 (recall)");
}

#[test]
fn b02_field_type_namesake_in_second_file() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("logs2.py", "class Log:\n    def write(self):\n        return 9\n"),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\nfrom logs2 import Log\n\ndef use(h: Holder):\n    h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "logs2.py::Log.write",
        "b02 reader imports the other Log",
    );
}

#[test]
fn b02b_field_type_namesake_unimported_by_reader() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "logs2.py",
            "class Log:\n    def write(self):\n        return 9\n",
        ),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef use(h: Holder):\n    h.log.write()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "logs2.py::Log.write", "b02b");
}

#[test]
fn b03_field_type_alias_collides_with_unrelated_unique_class() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "other.py",
            "class L:\n    def write(self):\n        return 2\n",
        ),
        (
            "holder.py",
            "from logs import Log as L\n\nclass Holder:\n    log: L\n",
        ),
        (
            "app.py",
            "from holder import Holder\n\ndef use(h: Holder):\n    h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "other.py::L.write",
        "b03 `log: L` is logs.Log",
    );
}

#[test]
fn b04_container_annotations_are_not_the_element() {
    for ann in [
        "List[Log]",
        "list[Log]",
        "Dict[str, Log]",
        "Set[Log]",
        "Iterable[Log]",
        "Type[Log]",
        "Callable[[], Log]",
        "ClassVar[List[Log]]",
        "Final[List[Log]]",
    ] {
        let res = resolve(&[
            ("logs.py", LOGS),
            ("holder.py", &holder(ann)),
            (
                "app.py",
                "from holder import Holder\n\ndef use(h: Holder):\n    h.log.append(1)\n",
            ),
        ]);
        no_edge(
            &res,
            "app.py::use",
            "logs.py::Log.append",
            &format!("b04 `log: {ann}`"),
        );
    }
}

#[test]
fn b05_union_field_does_not_pick_one() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("logs3.py", "class Log3:\n    def write(self):\n        return 3\n"),
        ("holder.py", "from typing import Union\nfrom logs import Log\nfrom logs3 import Log3\n\nclass Holder:\n    log: Union[Log, Log3]\n"),
        ("app.py", "from holder import Holder\n\ndef use(h: Holder):\n    h.log.write()\n"),
    ]);
    let a = reaches(&res, "app.py::use", "logs.py::Log.write");
    let b = reaches(&res, "app.py::use", "logs3.py::Log3.write");
    assert_eq!(a, b, "b05: {:?}", calls_from(&res, "app.py::use"));
}

#[test]
fn b06_subclass_redefines_field_with_a_different_type() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "logs3.py",
            "class Log3:\n    def write(self):\n        return 3\n",
        ),
        (
            "base.py",
            "from logs import Log\n\nclass Base:\n    log: Log\n",
        ),
        (
            "sub.py",
            "from base import Base\nfrom logs3 import Log3\n\nclass Sub(Base):\n    log: Log3\n",
        ),
        (
            "app.py",
            "from sub import Sub\n\ndef use(s: Sub):\n    s.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "logs.py::Log.write",
        "b06 Sub.log is Log3",
    );
    edge(&res, "app.py::use", "logs3.py::Log3.write", "b06 (recall)");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn b07_base_field_read_through_subclass_recall() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "base.py",
            "from logs import Log\n\nclass Base:\n    log: Log\n",
        ),
        (
            "sub.py",
            "from base import Base\n\nclass Sub(Base):\n    pass\n",
        ),
        (
            "app.py",
            "from sub import Sub\n\ndef use(s: Sub):\n    s.log.write()\n",
        ),
    ]);
    edge(&res, "app.py::use", "logs.py::Log.write", "b07 (recall)");
}

#[test]
fn b08_dataclass_vs_plain_and_slots() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "holder.py",
            "from dataclasses import dataclass\nfrom logs import Log\n\n@dataclass\nclass D:\n    log: Log\n\nclass S:\n    __slots__ = ('log',)\n    log: Log\n",
        ),
        (
            "app.py",
            "from holder import D, S\n\ndef use_d(h: D):\n    h.log.write()\n\ndef use_s(h: S):\n    h.log.write()\n",
        ),
    ]);
    edge(
        &res,
        "app.py::use_d",
        "logs.py::Log.write",
        "b08 dataclass (recall)",
    );
    edge(
        &res,
        "app.py::use_s",
        "logs.py::Log.write",
        "b08 slots (recall)",
    );
}

#[test]
fn b09_classvar_and_annotation_without_value_and_protocol() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "holder.py",
            "from typing import ClassVar, Protocol\nfrom logs import Log\n\nclass C:\n    log: ClassVar[Log]\n\nclass P(Protocol):\n    log: Log\n",
        ),
        (
            "app.py",
            "from holder import C, P\n\ndef use_c(h: C):\n    h.log.write()\n\ndef use_p(h: P):\n    h.log.write()\n",
        ),
    ]);
    edge(
        &res,
        "app.py::use_p",
        "logs.py::Log.write",
        "b09 Protocol field (recall)",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn b10_type_checking_import_in_owner() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "holder.py",
            "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from logs import Log\n\nclass Holder:\n    log: \"Log\"\n",
        ),
        ("app.py", "from holder import Holder\n\ndef use(h: Holder):\n    h.log.write()\n"),
    ]);
    edge(&res, "app.py::use", "logs.py::Log.write", "b10 (recall)");
}

#[test]
fn b11_two_holders_same_class_name_in_two_files() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "logs3.py",
            "class Log3:\n    def write(self):\n        return 3\n",
        ),
        (
            "h1.py",
            "from logs import Log\n\nclass Holder:\n    log: Log\n",
        ),
        (
            "h2.py",
            "from logs3 import Log3\n\nclass Holder:\n    log: Log3\n",
        ),
        (
            "app.py",
            "from h2 import Holder\n\ndef use(h: Holder):\n    h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "logs.py::Log.write",
        "b11 Holder is h2.Holder",
    );
}

#[test]
fn b11b_two_holders_reader_binds_local_name_to_one() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "logs3.py",
            "class Log3:\n    def write(self):\n        return 3\n",
        ),
        (
            "h1.py",
            "from logs import Log\n\nclass Holder:\n    log: Log\n",
        ),
        (
            "h2.py",
            "from logs3 import Log3\n\nclass Holder:\n    log: Log3\n",
        ),
        (
            "app.py",
            "from h1 import Holder\nimport h2\n\ndef use(h: Holder):\n    h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "logs3.py::Log3.write",
        "b11b Holder is h1.Holder",
    );
}

#[test]
fn b12_param_shadow_does_not_read_other_functions_declared_type() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef typed(h: Holder):\n    return h.log.write()\n\ndef untyped(h):\n    return h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::untyped",
        "logs.py::Log.write",
        "b12 untyped param named like another fn's typed param",
    );
}

#[test]
fn b13_local_from_factory_is_the_functions_own() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        ("other.py", "class Other:\n    def write(self):\n        return 1\n"),
        (
            "app.py",
            "from holder import Holder\nfrom other import Other\n\ndef typed(h: Holder):\n    return h.log.write()\n\ndef rebound():\n    h = Other()\n    return h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::rebound",
        "logs.py::Log.write",
        "b13 local h is Other",
    );
}

#[test]
fn b14_for_loop_and_with_binding_shadow() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef typed(h: Holder):\n    return h.log.write()\n\ndef loop(items):\n    for h in items:\n        h.log.write()\n\ndef ctx(m):\n    with m() as h:\n        h.log.write()\n",
        ),
    ]);
    no_edge(&res, "app.py::loop", "logs.py::Log.write", "b14 for var");
    no_edge(&res, "app.py::ctx", "logs.py::Log.write", "b14 with var");
}

#[test]
fn b15_field_through_factory_local_namesake_function() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef make() -> Holder:\n    return Holder()\n\ndef a():\n    r = make()\n    return r.log.write()\n\ndef b(r):\n    return r.log.write()\n",
        ),
    ]);
    edge(&res, "app.py::a", "logs.py::Log.write", "b15 (recall)");
    no_edge(
        &res,
        "app.py::b",
        "logs.py::Log.write",
        "b15 untyped r in b",
    );
}

#[test]
fn b16_field_annotation_inside_method_scope_class() {
    // a class declared inside a function with a same-named class at file level
    let res = resolve(&[
        ("logs.py", LOGS),
        ("other.py", "class Other:\n    def write(self):\n        return 1\n"),
        (
            "app.py",
            "from logs import Log\nfrom other import Other\n\nclass Holder:\n    log: Log\n\ndef use():\n    class Holder:\n        log: Other\n    h = Holder()\n    return h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "logs.py::Log.write",
        "b16 local Holder.log is Other",
    );
}

// =================== (c) 3-segment module paths ==========================

fn tree(extra: &[(&'static str, &'static str)], app: &'static str) -> ResolutionResult {
    let mut files: Vec<(&str, &str)> = vec![
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("app.py", app),
    ];
    files.extend_from_slice(extra);
    resolve(&files)
}

#[test]
fn c01_three_segment_recall() {
    let res = tree(&[], "import a.b.c\n\ndef use():\n    a.b.c.f()\n");
    edge(&res, "app.py::use", "a/b/c.py::f", "c01 (recall)");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn c02_function_level_import_recall() {
    let res = tree(&[], "def use():\n    import a.b.c\n    a.b.c.f()\n");
    edge(&res, "app.py::use", "a/b/c.py::f", "c02 (recall)");
}

#[test]
fn c03_two_segment_import_does_not_reach_three() {
    let res = tree(&[], "import a.b\n\ndef use():\n    a.b.c.f()\n");
    no_edge(
        &res,
        "app.py::use",
        "a/b/c.py::f",
        "c03 submodule never imported",
    );
}

#[test]
fn c03b_two_segment_import_where_c_is_a_variable() {
    let res = resolve(&[
        ("a/__init__.py", ""),
        (
            "a/b/__init__.py",
            "class Obj:\n    def f(self):\n        return 2\n\nc = Obj()\n",
        ),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("app.py", "import a.b\n\ndef use():\n    a.b.c.f()\n"),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "a/b/c.py::f",
        "c03b a.b.c is the Obj instance",
    );
}

#[test]
fn c04_namespace_package_recall() {
    let res = resolve(&[
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("app.py", "import a.b.c\n\ndef use():\n    a.b.c.f()\n"),
    ]);
    edge(&res, "app.py::use", "a/b/c.py::f", "c04 (recall)");
}

#[test]
fn c05_namespace_ambiguity_two_roots() {
    let res = resolve(&[
        ("src/a/b/c.py", "def f():\n    return 1\n"),
        ("lib/a/b/c.py", "def f():\n    return 2\n"),
        ("app.py", "import a.b.c\n\ndef use():\n    a.b.c.f()\n"),
    ]);
    no_edge(&res, "app.py::use", "src/a/b/c.py::f", "c05 ambiguous src");
    no_edge(&res, "app.py::use", "lib/a/b/c.py::f", "c05 ambiguous lib");
}

#[test]
fn c06_module_level_rebind_after_import() {
    let res = tree(
        &[("o.py", "class O:\n    def f(self):\n        return 1\n")],
        "import a.b.c\nfrom o import O\n\na = O()\n\ndef use():\n    a.b.c.f()\n",
    );
    no_edge(
        &res,
        "app.py::use",
        "a/b/c.py::f",
        "c06 a rebound at module level",
    );
}

#[test]
fn c06b_rebind_by_for_with_except_and_def() {
    for (label, tail) in [
        ("for", "for a in []:\n    pass\n"),
        ("with", "with open('x') as a:\n    pass\n"),
        ("def", "def a():\n    pass\n"),
        ("class", "class a:\n    pass\n"),
        ("del", "del a\n"),
        ("walrus", "(a := 3)\n"),
        ("import-as", "import os as a\n"),
    ] {
        let src = format!("import a.b.c\n{tail}\ndef use():\n    a.b.c.f()\n");
        let res = {
            let files: Vec<(&str, &str)> = vec![
                ("a/__init__.py", ""),
                ("a/b/__init__.py", ""),
                ("a/b/c.py", "def f():\n    return 1\n"),
                ("app.py", &src),
            ];
            resolve(&files)
        };
        no_edge(
            &res,
            "app.py::use",
            "a/b/c.py::f",
            &format!("c06b rebind via {label}"),
        );
    }
}

#[test]
fn c07_parameter_and_lambda_and_comprehension_named_like_root() {
    let res = tree(
        &[],
        "import a.b.c\n\ndef p(a):\n    return a.b.c.f()\n\ndef l(xs):\n    return list(map(lambda a: a.b.c.f(), xs))\n\ndef comp(xs):\n    return [a.b.c.f() for a in xs]\n",
    );
    no_edge(&res, "app.py::p", "a/b/c.py::f", "c07 param");
    no_edge(&res, "app.py::l", "a/b/c.py::f", "c07 lambda param");
    no_edge(&res, "app.py::comp", "a/b/c.py::f", "c07 comprehension var");
}

#[test]
fn c08_function_local_rebind_of_root() {
    let res = tree(
        &[("o.py", "class O:\n    def f(self):\n        return 1\n")],
        "import a.b.c\nfrom o import O\n\ndef use():\n    a = O()\n    a.b.c.f()\n",
    );
    no_edge(&res, "app.py::use", "a/b/c.py::f", "c08");
}

#[test]
fn c09_conditional_import_other_branch_binds_root_differently() {
    let res = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("z.py", "def g():\n    return 3\n"),
        (
            "app.py",
            "try:\n    import a.b.c\nexcept ImportError:\n    import z as a\n\ndef use():\n    a.b.c.f()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "a/b/c.py::f",
        "c09 `import z as a` is the other binding",
    );
}

#[test]
fn c10_alias_form_and_from_form_recall() {
    let res = tree(
        &[],
        "import a.b.c as x\nfrom a.b import c\n\ndef u1():\n    x.f()\n\ndef u2():\n    c.f()\n",
    );
    edge(&res, "app.py::u1", "a/b/c.py::f", "c10 as-form (recall)");
    edge(&res, "app.py::u2", "a/b/c.py::f", "c10 from-form (recall)");
}

#[test]
fn c11_third_party_name_collides_with_vendored_tree() {
    // `a` is not importable from app's tree: only a test-data copy exists.
    let res = resolve(&[
        ("tests/data/a/b/c.py", "def f():\n    return 1\n"),
        ("src/app.py", "import a.b.c\n\ndef use():\n    a.b.c.f()\n"),
    ]);
    no_edge(
        &res,
        "src/app.py::use",
        "tests/data/a/b/c.py::f",
        "c11 unrelated tree",
    );
}

#[test]
fn c12_three_segment_wrong_member_not_declared() {
    let res = tree(&[], "import a.b.c\n\ndef use():\n    a.b.c.nope()\n");
    assert!(
        calls_from(&res, "app.py::use").is_empty(),
        "c12 {:?}",
        calls_from(&res, "app.py::use")
    );
}

#[test]
fn c13_package_c_with_init_and_module_namesake() {
    // both a/b/c.py and a/b/c/__init__.py: Python prefers the package
    let res = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("a/b/c/__init__.py", "def f():\n    return 2\n"),
        ("app.py", "import a.b.c\n\ndef use():\n    a.b.c.f()\n"),
    ]);
    no_edge(
        &res,
        "app.py::use",
        "a/b/c.py::f",
        "c13 package shadows module",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn c14_member_is_a_class_not_a_function_call() {
    let res = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        (
            "a/b/c.py",
            "class K:\n    def run(self):\n        return 1\n\ndef f():\n    return 1\n",
        ),
        (
            "app.py",
            "import a.b.c\n\ndef use():\n    a.b.c.K().run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "a/b/c.py::K.run", "c14 (recall)");
}

// =================== scale / adversarial size ============================

#[test]
fn s01_thousands_of_factories_bounded_and_correct() {
    let n = 3000;
    let mut src = String::new();
    for i in 0..n {
        src.push_str(&format!(
            "class C{i}:\n    def run(self):\n        return {i}\n\ndef make{i}() -> C{i}:\n    return C{i}()\n\n"
        ));
    }
    for i in 0..n {
        src.push_str(&format!("def use{i}():\n    return make{i}().run()\n\n"));
    }
    let t = Instant::now();
    let res = resolve(&[("big.py", &src)]);
    let took = t.elapsed();
    eprintln!("S01_ELAPSED_MS {}", took.as_millis());
    let mut hit = 0;
    for i in 0..n {
        if reaches(
            &res,
            &format!("big.py::use{i}"),
            &format!("big.py::C{i}.run"),
        ) {
            hit += 1;
        }
        for j in [0usize, 7] {
            if j != i % 8 && i + j < n {
                let wrong = format!("big.py::C{}.run", i + j + 1);
                assert!(!reaches(&res, &format!("big.py::use{i}"), &wrong));
            }
        }
    }
    assert_eq!(hit, n, "only {hit}/{n} resolved");
    assert!(took.as_secs() < 120, "took {took:?}");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn s02_recursive_factory_return_types_terminate() {
    let mut chain = String::from("A.make()");
    for _ in 0..200 {
        chain.push_str(".make()");
    }
    let src = format!(
        "class A:\n    @staticmethod\n    def make() -> \"B\":\n        return B()\n    def run(self):\n        return 1\n\nclass B:\n    @staticmethod\n    def make() -> A:\n        return A()\n    def run(self):\n        return 2\n\ndef use():\n    return {chain}.run()\n"
    );
    let t = Instant::now();
    let res = resolve(&[("m.py", &src)]);
    eprintln!("S02_ELAPSED_MS {}", t.elapsed().as_millis());
    // 201 makes: A.make -> B, B.make -> A ... the 201st result is B (odd count)
    no_edge(&res, "m.py::use", "m.py::A.run", "s02 201 hops lands on B");
    assert!(t.elapsed().as_secs() < 60);
}

#[test]
fn s03_deep_chain_of_instance_methods() {
    let mut chain = String::from("make()");
    for _ in 0..300 {
        chain.push_str(".a()");
    }
    let src = format!(
        "class A:\n    def a(self) -> \"A\":\n        return self\n    def run(self):\n        return 1\n\ndef make() -> A:\n    return A()\n\ndef use():\n    return {chain}.run()\n"
    );
    let t = Instant::now();
    let res = resolve(&[("m.py", &src)]);
    eprintln!("S03_ELAPSED_MS {}", t.elapsed().as_millis());
    assert!(t.elapsed().as_secs() < 60);
    let _ = res;
}

#[test]
fn s04_many_calls_sharing_a_start_offset() {
    // one 2000-hop chain gives 2000 calls at the same start offset: quadratic scan
    let mut chain = String::from("make()");
    for _ in 0..2000 {
        chain.push_str(".a()");
    }
    let src = format!(
        "class A:\n    def a(self) -> \"A\":\n        return self\n\ndef make() -> A:\n    return A()\n\ndef use():\n    return {chain}\n"
    );
    let t = Instant::now();
    let extractions = vec![extract_file("m.py", &src)];
    let ex = t.elapsed();
    let t2 = Instant::now();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let _ = resolver.resolve_all(&extractions).unwrap();
    eprintln!(
        "S04_EXTRACT_MS {} RESOLVE_MS {}",
        ex.as_millis(),
        t2.elapsed().as_millis()
    );
    assert!(
        t2.elapsed().as_secs() < 20,
        "resolve took {:?}",
        t2.elapsed()
    );
}

// =================== regression harness: edge dump =======================

/// Dump every call/import edge for each corpus directory to
/// `$DEVMAP_EDGE_DUMP`; a no-op when the variable is unset.
#[test]
fn z_edge_dump_for_diffing() {
    let Ok(out) = std::env::var("DEVMAP_EDGE_DUMP") else {
        return;
    };
    let root = std::env::var("DEVMAP_DUMP_ROOT").expect("DEVMAP_DUMP_ROOT");
    let mut lines: Vec<String> = Vec::new();
    for corpus in std::fs::read_dir(&root).unwrap().flatten() {
        if !corpus.path().is_dir() {
            continue;
        }
        let mut files: Vec<(String, String)> = Vec::new();
        walk(&corpus.path(), &corpus.path(), &mut files);
        files.sort();
        let extractions: Vec<Extraction> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let res = resolver.resolve_all(&extractions).unwrap();
        let name = corpus.file_name().to_string_lossy().to_string();
        for e in &res.edges {
            lines.push(format!(
                "{name}\t{:?}\t{}\t{}\t{}\t{}\t{:?}",
                e.edge_kind,
                e.source_symbol,
                e.target_symbol,
                e.source_file,
                e.target_file,
                e.confidence
            ));
        }
        for u in &res.unresolved {
            lines.push(format!(
                "{name}\tUNRES\t{}\t{}",
                u.source_symbol, u.callee_name
            ));
        }
    }
    lines.sort();
    std::fs::write(out, lines.join("\n")).unwrap();
}

fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(base, &p, out);
        } else if let Ok(text) = std::fs::read_to_string(&p) {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
            out.push((rel, text));
        }
    }
}

// =================== second round ========================================

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn a21b_local_instance_method_call_result() {
    let res = resolve(&[(
        "m.py",
        "class B:\n    def run(self):\n        return 1\n\nclass C:\n    def make(self) -> B:\n        return B()\n\ndef go():\n    c = C()\n    return c.make().run()\n",
    )]);
    edge(
        &res,
        "m.py::go",
        "m.py::B.run",
        "a21b local c = C(); c.make().run() regression",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn a21c_module_global_instance_method_call_result() {
    let res = resolve(&[(
        "m.py",
        "class B:\n    def run(self):\n        return 1\n\nclass C:\n    def make(self) -> B:\n        return B()\n\nc: C = C()\n\ndef go():\n    return c.make().run()\n",
    )]);
    edge(
        &res,
        "m.py::go",
        "m.py::B.run",
        "a21c global c: C; c.make().run() regression",
    );
}

#[test]
fn a25b_decorated_factory_assignment_form_classifies_preexisting() {
    let res = resolve(&[(
        "m.py",
        "from typing import List\n\nclass A:\n    def run(self):\n        return 1\n    def append(self, x):\n        return 2\n\ndef many(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n\n@many\ndef make() -> A:\n    return A()\n\ndef use():\n    x = make()\n    x.append(1)\n",
    )]);
    no_edge(
        &res,
        "m.py::use",
        "m.py::A.append",
        "a25b x = make() with @many",
    );
}

#[test]
fn a30b_aliased_return_type_assignment_form_classifies_preexisting() {
    let res = resolve(&[
        (
            "real.py",
            "class Log:\n    def write(self):\n        return 1\n",
        ),
        (
            "other.py",
            "class L:\n    def write(self):\n        return 2\n",
        ),
        (
            "factory.py",
            "from real import Log as L\n\ndef make() -> L:\n    return L()\n",
        ),
        (
            "app.py",
            "from factory import make\n\ndef use():\n    x = make()\n    x.write()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "other.py::L.write", "a30b x = make()");
}

#[test]
fn a02b_star_import_assignment_form_classifies_preexisting() {
    let res = resolve(&[
        (
            "a.py",
            &format!("{A_RUN}\ndef make() -> A:\n    return A()\n"),
        ),
        (
            "b.py",
            &format!("{B_RUN}\ndef make() -> B:\n    return B()\n"),
        ),
        (
            "app.py",
            "from a import *\nfrom b import *\n\ndef use():\n    x = make()\n    x.run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a02b x = make() star");
}

#[test]
fn a33_nested_def_shadows_factory() {
    let res = resolve(&[
        ("a.py", &format!("{A_RUN}\ndef make() -> A:\n    return A()\n")),
        ("b.py", B_RUN),
        (
            "app.py",
            "from a import make\nfrom b import B\n\ndef use():\n    def make() -> B:\n        return B()\n    return make().run()\n",
        ),
    ]);
    no_edge(&res, "app.py::use", "a.py::A.run", "a33");
}

#[test]
fn a34_conditional_defs_of_the_factory() {
    let res = resolve(&[(
        "m.py",
        "import sys\n\nclass A:\n    def run(self):\n        return 1\n\nclass B:\n    def run(self):\n        return 2\n\nif sys.platform == 'win32':\n    def make() -> A:\n        return A()\nelse:\n    def make() -> B:\n        return B()\n\ndef use():\n    return make().run()\n",
    )]);
    let a = reaches(&res, "m.py::use", "m.py::A.run");
    let b = reaches(&res, "m.py::use", "m.py::B.run");
    assert_eq!(a, b, "a34: {:?}", calls_from(&res, "m.py::use"));
}

#[test]
fn b12b_same_file_untyped_param_classifies_preexisting() {
    let res = resolve(&[(
        "m.py",
        "class Log:\n    def write(self):\n        return 1\n\nclass Holder:\n    log: Log\n\ndef typed(h: Holder):\n    return h.log.write()\n\ndef untyped(h):\n    return h.log.write()\n",
    )]);
    no_edge(&res, "m.py::untyped", "m.py::Log.write", "b12b same file");
}

#[test]
fn b12c_untyped_method_param_other_file_holder() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef typed(h: Holder):\n    return 1\n\nclass Svc:\n    def go(self, h):\n        return h.log.write()\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::Svc.go",
        "logs.py::Log.write",
        "b12c untyped method param h",
    );
}

#[test]
fn b14b_with_var_alone() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef typed(h: Holder):\n    return h.log.write()\n\ndef ctx(m):\n    with m() as h:\n        h.log.write()\n",
        ),
    ]);
    no_edge(&res, "app.py::ctx", "logs.py::Log.write", "b14b with var");
}

#[test]
fn b14c_comprehension_var() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", &holder("Log")),
        (
            "app.py",
            "from holder import Holder\n\ndef typed(h: Holder):\n    return h.log.write()\n\ndef comp(xs):\n    return [h.log.write() for h in xs]\n",
        ),
    ]);
    no_edge(
        &res,
        "app.py::comp",
        "logs.py::Log.write",
        "b14c comprehension var",
    );
}

fn c06b_case(label: &str, tail: &str) {
    let src = format!("import a.b.c\n{tail}\ndef use():\n    a.b.c.f()\n");
    let files: Vec<(&str, &str)> = vec![
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("app.py", &src),
    ];
    let res = resolve(&files);
    no_edge(
        &res,
        "app.py::use",
        "a/b/c.py::f",
        &format!("c06b rebind via {label}"),
    );
}

#[test]
fn c06c_rebind_for() {
    c06b_case("for", "for a in []:\n    pass\n");
}
#[test]
fn c06d_rebind_with() {
    c06b_case("with", "with open('x') as a:\n    pass\n");
}
#[test]
fn c06e_rebind_def() {
    c06b_case("def", "def a():\n    pass\n");
}
#[test]
fn c06f_rebind_class() {
    c06b_case("class", "class a:\n    pass\n");
}
#[test]
fn c06g_rebind_del() {
    c06b_case("del", "del a\n");
}
#[test]
fn c06h_rebind_walrus() {
    c06b_case("walrus", "(a := 3)\n");
}
#[test]
fn c06i_rebind_import_as() {
    c06b_case("import-as", "import os as a\n");
}
#[test]
fn c06j_rebind_try_except_as() {
    c06b_case(
        "except-as",
        "try:\n    pass\nexcept Exception as a:\n    pass\n",
    );
}
#[test]
fn c06k_rebind_assign_later() {
    c06b_case("later-assign", "a = object()\n");
}
#[test]
fn c06l_rebind_from_import() {
    c06b_case("from-import", "from os import path as a\n");
}

#[test]
fn c15_global_declaration_rebind_in_other_function() {
    let res = tree(
        &[("o.py", "class O:\n    def f(self):\n        return 1\n")],
        "import a.b.c\nfrom o import O\n\ndef setup():\n    global a\n    a = O()\n\ndef use():\n    a.b.c.f()\n",
    );
    no_edge(
        &res,
        "app.py::use",
        "a/b/c.py::f",
        "c15 global a rebound in setup()",
    );
}

#[test]
fn s05_many_dotted_calls_times_many_imports() {
    let mut src = String::new();
    for i in 0..2000 {
        src.push_str(&format!("import pk{i}.sub.mod\n"));
    }
    src.push_str("\ndef use(o):\n");
    for _ in 0..5000 {
        src.push_str("    o.attr.method()\n");
    }
    let t = Instant::now();
    let _ = resolve(&[("big.py", &src)]);
    eprintln!("S05_ELAPSED_MS {}", t.elapsed().as_millis());
    assert!(t.elapsed().as_secs() < 60);
}

#[test]
fn s06_many_dotted_module_calls_resolve_cost() {
    let mut files: Vec<(String, String)> = vec![];
    let mut src = String::new();
    for i in 0..300 {
        files.push((format!("pk{i}/__init__.py"), String::new()));
        files.push((format!("pk{i}/sub/__init__.py"), String::new()));
        files.push((
            format!("pk{i}/sub/mod.py"),
            "def f():\n    return 1\n".to_string(),
        ));
        src.push_str(&format!("import pk{i}.sub.mod\n"));
    }
    src.push_str("\ndef use():\n");
    for i in 0..300 {
        for _ in 0..10 {
            src.push_str(&format!("    pk{i}.sub.mod.f()\n"));
        }
    }
    files.push(("app.py".to_string(), src));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let t = Instant::now();
    let res = resolve(&refs);
    eprintln!("S06_ELAPSED_MS {}", t.elapsed().as_millis());
    assert!(t.elapsed().as_secs() < 60);
    edge(&res, "app.py::use", "pk0/sub/mod.py::f", "s06 (recall)");
}

/// Characterisation: print the tier of the wrong edges the probes above catch.
#[test]
fn z_confidence_report() {
    let cases = vec![
        (
            "b12 untyped param",
            vec![
                ("logs.py", LOGS.to_string()),
                ("holder.py", holder("Log")),
                ("app.py", "from holder import Holder\n\ndef typed(h: Holder):\n    return h.log.write()\n\ndef untyped(h):\n    return h.log.write()\n".to_string()),
            ],
            "app.py::untyped",
        ),
        (
            "a30 alias",
            vec![
                ("real.py", "class Log:\n    def write(self):\n        return 1\n".to_string()),
                ("other.py", "class L:\n    def write(self):\n        return 2\n".to_string()),
                ("factory.py", "from real import Log as L\n\ndef make() -> L:\n    return L()\n".to_string()),
                ("app.py", "from factory import make\n\ndef use():\n    make().write()\n".to_string()),
            ],
            "app.py::use",
        ),
        (
            "c05 ambiguous roots",
            vec![
                ("src/a/b/c.py", "def f():\n    return 1\n".to_string()),
                ("lib/a/b/c.py", "def f():\n    return 2\n".to_string()),
                ("app.py", "import a.b.c\n\ndef use():\n    a.b.c.f()\n".to_string()),
            ],
            "app.py::use",
        ),
        (
            "c06c for rebind",
            vec![
                ("a/__init__.py", String::new()),
                ("a/b/__init__.py", String::new()),
                ("a/b/c.py", "def f():\n    return 1\n".to_string()),
                ("app.py", "import a.b.c\nfor a in []:\n    pass\n\ndef use():\n    a.b.c.f()\n".to_string()),
            ],
            "app.py::use",
        ),
        (
            "a02 star",
            vec![
                ("a.py", format!("{A_RUN}\ndef make() -> A:\n    return A()\n")),
                ("b.py", format!("{B_RUN}\ndef make() -> B:\n    return B()\n")),
                ("app.py", "from a import *\nfrom b import *\n\ndef use():\n    make().run()\n".to_string()),
            ],
            "app.py::use",
        ),
    ];
    for (label, files, caller) in cases {
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (*a, b.as_str())).collect();
        let res = resolve(&refs);
        for e in res
            .edges
            .iter()
            .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == caller)
        {
            eprintln!(
                "CONF {label}: {} -> {} {:?}",
                e.source_symbol, e.target_symbol, e.confidence
            );
        }
    }
}

fn ab_hops(hops: usize) -> String {
    let mut chain = String::from("A.make()");
    for _ in 1..hops {
        chain.push_str(".make()");
    }
    format!(
        "class A:\n    @staticmethod\n    def make() -> \"B\":\n        return B()\n    def run(self):\n        return 1\n\nclass B:\n    @staticmethod\n    def make() -> A:\n        return A()\n    def run(self):\n        return 2\n\ndef use():\n    return {chain}.run()\n"
    )
}

#[test]
fn t01_one_hop_lands_on_b() {
    let res = resolve(&[("m.py", &ab_hops(1))]);
    no_edge(
        &res,
        "m.py::use",
        "m.py::A.run",
        "t01 A.make().run() is a B",
    );
}

#[test]
fn t02_two_hops_land_on_a() {
    let res = resolve(&[("m.py", &ab_hops(2))]);
    no_edge(
        &res,
        "m.py::use",
        "m.py::B.run",
        "t02 A.make().make().run() is an A",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up"]
fn t03_three_hops_land_on_b() {
    let res = resolve(&[("m.py", &ab_hops(3))]);
    no_edge(&res, "m.py::use", "m.py::A.run", "t03 three hops is a B");
}
