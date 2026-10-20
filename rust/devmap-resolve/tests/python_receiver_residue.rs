//! Python receiver typing residue (ft-c0afdfef): `f(x).m()` chains, a class-body
//! field annotation read through another file, 3-segment module paths, and the
//! two constructs that stay honest misses because the extractor does not yet
//! say enough (tuple unpacking, pytest fixtures).
//!
//! Every positive test here names the pre-change behaviour it fixes; every
//! negative test is a precision guard: a new typing rule may add a binding, but
//! it must never add a wrong one.

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

fn unresolved_call(res: &ResolutionResult, caller: &str, callee: &str) -> bool {
    res.unresolved
        .iter()
        .any(|row| row.source_symbol == caller && row.callee_name == callee)
}

const TABLE: &str = "class ZqTable:\n    def digest_of(self):\n        return 'd'\n\n    def cache_clear(self):\n        return None\n\n\ndef make_table(keys) -> ZqTable:\n    return ZqTable()\n";

// ------------------------------------------------------------ f(x).m() chains

/// py.Z3. `make_table([1]).digest_of()`: the receiver the extractor records is
/// the callee text `make_table`, so the call is only told from
/// `make_table.digest_of()` by the inner call that starts where it starts.
/// Fails at ec034f60: no edge.
#[test]
fn z3_call_result_binds_the_return_annotated_class() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef use_call_result():\n    return make_table([1]).digest_of()\n",
        ),
    ]);
    assert!(
        reaches(
            &res,
            "pkg/calls.py::use_call_result",
            "pkg/base.py::ZqTable.digest_of"
        ),
        "{:?}",
        calls_from(&res, "pkg/calls.py::use_call_result")
    );
}

/// py.Z3.not. `make_table.cache_clear()` reads an attribute of the function
/// object (`functools.lru_cache`), not of the `ZqTable` it returns.
#[test]
fn z3_function_attribute_is_not_a_member_of_the_result() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef use_function_attribute():\n    return make_table.cache_clear()\n",
        ),
    ]);
    assert!(
        !reaches(
            &res,
            "pkg/calls.py::use_function_attribute",
            "pkg/base.py::ZqTable.cache_clear"
        ),
        "{:?}",
        calls_from(&res, "pkg/calls.py::use_function_attribute")
    );
}

/// Both spellings in one function: only the call-result one binds.
#[test]
fn z3_the_two_spellings_in_one_caller_are_told_apart() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef both():\n    make_table.cache_clear()\n    return make_table(1).digest_of()\n",
        ),
    ]);
    let got = calls_from(&res, "pkg/calls.py::both");
    assert!(got.iter().any(|t| t == "pkg/base.py::ZqTable.digest_of"), "{got:?}");
    assert!(!got.iter().any(|t| t == "pkg/base.py::ZqTable.cache_clear"), "{got:?}");
}

/// A call argument that is itself a call of the same name starts later than
/// the outer call and is not the receiver.
#[test]
fn z3_a_same_named_call_in_the_arguments_is_not_the_receiver() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef arg_only():\n    return make_table.cache_clear(make_table(2))\n",
        ),
    ]);
    assert!(
        !reaches(&res, "pkg/calls.py::arg_only", "pkg/base.py::ZqTable.cache_clear"),
        "{:?}",
        calls_from(&res, "pkg/calls.py::arg_only")
    );
}

#[test]
fn z3_an_unannotated_factory_binds_nothing() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/base.py",
            "class ZqTable:\n    def digest_of(self):\n        return 'd'\n\n\ndef make_table(keys):\n    return ZqTable()\n",
        ),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef use():\n    return make_table([1]).digest_of()\n",
        ),
    ]);
    assert!(
        !reaches(&res, "pkg/calls.py::use", "pkg/base.py::ZqTable.digest_of"),
        "{:?}",
        calls_from(&res, "pkg/calls.py::use")
    );
}

