//! Closing adversarial probes for lane/py-receiver-residue (ft-c0afdfef) at
//! fc69f090. Each probe asserts the correct Python behaviour; a failing probe
//! is a finding. `edge` probes are recall (LOW unless the lost edge is a
//! common shape); `no_edge` probes are precision.

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

const AC: &str = "class A:\n    def run(self):\n        return 1\n    def append(self, x):\n        return 2\n";
const USE: &str = "from f import make\n\ndef use():\n    make().run()\n";
const USE_APPEND: &str = "from f import make\n\ndef use():\n    make().append(1)\n";

fn factory(src_between: &str) -> ResolutionResult {
    // `f.py` = class A + whatever sits before `make`; `app.py` calls make().run()
    let f = format!("import functools\nfrom functools import cache, lru_cache, cached_property\n\n{AC}\n{src_between}def make() -> A:\n    return A()\n");
    resolve(&[("f.py", &f), ("app.py", USE)])
}

// =================== decorator heuristic ==================================

#[test]
fn d01_lru_cache_with_args_is_transparent() {
    let res = factory("@functools.lru_cache(maxsize=None)\n");
    edge(&res, "app.py::use", "f.py::A.run", "d01 @functools.lru_cache(maxsize=None) recall");
}

#[test]
fn d01b_functools_dot_cache_bare() {
    let res = factory("@functools.cache\n");
    edge(&res, "app.py::use", "f.py::A.run", "d01b @functools.cache recall");
}

#[test]
fn d01c_lru_cache_maxsize_128() {
    let res = factory("@lru_cache(maxsize=128)\n");
    edge(&res, "app.py::use", "f.py::A.run", "d01c @lru_cache(maxsize=128) recall");
}

#[test]
fn d01d_bare_forms() {
    for dec in ["@lru_cache\n", "@lru_cache()\n", "@cache\n"] {
        let res = factory(dec);
        edge(&res, "app.py::use", "f.py::A.run", &format!("d01d {dec:?} recall"));
    }
}

#[test]
fn d02_aliased_cache_recall() {
    let f = format!("from functools import cache as memo\n\n{AC}\n@memo\ndef make() -> A:\n    return A()\n");
    let res = resolve(&[("f.py", &f), ("app.py", USE)]);
    edge(&res, "app.py::use", "f.py::A.run", "d02 @memo (functools.cache alias) recall");
}

#[test]
fn d03_user_defined_cache_that_wraps_into_list() {
    let f = format!(
        "from typing import List\n\n{AC}\ndef cache(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n\n@cache\ndef make() -> A:\n    return A()\n"
    );
    let res = resolve(&[("f.py", &f), ("app.py", USE_APPEND)]);
    no_edge(&res, "app.py::use", "f.py::A.append", "d03 user-defined @cache wraps to List[A]");
}

#[test]
fn d03b_user_defined_lru_cache_that_wraps_into_list() {
    let f = format!(
        "from typing import List\n\n{AC}\ndef lru_cache(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n\n@lru_cache\ndef make() -> A:\n    return A()\n"
    );
    let res = resolve(&[("f.py", &f), ("app.py", USE_APPEND)]);
    no_edge(&res, "app.py::use", "f.py::A.append", "d03b user-defined @lru_cache wraps to List[A]");
}

#[test]
fn d03c_imported_user_cache_that_wraps_into_list() {
    let res = resolve(&[
        (
            "deco.py",
            "from typing import List\n\nclass A:\n    def append(self, x):\n        return 2\n\ndef cache(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n",
        ),
        (
            "f.py",
            "from deco import cache, A\n\n@cache\ndef make() -> A:\n    return A()\n",
        ),
        ("app.py", USE_APPEND),
    ]);
    no_edge(&res, "app.py::use", "deco.py::A.append", "d03c imported user @cache");
}

#[test]
fn d03d_attribute_decorator_named_cache_from_user_module() {
    let res = resolve(&[
        (
            "deco.py",
            "from typing import List\n\nclass A:\n    def append(self, x):\n        return 2\n\ndef cache(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n",
        ),
        (
            "f.py",
            "import deco\nfrom deco import A\n\n@deco.cache\ndef make() -> A:\n    return A()\n",
        ),
        ("app.py", USE_APPEND),
    ]);
    no_edge(&res, "app.py::use", "deco.py::A.append", "d03d @deco.cache (user module attribute)");
}

#[test]
fn d04_property_on_factory_method_is_not_a_call() {
    // `h.make()` on a property returns the instance's .__call__, not A.
    let res = resolve(&[(
        "m.py",
        "class A:\n    def run(self):\n        return 1\n\nclass H:\n    @property\n    def make(self) -> A:\n        return A()\n\ndef use(h: H):\n    h.make().run()\n",
    )]);
    no_edge(&res, "m.py::use", "m.py::A.run", "d04 @property make() call");
}