#[test]
fn z3_a_generic_or_optional_return_binds_nothing() {
    for annotation in ["list[ZqTable]", "Optional[ZqTable]", "ZqTable | None", "tuple[ZqTable, int]"] {
        let base = format!(
            "from typing import Optional\n\n\nclass ZqTable:\n    def digest_of(self):\n        return 'd'\n\n\ndef make_table(keys) -> {annotation}:\n    return []\n"
        );
        let res = resolve(&[
            ("pkg/__init__.py", ""),
            ("pkg/base.py", &base),
            (
                "pkg/calls.py",
                "from .base import make_table\n\n\ndef use():\n    return make_table([1]).digest_of()\n",
            ),
        ]);
        assert!(
            !reaches(&res, "pkg/calls.py::use", "pkg/base.py::ZqTable.digest_of"),
            "`-> {annotation}` typed the receiver: {:?}",
            calls_from(&res, "pkg/calls.py::use")
        );
    }
}

/// A parameter named like the factory is the parameter, not the module's
/// function.
#[test]
fn z3_a_factory_name_the_scope_binds_itself_binds_nothing() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef use(make_table):\n    return make_table([1]).digest_of()\n",
        ),
    ]);
    assert!(
        !reaches(&res, "pkg/calls.py::use", "pkg/base.py::ZqTable.digest_of"),
        "{:?}",
        calls_from(&res, "pkg/calls.py::use")
    );
}

/// The annotated class is declared in two files and the caller names neither:
/// there is no telling which one `-> ZqTable` meant.
#[test]
fn z3_a_return_type_the_corpus_declares_twice_binds_nothing() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/other.py",
            "class ZqTable:\n    def digest_of(self):\n        return 'other'\n",
        ),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef use():\n    return make_table([1]).digest_of()\n",
        ),
    ]);
    let got = calls_from(&res, "pkg/calls.py::use");
    assert!(
        !got.iter().any(|t| t == "pkg/other.py::ZqTable.digest_of"),
        "bound the namesake the annotation did not mean: {got:?}"
    );
}

/// The caller's own namesake class is not the class the factory's file meant.
#[test]
fn z3_a_callers_own_namesake_class_is_not_the_return_type() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\nclass ZqTable:\n    def digest_of(self):\n        return 'mine'\n\n\ndef use():\n    return make_table([1]).digest_of()\n",
        ),
    ]);
    assert!(
        !reaches(&res, "pkg/calls.py::use", "pkg/calls.py::ZqTable.digest_of"),
        "{:?}",
        calls_from(&res, "pkg/calls.py::use")
    );
}

/// `module.factory(x).m()`: the inner call carries its own receiver, and the
/// identity of the whole is `module.factory`, which `factory_return_type`
/// already reads through the import.
#[test]
fn z3_a_module_member_factory_binds_through_the_import() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from . import base\n\n\ndef use():\n    return base.make_table([1]).digest_of()\n",
        ),
    ]);
    assert!(
        reaches(&res, "pkg/calls.py::use", "pkg/base.py::ZqTable.digest_of"),
        "{:?}",
        calls_from(&res, "pkg/calls.py::use")
    );
}

/// What the call result's own members hold is not read: `f(x).attr.m()` and a
/// second call in the chain stay unresolved rather than guessed.
#[test]
fn z3_members_and_second_calls_of_the_result_are_not_typed() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/base.py", TABLE),
        (
            "pkg/calls.py",
            "from .base import make_table\n\n\ndef attr():\n    return make_table([1]).inner.digest_of()\n\n\ndef second():\n    return make_table([1]).cache_clear().digest_of()\n",
        ),
    ]);
    assert!(
        !reaches(&res, "pkg/calls.py::attr", "pkg/base.py::ZqTable.digest_of"),
        "{:?}",
        calls_from(&res, "pkg/calls.py::attr")
    );
    assert!(
        !reaches(&res, "pkg/calls.py::second", "pkg/base.py::ZqTable.digest_of"),
        "{:?}",
        calls_from(&res, "pkg/calls.py::second")
    );
}

// --------------------------------------------- class-body field annotations