#[test]
fn d04b_cached_property_factory_method_not_a_call() {
    let res = resolve(&[(
        "m.py",
        "from functools import cached_property\n\nclass A:\n    def run(self):\n        return 1\n\nclass H:\n    @cached_property\n    def make(self) -> A:\n        return A()\n\ndef use(h: H):\n    h.make().run()\n",
    )]);
    no_edge(&res, "m.py::use", "m.py::A.run", "d04b @cached_property make() call");
}

#[test]
fn d05_stacked_transparent_then_opaque() {
    let f = format!(
        "import functools\nfrom typing import List\n\n{AC}\ndef many(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n\n@functools.lru_cache(maxsize=None)\n@many\ndef make() -> A:\n    return A()\n"
    );
    let res = resolve(&[("f.py", &f), ("app.py", USE_APPEND)]);
    no_edge(&res, "app.py::use", "f.py::A.append", "d05 lru_cache stacked on @many");
}

#[test]
fn d05b_opaque_then_transparent_stacked() {
    let f = format!(
        "import functools\nfrom typing import List\n\n{AC}\ndef many(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n\n@many\n@functools.lru_cache(maxsize=None)\ndef make() -> A:\n    return A()\n"
    );
    let res = resolve(&[("f.py", &f), ("app.py", USE_APPEND)]);
    no_edge(&res, "app.py::use", "f.py::A.append", "d05b @many stacked on lru_cache");
}

#[test]
fn d05c_staticmethod_on_a_class_factory_recall() {
    let res = resolve(&[
        (
            "f.py",
            &format!("{AC}\nclass Fx:\n    @staticmethod\n    def make() -> A:\n        return A()\n"),
        ),
        ("app.py", "from f import Fx\n\ndef use():\n    Fx.make().run()\n"),
    ]);
    edge(&res, "app.py::use", "f.py::A.run", "d05c @staticmethod factory recall");
}

#[test]
fn d05d_staticmethod_stacked_on_opaque() {
    let res = resolve(&[
        (
            "f.py",
            &format!("from typing import List\n\n{AC}\ndef many(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n\nclass Fx:\n    @staticmethod\n    @many\n    def make() -> A:\n        return A()\n"),
        ),
        ("app.py", "from f import Fx\n\ndef use():\n    Fx.make().append(1)\n"),
    ]);
    no_edge(&res, "app.py::use", "f.py::A.append", "d05d @staticmethod @many");
}

#[test]
fn d06_class_decorator_does_not_block_its_factory() {
    let f = "from dataclasses import dataclass\n\n@dataclass\nclass A:\n    x: int = 0\n    def run(self):\n        return 1\n\ndef make() -> A:\n    return A()\n";
    let res = resolve(&[("f.py", f), ("app.py", USE)]);
    edge(&res, "app.py::use", "f.py::A.run", "d06 class decorator recall");
}

#[test]
fn d06b_class_decorator_directly_before_a_method_factory_class() {
    // The class decorator sits between the previous symbol and the class; the
    // first method must not be read as decorated by it.
    let f = "class A:\n    def run(self):\n        return 1\n\ndef register(c):\n    return c\n\n@register\nclass Fx:\n    @staticmethod\n    def make() -> A:\n        return A()\n";
    let res = resolve(&[("f.py", f), ("app.py", "from f import Fx\n\ndef use():\n    Fx.make().run()\n")]);
    edge(&res, "app.py::use", "f.py::A.run", "d06b class decorator then staticmethod recall");
}

#[test]
fn d07_multiline_decorator_call_spanning_previous_symbol() {
    let f = format!(
        "from typing import List\n\n{AC}\ndef prev():\n    return 1\n\ndef many(*xs):\n    def deco(fn):\n        def inner(*a) -> List[A]:\n            return [fn(*a)]\n        return inner\n    return deco\n\n@many(\n    prev,\n    1,\n)\ndef make() -> A:\n    return A()\n"
    );
    let res = resolve(&[("f.py", &f), ("app.py", USE_APPEND)]);
    no_edge(&res, "app.py::use", "f.py::A.append", "d07 multi-line decorator call");
}

#[test]
fn d07b_decorator_directly_after_previous_symbol_no_blank_line() {
    let f = format!(
        "from typing import List\n\n{AC}\ndef many(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\ndef prev():\n    return 1\n@many\ndef make() -> A:\n    return A()\n"
    );
    let res = resolve(&[("f.py", &f), ("app.py", USE_APPEND)]);
    no_edge(&res, "app.py::use", "f.py::A.append", "d07b decorator with no blank line");
}