const TRAINER: &str = "from dataclasses import dataclass\n\n\nclass ZqLog:\n    def losses(self):\n        return []\n\n\nclass ZqLogPlain:\n    def losses(self):\n        return [1]\n\n\nclass ZqLogSlots:\n    def losses(self):\n        return [2]\n\n\nclass ZqHolderPlain:\n    loss_log: ZqLogPlain\n\n\n@dataclass\nclass ZqHolderData:\n    loss_log: ZqLog\n\n\n@dataclass(frozen=True, slots=True)\nclass ZqHolderSlots:\n    loss_log: ZqLogSlots\n\n\ndef train_plain() -> ZqHolderPlain:\n    return ZqHolderPlain()\n\n\ndef train_data() -> ZqHolderData:\n    return ZqHolderData()\n\n\ndef train_slots() -> ZqHolderSlots:\n    return ZqHolderSlots()\n";

const RUN: &str = "from .trainer import ZqHolderPlain, train_data, train_plain, train_slots\n\n\ndef run_plain():\n    result = train_plain()\n    return result.loss_log.losses()\n\n\ndef run_data():\n    result = train_data()\n    return result.loss_log.losses()\n\n\ndef run_slots():\n    result = train_slots()\n    return result.loss_log.losses()\n\n\ndef run_param(result: ZqHolderPlain):\n    return result.loss_log.losses()\n\n\ndef run_param_plainlog(result: ZqHolderPlain):\n    log = result.loss_log\n    return log.losses()\n";

fn field_files<'a>() -> [(&'a str, &'a str); 3] {
    [("pkg/__init__.py", ""), ("pkg/trainer.py", TRAINER), ("pkg/run.py", RUN)]
}

/// py.Z6a: a plain class's `loss_log: ZqLogPlain`, declared in another file than
/// the one reading it. Fails at ec034f60: the field slot is read under the
/// reading file's name, so the owner's own declaration is never found.
#[test]
fn z6a_a_plain_class_field_annotation_types_the_receiver() {
    let res = resolve(&field_files());
    assert!(
        reaches(&res, "pkg/run.py::run_plain", "pkg/trainer.py::ZqLogPlain.losses"),
        "{:?}",
        calls_from(&res, "pkg/run.py::run_plain")
    );
}

/// py.Z6b: the same through `@dataclass`.
#[test]
fn z6b_a_dataclass_field_annotation_types_the_receiver() {
    let res = resolve(&field_files());
    assert!(
        reaches(&res, "pkg/run.py::run_data", "pkg/trainer.py::ZqLog.losses"),
        "{:?}",
        calls_from(&res, "pkg/run.py::run_data")
    );
}

/// py.Z6c: and through `@dataclass(frozen=True, slots=True)`.
#[test]
fn z6c_a_frozen_slots_dataclass_field_annotation_types_the_receiver() {
    let res = resolve(&field_files());
    assert!(
        reaches(&res, "pkg/run.py::run_slots", "pkg/trainer.py::ZqLogSlots.losses"),
        "{:?}",
        calls_from(&res, "pkg/run.py::run_slots")
    );
}

/// py.Z6d: the root is typed by a parameter annotation.
#[test]
fn z6d_a_parameter_typed_root_reads_the_field_annotation() {
    let res = resolve(&field_files());
    assert!(
        reaches(&res, "pkg/run.py::run_param", "pkg/trainer.py::ZqLogPlain.losses"),
        "{:?}",
        calls_from(&res, "pkg/run.py::run_param")
    );
}

/// py.Z6e stays an honest miss: `log = result.loss_log` records no initializer
/// for `log` (the extractor attaches `assigned_to` to calls, not to an
/// attribute read), so the resolver has nothing to read the field from.
/// Needs extractor change (schema frozen at 97).
#[test]
fn z6e_a_field_read_into_a_local_is_an_honest_miss() {
    let res = resolve(&field_files());
    assert!(
        !reaches(&res, "pkg/run.py::run_param_plainlog", "pkg/trainer.py::ZqLogPlain.losses"),
        "the extractor change landed: move py.Z6e out of known_failing.tsv and assert the edge"
    );
    assert!(
        unresolved_call(&res, "pkg/run.py::run_param_plainlog", "losses"),
        "an absent edge must be a recorded unresolved row"
    );
}

/// The field is keyed by its owner: a namesake field on a sibling class of the
/// same file types nothing here.
#[test]
fn z6_a_namesake_field_on_another_class_does_not_answer() {
    let trainer = format!(
        "{TRAINER}\n\nclass ZqHolderOther:\n    loss_log: ZqLogSlots\n"
    );
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/trainer.py", &trainer),
        ("pkg/run.py", RUN),
    ]);
    let got = calls_from(&res, "pkg/run.py::run_plain");
    assert!(got.iter().any(|t| t == "pkg/trainer.py::ZqLogPlain.losses"), "{got:?}");
    assert!(!got.iter().any(|t| t == "pkg/trainer.py::ZqLogSlots.losses"), "{got:?}");
}

/// The annotation's class is declared twice in the corpus and the reader names
/// neither: the field's type is not one class.
#[test]
fn z6_a_field_type_the_corpus_declares_twice_binds_nothing() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/trainer.py", TRAINER),
        (
            "pkg/other.py",
            "class ZqLogPlain:\n    def losses(self):\n        return ['other']\n",
        ),
        ("pkg/run.py", RUN),
    ]);
    let got = calls_from(&res, "pkg/run.py::run_plain");
    assert!(
        !got.iter().any(|t| t == "pkg/other.py::ZqLogPlain.losses"),
        "bound the namesake the annotation did not mean: {got:?}"
    );
}

/// The reader's own `ZqLogPlain` is not the class the owner's file annotated.
#[test]
fn z6_a_readers_own_namesake_is_not_the_field_type() {
    let run = format!(
        "{RUN}\n\nclass ZqLogPlain:\n    def losses(self):\n        return ['mine']\n"
    );
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/trainer.py", TRAINER),
        ("pkg/run.py", &run),
    ]);
    assert!(
        !reaches(&res, "pkg/run.py::run_plain", "pkg/run.py::ZqLogPlain.losses"),
        "{:?}",
        calls_from(&res, "pkg/run.py::run_plain")
    );
}

/// A container or optional annotation names no class a method can be looked up
/// in.
#[test]
fn z6_a_generic_field_annotation_binds_nothing() {
    for annotation in ["list[ZqLog]", "Optional[ZqLog]", "ZqLog | None", "dict[str, ZqLog]"] {
        let trainer = format!(
            "from typing import Optional\n\n\nclass ZqLog:\n    def losses(self):\n        return []\n\n\nclass ZqHolder:\n    loss_log: {annotation}\n\n\ndef train() -> ZqHolder:\n    return ZqHolder()\n"
        );
        let res = resolve(&[
            ("pkg/__init__.py", ""),
            ("pkg/trainer.py", &trainer),
            (
                "pkg/run.py",
                "from .trainer import train\n\n\ndef run():\n    result = train()\n    return result.loss_log.losses()\n",
            ),
        ]);
        assert!(
            !reaches(&res, "pkg/run.py::run", "pkg/trainer.py::ZqLog.losses"),
            "`loss_log: {annotation}` typed the receiver: {:?}",
            calls_from(&res, "pkg/run.py::run")
        );
    }
}

/// A subclass reads no field its base annotated: the slot is keyed by the class
/// that wrote it, and a subclass may redeclare it.
#[test]
fn z6_an_inherited_field_annotation_is_not_read_through_the_subclass() {
    let trainer = format!("{TRAINER}\n\nclass ZqHolderChild(ZqHolderPlain):\n    pass\n\n\ndef train_child() -> ZqHolderChild:\n    return ZqHolderChild()\n");
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/trainer.py", &trainer),
        (
            "pkg/run.py",
            "from .trainer import train_child\n\n\ndef run():\n    result = train_child()\n    return result.loss_log.losses()\n",
        ),
    ]);
    assert!(
        !reaches(&res, "pkg/run.py::run", "pkg/trainer.py::ZqLogPlain.losses"),
        "an inherited annotation is a separate rule: {:?}",
        calls_from(&res, "pkg/run.py::run")
    );
}

// --------------------------------------------------- tuple unpack and pytest