#[test]
fn d07c_decorator_is_the_first_statement_after_imports_only() {
    // No previous symbol at all: the decorator is the first thing in the file.
    let g = format!("import deco\nfrom deco import A\n\n@deco.many\ndef make() -> A:\n    return A()\n");
    let res = resolve(&[
        ("deco.py", "from typing import List\n\nclass A:\n    def append(self, x):\n        return 1\n\ndef many(fn):\n    def inner(*a) -> List[A]:\n        return [fn(*a)]\n    return inner\n"),
        ("f.py", &g),
        ("app.py", USE_APPEND),
    ]);
    no_edge(&res, "app.py::use", "deco.py::A.append", "d07c first-in-file decorator");
}

// ---- cost side: recall lost, to be recorded as LOW ---------------------

#[test]
fn d08_comment_between_functions_is_not_a_decorator() {
    let res = factory("# a comment between\n");
    edge(&res, "app.py::use", "f.py::A.run", "d08 comment recall");
}

#[test]
fn d08b_docstring_between_functions_is_not_a_decorator() {
    let res = factory("\"\"\"module level string between\"\"\"\n\n");
    edge(&res, "app.py::use", "f.py::A.run", "d08b string recall");
}

#[test]
fn d08c_bare_annotation_between_functions_is_not_a_decorator() {
    let res = factory("LIMIT: int\n\n");
    edge(&res, "app.py::use", "f.py::A.run", "d08c annotation recall");
}

#[test]
fn d08d_constant_assignment_between_functions() {
    let res = factory("LIMIT = 3\nNAMES = ['a', 'b']\n\n");
    edge(&res, "app.py::use", "f.py::A.run", "d08d constants recall");
}

#[test]
fn d08e_assigned_call_between_functions() {
    let res = factory("LOG = dict()\nREG = list(range(3))\n\n");
    edge(&res, "app.py::use", "f.py::A.run", "d08e module-level assigned calls recall");
}

#[test]
fn d08f_method_factory_in_class_with_base_class() {
    let f = format!("class Base:\n    pass\n\n{AC}\nclass Fx(Base):\n    def make(self) -> A:\n        return A()\n");
    let res = resolve(&[
        ("f.py", &f),
        ("app.py", "from f import Fx\n\ndef use():\n    Fx().make().run()\n"),
    ]);
    edge(&res, "app.py::use", "f.py::A.run", "d08f method in class with a base (class header Name is not a decorator)");
}

#[test]
#[ignore = "ft-c0afdfef follow-up: a nested def now shadows the factory name (a33), so the local def is not yet resolved as the callee"]
fn d08g_nested_factory_after_outer_annotation() {
    let f = format!("{AC}\ndef outer(x: int = 3) -> A:\n    def make() -> A:\n        return A()\n    return make().run() and A()\n");
    let res = resolve(&[("f.py", &f)]);
    edge(&res, "f.py::outer", "f.py::A.run", "d08g nested factory after outer signature");
}

#[test]
fn d08h_class_attribute_between_methods() {
    let f = format!("{AC}\nclass Fx:\n    NAME = 'x'\n    count: int = 0\n\n    def make(self) -> A:\n        return A()\n");
    let res = resolve(&[
        ("f.py", &f),
        ("app.py", "from f import Fx\n\ndef use():\n    Fx().make().run()\n"),
    ]);
    edge(&res, "app.py::use", "f.py::A.run", "d08h class attrs before method recall");
}

#[test]
fn d08i_method_after_method_with_default_ref() {
    let f = format!("{AC}\nclass Fx:\n    def first(self, x=A):\n        return x\n\n    def make(self) -> A:\n        return A()\n");
    let res = resolve(&[
        ("f.py", &f),
        ("app.py", "from f import Fx\n\ndef use():\n    Fx().make().run()\n"),
    ]);
    edge(&res, "app.py::use", "f.py::A.run", "d08i sibling method default arg recall");
}

// =================== annotation through imports ===========================

const LOGS: &str = "class Log:\n    def write(self):\n        return 1\n    def append(self, x):\n        return 2\n";
const APP_LOG: &str = "from pkg.fac import make\n\ndef use():\n    make().write()\n";

#[test]
fn n01_from_dot_import_module_attribute_annotation() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/models.py", LOGS),
        ("pkg/fac.py", "from . import models\n\ndef make() -> models.Log:\n    return models.Log()\n"),
        ("app.py", APP_LOG),
    ]);
    edge(&res, "app.py::use", "pkg/models.py::Log.write", "n01 recall via module attribute");
}

#[test]
fn n01b_module_attribute_annotation_does_not_pick_namesake() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/models.py", LOGS),
        ("other.py", "class Log:\n    def write(self):\n        return 9\n"),
        ("pkg/fac.py", "from . import models\n\ndef make() -> models.Log:\n    return models.Log()\n"),
        ("app.py", "from pkg.fac import make\nfrom other import Log\n\ndef use():\n    make().write()\n"),
    ]);
    no_edge(&res, "app.py::use", "other.py::Log.write", "n01b reader's Log is the other class");
}

#[test]
fn n02_import_as_module_attribute_annotation() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/mod.py", LOGS),
        ("pkg/fac.py", "import pkg.mod as m\n\ndef make() -> m.Log:\n    return m.Log()\n"),
        ("app.py", APP_LOG),
    ]);
    edge(&res, "app.py::use", "pkg/mod.py::Log.write", "n02 recall via import-as");
}

#[test]
fn n03_string_annotation_defined_later_in_owner() {
    let fac_src = format!("def make() -> \"Log\":\n    return Log()\n\n{LOGS}");
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/fac.py", &fac_src),
        ("app.py", APP_LOG),
    ]);
    edge(&res, "app.py::use", "pkg/fac.py::Log.write", "n03 recall for forward-ref string");
}

#[test]
fn n03b_string_annotation_defined_later_not_a_namesake_elsewhere() {
    let fac_src = format!("def make() -> \"Log\":\n    return Log()\n\n{LOGS}");
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("other.py", "class Log:\n    def write(self):\n        return 9\n"),
        ("pkg/fac.py", &fac_src),
        ("app.py", "from pkg.fac import make\nfrom other import Log\n\ndef use():\n    make().write()\n"),
    ]);
    no_edge(&res, "app.py::use", "other.py::Log.write", "n03b forward ref resolved in owner file");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: the owner-file annotation reader abstains on this spelling (TYPE_CHECKING import, future annotations, Optional string, module-attribute field)"]
fn n04_type_checking_only_import() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "fac.py",
            "from typing import TYPE_CHECKING\n\nif TYPE_CHECKING:\n    from logs import Log\n\ndef make() -> \"Log\":\n    raise NotImplementedError\n",
        ),
        ("app.py", "from fac import make\n\ndef use():\n    make().write()\n"),
    ]);
    edge(&res, "app.py::use", "logs.py::Log.write", "n04 TYPE_CHECKING import recall");
}

#[test]
fn n04b_type_checking_import_does_not_pick_namesake() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("other.py", "class Log:\n    def write(self):\n        return 9\n"),
        (
            "fac.py",
            "from typing import TYPE_CHECKING\n\nif TYPE_CHECKING:\n    from logs import Log\n\ndef make() -> \"Log\":\n    raise NotImplementedError\n",
        ),
        ("app.py", "from fac import make\nfrom other import Log\n\ndef use():\n    make().write()\n"),
    ]);
    no_edge(&res, "app.py::use", "other.py::Log.write", "n04b");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: the owner-file annotation reader abstains on this spelling (TYPE_CHECKING import, future annotations, Optional string, module-attribute field)"]
fn n05_future_annotations() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "fac.py",
            "from __future__ import annotations\nfrom logs import Log\n\ndef make() -> Log:\n    return Log()\n",
        ),
        ("app.py", "from fac import make\n\ndef use():\n    make().write()\n"),
    ]);
    edge(&res, "app.py::use", "logs.py::Log.write", "n05 __future__ annotations recall");
}

#[test]
fn n06_owner_local_class_shadows_imported_name() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "fac.py",
            "from logs import Log\n\nclass Log:\n    def write(self):\n        return 5\n\ndef make() -> Log:\n    return Log()\n",
        ),
        ("app.py", "from fac import make\n\ndef use():\n    make().write()\n"),
    ]);
    no_edge(&res, "app.py::use", "logs.py::Log.write", "n06 local class Log shadows import");
}

#[test]
fn n06b_import_after_local_class_wins() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "fac.py",
            "class Log:\n    def write(self):\n        return 5\n\nfrom logs import Log\n\ndef make() -> Log:\n    return Log()\n",
        ),
        ("app.py", "from fac import make\n\ndef use():\n    make().write()\n"),
    ]);
    let local = reaches(&res, "app.py::use", "fac.py::Log.write");
    let imported = reaches(&res, "app.py::use", "logs.py::Log.write");
    assert_eq!(local, imported, "n06b: rebound class + import picks one: {:?}", calls_from(&res, "app.py::use"));
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: the owner-file annotation reader abstains on this spelling (TYPE_CHECKING import, future annotations, Optional string, module-attribute field)"]
fn n07_optional_string_annotation() {
    let res = resolve(&[
        ("logs.py", LOGS),
        (
            "fac.py",
            "from typing import Optional\nfrom logs import Log\n\ndef make() -> Optional[\"Log\"]:\n    return None\n",
        ),
        ("app.py", "from fac import make\n\ndef use():\n    make().write()\n"),
    ]);
    edge(&res, "app.py::use", "logs.py::Log.write", "n07 Optional[\"Log\"] recall");
}