/// py.Z4 stays an honest miss: the helper has no annotation and its `return
/// table, other` is not a fact the extractor records (the extraction has no
/// return-statement or tuple-element facts), so the unpacked name has no type.
/// Needs extractor change (schema frozen at 97).
#[test]
fn z4_tuple_unpack_from_an_unannotated_helper_is_an_honest_miss() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/base.py",
            "class ZqTable:\n    def digest_of(self):\n        return 'd'\n\n\nclass ZqBase:\n    def accumulate(self, n):\n        return n\n\n\ndef unannotated_pair():\n    table = ZqTable()\n    other = ZqBase()\n    return table, other\n",
        ),
        (
            "pkg/calls.py",
            "from .base import unannotated_pair\n\n\ndef use_tuple_unpack():\n    table, other = unannotated_pair()\n    return table.digest_of()\n",
        ),
    ]);
    assert!(
        !reaches(&res, "pkg/calls.py::use_tuple_unpack", "pkg/base.py::ZqTable.digest_of"),
        "the extractor change landed: move py.Z4 out of known_failing.tsv and assert the edge"
    );
    assert!(unresolved_call(&res, "pkg/calls.py::use_tuple_unpack", "digest_of"));
}

/// py.Z5 / py.C23b stay honest misses: the extraction records `@pytest.fixture`
/// as two Name references with no link to the function it decorates, so the
/// resolver cannot tell a fixture from a function that merely sits below some
/// other reference, nor see a `@pytest.mark.parametrize("zq_env", ..)` that
/// replaces it. Needs extractor change (schema frozen at 97).
#[test]
fn z5_a_pytest_fixture_parameter_is_an_honest_miss() {
    let res = resolve(&[
        (
            "tests/test_zq.py",
            "import pytest\n\n\nclass ZqEnv:\n    def launch(self):\n        return 1\n\n\n@pytest.fixture\ndef zq_env() -> ZqEnv:\n    return ZqEnv()\n\n\ndef test_zq_launches(zq_env):\n    assert zq_env.launch() == 1\n",
        ),
    ]);
    assert!(
        !reaches(&res, "tests/test_zq.py::test_zq_launches", "tests/test_zq.py::ZqEnv.launch"),
        "the extractor change landed: move py.Z5 out of known_failing.tsv and assert the edge"
    );
    assert!(unresolved_call(&res, "tests/test_zq.py::test_zq_launches", "launch"));
}

// ------------------------------------------------------ 3-segment module path

fn deep_files<'a>(main: &'a str) -> [(&'a str, &'a str); 5] {
    [
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("a/b/other.py", "def f():\n    return 2\n"),
        ("main.py", main),
    ]
}

/// `import a.b.c` then `a.b.c.f()`: the dotted module path goes through import
/// resolution and binds the one `f` that file declares.
#[test]
fn a_three_segment_import_binds_the_module_member() {
    let res = resolve(&deep_files("import a.b.c\n\n\ndef g():\n    return a.b.c.f()\n"));
    assert!(
        reaches(&res, "main.py::g", "a/b/c.py::f"),
        "{:?}",
        calls_from(&res, "main.py::g")
    );
    assert!(!reaches(&res, "main.py::g", "a/b/other.py::f"));
}

/// `import a.b.c as m` binds the leaf, `from a.b import c` binds the leaf too.
#[test]
fn a_three_segment_alias_and_from_import_bind_the_module_member() {
    let aliased = resolve(&deep_files("import a.b.c as m\n\n\ndef g():\n    return m.f()\n"));
    assert!(reaches(&aliased, "main.py::g", "a/b/c.py::f"), "{:?}", calls_from(&aliased, "main.py::g"));
    let from = resolve(&deep_files("from a.b import c\n\n\ndef g():\n    return c.f()\n"));
    assert!(reaches(&from, "main.py::g", "a/b/c.py::f"), "{:?}", calls_from(&from, "main.py::g"));
}

/// A path prefix is not the leaf: `a.b.f()` after `import a.b.c` names `f` of
/// package `a.b` (its `__init__`), which declares none.
#[test]
fn a_three_segment_import_does_not_bind_the_prefix_module() {
    let res = resolve(&deep_files("import a.b.c\n\n\ndef g():\n    return a.b.f()\n"));
    assert!(!reaches(&res, "main.py::g", "a/b/c.py::f"), "{:?}", calls_from(&res, "main.py::g"));
    assert!(!reaches(&res, "main.py::g", "a/b/other.py::f"), "{:?}", calls_from(&res, "main.py::g"));
}