#[test]
fn n08_unfollowable_import_does_not_fall_to_unique_corpus_name() {
    let res = resolve(&[
        ("other.py", "class Log:\n    def write(self):\n        return 9\n"),
        (
            "fac.py",
            "from thirdparty import Log\n\ndef make() -> Log:\n    return Log()\n",
        ),
        ("app.py", "from fac import make\n\ndef use():\n    make().write()\n"),
    ]);
    no_edge(&res, "app.py::use", "other.py::Log.write", "n08 external Log vs corpus Log");
}

#[test]
fn n09_star_import_in_owner_abstains_not_unique_name() {
    let res = resolve(&[
        ("other.py", "class Log:\n    def write(self):\n        return 9\n"),
        (
            "fac.py",
            "from thirdparty import *\n\ndef make() -> Log:\n    return Log()\n",
        ),
        ("app.py", "from fac import make\n\ndef use():\n    make().write()\n"),
    ]);
    no_edge(&res, "app.py::use", "other.py::Log.write", "n09 star import in owner");
}

#[test]
fn n10_field_annotation_through_owner_imports() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("other.py", "class Log:\n    def write(self):\n        return 9\n"),
        (
            "holder.py",
            "from thirdparty import Log\n\nclass Holder:\n    log: Log\n",
        ),
        ("app.py", "from holder import Holder\n\ndef use(h: Holder):\n    h.log.write()\n"),
    ]);
    no_edge(&res, "app.py::use", "logs.py::Log.write", "n10 external field type, logs.py");
    no_edge(&res, "app.py::use", "other.py::Log.write", "n10 external field type, other.py");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: the owner-file annotation reader abstains on this spelling (TYPE_CHECKING import, future annotations, Optional string, module-attribute field)"]
fn n10b_field_annotation_module_attribute() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/models.py", LOGS),
        ("pkg/holder.py", "from . import models\n\nclass Holder:\n    log: models.Log\n"),
        ("app.py", "from pkg.holder import Holder\n\ndef use(h: Holder):\n    h.log.write()\n"),
    ]);
    edge(&res, "app.py::use", "pkg/models.py::Log.write", "n10b field module attribute recall");
}

// =================== scope shadow =========================================

fn shadow_files(app_tail: &str) -> ResolutionResult {
    let app = format!(
        "from holder import Holder\n\ndef typed(h: Holder):\n    return h.log.write()\n\n{app_tail}"
    );
    resolve(&[
        ("logs.py", LOGS),
        ("holder.py", "from logs import Log\n\nclass Holder:\n    log: Log\n"),
        ("app.py", &app),
    ])
}

fn shadow(label: &str, caller: &str, tail: &str) {
    let res = shadow_files(tail);
    // sanity: the typed function itself still resolves (the probe is live)
    edge(&res, "app.py::typed", "logs.py::Log.write", &format!("{label} sanity"));
    no_edge(&res, caller, "logs.py::Log.write", label);
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: a global declaration or class-body binding is not a scope fact the scope lookup holds"]
fn s01_global_declaration() {
    shadow("s01 global h", "app.py::other", "def other():\n    global h\n    h.log.write()\n");
}

#[test]
fn s02_nonlocal_declaration() {
    shadow(
        "s02 nonlocal h",
        "app.py::outer.inner",
        "def outer():\n    h = make()\n    def inner():\n        nonlocal h\n        h.log.write()\n    return inner\n",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: match/case captures need extractor binding facts for match patterns (schema frozen at 97)"]
fn s03_match_capture_pattern() {
    shadow("s03 match capture", "app.py::other", "def other(x):\n    match x:\n        case [h, *_]:\n            h.log.write()\n");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: match/case captures need extractor binding facts for match patterns (schema frozen at 97)"]
fn s03b_match_class_pattern() {
    shadow("s03b Foo(h)", "app.py::other", "def other(x):\n    match x:\n        case Foo(h):\n            h.log.write()\n");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: match/case captures need extractor binding facts for match patterns (schema frozen at 97)"]
fn s03c_match_mapping_pattern() {
    shadow("s03c {k: h}", "app.py::other", "def other(x):\n    match x:\n        case {\"k\": h}:\n            h.log.write()\n");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: match/case captures need extractor binding facts for match patterns (schema frozen at 97)"]
fn s03d_match_as_pattern() {
    shadow("s03d as h", "app.py::other", "def other(x):\n    match x:\n        case (1 | 2) as h:\n            h.log.write()\n");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: match/case captures need extractor binding facts for match patterns (schema frozen at 97)"]
fn s03e_match_star_pattern() {
    shadow("s03e [*h]", "app.py::other", "def other(x):\n    match x:\n        case [*h]:\n            h.log.write()\n");
}

#[test]
fn s04_del_then_reuse() {
    shadow("s04 del h", "app.py::other", "def other(h):\n    del h\n    h = make()\n    h.log.write()\n");
}

#[test]
fn s05_lambda_parameter() {
    shadow("s05 lambda param", "app.py::other", "def other():\n    return (lambda h: h.log.write())\n");
}

#[test]
fn s06_class_body_variable_vs_method_param() {
    shadow(
        "s06 method param h",
        "app.py::K.m",
        "class K:\n    h = None\n    def m(self, h):\n        return h.log.write()\n",
    );
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: a global declaration or class-body binding is not a scope fact the scope lookup holds"]
fn s06b_class_body_variable_is_not_visible_in_method() {
    // `h` in a method body is the module's, never the class body's.
    shadow(
        "s06b class var h not in method scope",
        "app.py::K.m",
        "class K:\n    h = make()\n    def m(self):\n        return h.log.write()\n",
    );
}

#[test]
fn s07_star_args_and_kwargs_named_h() {
    shadow("s07 *h", "app.py::other", "def other(*h):\n    return h.log.write()\n");
    shadow("s07 **h", "app.py::other", "def other(**h):\n    return h.log.write()\n");
}

#[test]
fn s08_positional_only_and_keyword_only() {
    shadow("s08 posonly", "app.py::other", "def other(h, /):\n    return h.log.write()\n");
    shadow("s08 kwonly", "app.py::other", "def other(*, h):\n    return h.log.write()\n");
    shadow("s08 kwonly default", "app.py::other", "def other(*, h=None):\n    return h.log.write()\n");
}

#[test]
fn s09_same_function_name_in_two_classes() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", "from logs import Log\n\nclass Holder:\n    log: Log\n"),
        (
            "app.py",
            "from holder import Holder\n\nclass P:\n    def go(self, h: Holder):\n        return h.log.write()\n\nclass Q:\n    def go(self, h):\n        return h.log.write()\n",
        ),
    ]);
    edge(&res, "app.py::P.go", "logs.py::Log.write", "s09 sanity");
    no_edge(&res, "app.py::Q.go", "logs.py::Log.write", "s09 Q.go untyped h");
}

#[test]
fn s10_closure_over_typed_param_recall() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", "from logs import Log\n\nclass Holder:\n    log: Log\n"),
        (
            "app.py",
            "from holder import Holder\n\ndef typed(h: Holder):\n    def inner():\n        return h.log.write()\n    return inner\n",
        ),
    ]);
    edge(&res, "app.py::typed.inner", "logs.py::Log.write", "s10 closure over typed param (recall)");
}

#[test]
fn s10b_lambda_closure_over_typed_param_recall() {
    let res = resolve(&[
        ("logs.py", LOGS),
        ("holder.py", "from logs import Log\n\nclass Holder:\n    log: Log\n"),
        (
            "app.py",
            "from holder import Holder\n\ndef typed(h: Holder):\n    return (lambda: h.log.write())\n",
        ),
    ]);
    edge(&res, "app.py::typed", "logs.py::Log.write", "s10b lambda closure over typed param (recall)");
}

#[test]
fn s11_typed_keyword_only_and_posonly_recall() {
    for sig in ["h: Holder, /", "*, h: Holder", "x=1, *, h: Holder = None"] {
        let app = format!("from holder import Holder\n\ndef typed({sig}):\n    return h.log.write()\n");
        let res = resolve(&[
            ("logs.py", LOGS),
            ("holder.py", "from logs import Log\n\nclass Holder:\n    log: Log\n"),
            ("app.py", &app),
        ]);
        edge(&res, "app.py::typed", "logs.py::Log.write", &format!("s11 typed sig `{sig}` recall"));
    }
}

#[test]
fn s12_factory_name_shadowed_by_each_binding_kind() {
    // the factory-call path: `make` re-bound in the calling function
    for (label, body) in [
        ("global", "    global make\n    return make().append(1)\n"),
        ("param-star", ""),
        ("except-as", "    try:\n        pass\n    except Exception as make:\n        return make().append(1)\n"),
        ("walrus", "    (make := list)\n    return make().append(1)\n"),
        ("for", "    for make in []:\n        return make().append(1)\n"),
        ("import-local", "    from other import make\n    return make().append(1)\n"),
    ] {
        if body.is_empty() {
            continue;
        }
        let app = format!("from f import make\n\ndef use():\n{body}");
        let f = format!("{AC}\ndef make() -> A:\n    return A()\n");
        let res = resolve(&[
            ("f.py", &f),
            ("other.py", "def make():\n    return []\n"),
            ("app.py", &app),
        ]);
        if label == "global" {
            continue; // `global make` refers to the import: A is the right answer
        }
        no_edge(&res, "app.py::use", "f.py::A.append", &format!("s12 {label}"));
    }
}

// =================== two-import abstention ================================

#[test]
fn i01_same_module_imported_twice_does_not_abstain() {
    let res = resolve(&[
        ("a.py", "def f():\n    return 1\n"),
        ("app.py", "import a\nimport a\n\ndef use():\n    a.f()\n"),
    ]);
    edge(&res, "app.py::use", "a.py::f", "i01 import a; import a (recall)");
}

#[test]
fn i01b_same_from_import_twice_does_not_abstain() {
    let res = resolve(&[
        ("f.py", &format!("{AC}\ndef make() -> A:\n    return A()\n")),
        ("app.py", "from f import make\nfrom f import make\n\ndef use():\n    make().run()\n"),
    ]);
    edge(&res, "app.py::use", "f.py::A.run", "i01b same from-import twice (recall)");
}

#[test]
fn i02_same_from_import_in_both_branches() {
    let res = resolve(&[
        ("f.py", &format!("{AC}\ndef make() -> A:\n    return A()\n")),
        (
            "app.py",
            "import sys\nif sys.platform == 'win32':\n    from f import make\nelse:\n    from f import make\n\ndef use():\n    make().run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "f.py::A.run", "i02 same target in both branches (recall)");
}

#[test]
fn i02b_try_except_same_target() {
    let res = resolve(&[
        ("f.py", &format!("{AC}\ndef make() -> A:\n    return A()\n")),
        (
            "app.py",
            "try:\n    from f import make\nexcept ImportError:\n    from f import make\n\ndef use():\n    make().run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "f.py::A.run", "i02b try/except same target (recall)");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: two paths to the same declaration are read as two providers and abstain"]
fn i03_reexport_chain_to_the_same_declaration() {
    let res = resolve(&[
        ("pkg/__init__.py", "from .impl import make\n"),
        ("pkg/impl.py", &format!("{AC}\ndef make() -> A:\n    return A()\n")),
        (
            "app.py",
            "from pkg import make\nfrom pkg.impl import make\n\ndef use():\n    make().run()\n",
        ),
    ]);
    edge(&res, "app.py::use", "pkg/impl.py::A.run", "i03 two imports to the same declaration (recall)");
}

#[test]
fn i04_two_stars_second_defines_nothing_of_the_name() {
    let res = resolve(&[
        ("a.py", &format!("{AC}\ndef make() -> A:\n    return A()\n")),
        ("b.py", "def other():\n    return 1\n"),
        ("app.py", "from a import *\nfrom b import *\n\ndef use():\n    make().run()\n"),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "i04 only one star defines make (recall)");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: two paths to the same declaration are read as two providers and abstain"]
fn i04b_two_stars_reexporting_the_same_declaration() {
    let res = resolve(&[
        ("a.py", &format!("{AC}\ndef make() -> A:\n    return A()\n")),
        ("b.py", "from a import make\n"),
        ("app.py", "from a import *\nfrom b import *\n\ndef use():\n    make().run()\n"),
    ]);
    edge(&res, "app.py::use", "a.py::A.run", "i04b same declaration via two stars (recall)");
}

// =================== 3-segment and ordinary import resolution =============

fn src_layout(app: &'static str) -> ResolutionResult {
    resolve(&[
        ("src/a/__init__.py", ""),
        ("src/a/b/__init__.py", ""),
        ("src/a/b/c.py", "def f():\n    return 1\n"),
        ("app.py", app),
    ])
}

#[test]
fn t01_src_layout_three_segment() {
    let res = src_layout("import a.b.c\n\ndef use():\n    a.b.c.f()\n");
    edge(&res, "app.py::use", "src/a/b/c.py::f", "t01 src layout, one root");
}

#[test]
fn t01b_src_layout_from_forms() {
    let res = src_layout("from a.b import c\nfrom a.b.c import f\n\ndef use():\n    c.f()\n    f()\n");
    assert!(
        reaches(&res, "app.py::use", "src/a/b/c.py::f"),
        "t01b: {:?}",
        calls_from(&res, "app.py::use")
    );
}

#[test]
fn t01c_from_form_edges_equal_import_form() {
    let res = src_layout("from a.b.c import f\n\ndef use():\n    f()\n");
    edge(&res, "app.py::use", "src/a/b/c.py::f", "t01c from a.b.c import f");
    let res = src_layout("from a.b import c\n\ndef use():\n    c.f()\n");
    edge(&res, "app.py::use", "src/a/b/c.py::f", "t01c from a.b import c");
}

#[test]
fn t02_relative_imports_resolve() {
    let res = resolve(&[
        ("pkg/__init__.py", ""),
        ("pkg/c.py", "def f():\n    return 1\n"),
        ("pkg/sub/__init__.py", ""),
        ("pkg/sub/g.py", "def g():\n    return 2\n"),
        (
            "pkg/app.py",
            "from . import c\nfrom .c import f\nfrom .sub import g as gm\nfrom .sub.g import g\n\ndef use():\n    c.f()\n    f()\n    gm.g()\n    g()\n",
        ),
    ]);
    edge(&res, "pkg/app.py::use", "pkg/c.py::f", "t02 relative");
    edge(&res, "pkg/app.py::use", "pkg/sub/g.py::g", "t02 relative nested");
}

#[test]
fn t03_importer_directory_candidate_still_resolves() {
    let res = resolve(&[
        ("tool/sub/__init__.py", ""),
        ("tool/sub/mod.py", "def f():\n    return 1\n"),
        ("tool/run.py", "import sub.mod\n\ndef use():\n    sub.mod.f()\n"),
    ]);
    edge(&res, "tool/run.py::use", "tool/sub/mod.py::f", "t03 importer-relative absolute import");
}

#[test]
fn t04_importer_in_src_does_not_double_count_its_own_root() {
    // `src/x.py` importing `a.b.c`: the `src/` candidate and the importer's
    // directory candidate are the same file and must not read as two answers.
    let res = resolve(&[
        ("src/a/__init__.py", ""),
        ("src/a/b/__init__.py", ""),
        ("src/a/b/c.py", "def f():\n    return 1\n"),
        ("src/x.py", "import a.b.c\n\ndef use():\n    a.b.c.f()\n"),
    ]);
    edge(&res, "src/x.py::use", "src/a/b/c.py::f", "t04 importer inside the root");
}

#[test]
fn t05_two_submodules_of_one_package_imported_together() {
    let res = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("a/b/d.py", "def g():\n    return 2\n"),
        (
            "app.py",
            "import a.b.c\nimport a.b.d\n\ndef use():\n    a.b.c.f()\n    a.b.d.g()\n",
        ),
    ]);
    edge(&res, "app.py::use", "a/b/c.py::f", "t05 import a.b.c + import a.b.d (recall)");
    edge(&res, "app.py::use", "a/b/d.py::g", "t05 import a.b.c + import a.b.d (recall)");
}

#[test]
fn t05b_package_and_submodule_imported_together() {
    let res = resolve(&[
        ("a/__init__.py", "def top():\n    return 0\n"),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("app.py", "import a\nimport a.b.c\n\ndef use():\n    a.b.c.f()\n    a.top()\n"),
    ]);
    edge(&res, "app.py::use", "a/b/c.py::f", "t05b import a + import a.b.c (recall)");
}

#[test]
fn t06_unrelated_function_local_named_like_root() {
    let res = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        (
            "app.py",
            "import a.b.c\n\ndef other():\n    a = 3\n    return a\n\ndef use():\n    a.b.c.f()\n",
        ),
    ]);
    edge(&res, "app.py::use", "a/b/c.py::f", "t06 unrelated function's local `a` (recall)");
}

#[test]
#[ignore = "pre-existing, ft-c0afdfef follow-up: intended ambiguity cost, a second file of the dotted path makes the module path abstain"]
fn t07_unrelated_same_tail_elsewhere_in_corpus() {
    let res = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        ("docs/examples/a/b/c.py", "def f():\n    return 2\n"),
        ("app.py", "import a.b.c\n\ndef use():\n    a.b.c.f()\n"),
    ]);
    edge(&res, "app.py::use", "a/b/c.py::f", "t07 docs copy of the tail (recall)");
}

#[test]
fn t08_three_segment_with_conditional_import_same_target() {
    let res = resolve(&[
        ("a/__init__.py", ""),
        ("a/b/__init__.py", ""),
        ("a/b/c.py", "def f():\n    return 1\n"),
        (
            "app.py",
            "try:\n    import a.b.c\nexcept ImportError:\n    import a.b.c\n\ndef use():\n    a.b.c.f()\n",
        ),
    ]);
    edge(&res, "app.py::use", "a/b/c.py::f", "t08 same import in try and except (recall)");
}

// =================== regression harness: edge dump =======================

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
        let extractions: Vec<Extraction> =
            files.iter().map(|(p, s)| extract_file(p, s)).collect();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let res = resolver.resolve_all(&extractions).unwrap();
        let name = corpus.file_name().to_string_lossy().to_string();
        for e in &res.edges {
            lines.push(format!(
                "{name}\t{:?}\t{}\t{}\t{}\t{}\t{:?}",
                e.edge_kind, e.source_symbol, e.target_symbol, e.source_file, e.target_file, e.confidence
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
