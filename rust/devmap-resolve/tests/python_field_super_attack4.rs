//! Round-four adversarial probes for the Python heritage follow-up (042133bf):
//! `PYTHON_TYPING_ALIAS_METHODS`, the structural-only set, module-scope
//! rebinding marks, `PythonImportBind::Star`, the class-enclosing-scope read
//! and the `python_class_named` non-class guard. Each test states the correct
//! behaviour (CPython semantics); a failing test is a BREAK of the new code.

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

// ------------------------------------------------ 1. typing alias method table

/// `base.py::Base` defines every name; `c.py` has, per name, one class that
/// reaches it through `super()` and one through `self`, each with `head`
/// ahead of `Base` in the MRO. When `head` really defines the method the MRO
/// stops there, so any in-repo `Base.<name>` edge is wrong.
fn alias_leaks(import: &str, head: &str, names: &[&str]) -> Vec<String> {
    alias_leaks_via(import, head, names, false)
}

/// As [`alias_leaks`]; `with_self` also checks a `self.<name>()` call.
fn alias_leaks_via(import: &str, head: &str, names: &[&str], with_self: bool) -> Vec<String> {
    let mut base = String::from("class Base:\n");
    for name in names {
        base.push_str(&format!("    def {name}(self):\n        return 1\n"));
    }
    let mut c = format!("{import}\nfrom base import Base\n\n");
    for (i, name) in names.iter().enumerate() {
        c.push_str(&format!(
            "class S{i}({head}, Base):\n    def {name}(self):\n        return super().{name}()\n\n"
        ));
        c.push_str(&format!(
            "class T{i}({head}, Base):\n    def go(self):\n        return self.{name}()\n\n"
        ));
    }
    let r = resolve(&[("base.py", base.as_str()), ("c.py", c.as_str())]);
    let mut leaks = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let mut callers = vec![format!("c.py::S{i}.{name}")];
        if with_self {
            callers.push(format!("c.py::T{i}.go"));
        }
        for caller in callers {
            for target in methods(&r, &caller) {
                if target.starts_with("base.py::Base.") {
                    leaks.push(format!("{head}: {caller} -> {target}"));
                }
            }
        }
    }
    leaks
}

macro_rules! alias_case {
    ($test:ident, $import:expr, $head:expr, [$($name:expr),+ $(,)?]) => {
        #[test]
        fn $test() {
            let got = alias_leaks($import, $head, &[$($name),+]);
            assert!(got.is_empty(), "the alias defines these; the walk skipped past it: {got:#?}");
        }
    };
}

alias_case!(typing_counter_methods, "import typing", "typing.Counter",
    ["most_common", "elements", "subtract", "total", "update", "copy", "fromkeys", "get", "items", "keys", "values", "pop", "popitem", "setdefault", "clear"]);
alias_case!(typing_ordered_dict_methods, "import typing", "typing.OrderedDict",
    ["move_to_end", "popitem", "copy", "fromkeys", "update", "setdefault", "pop", "get", "keys", "items", "values", "clear"]);
alias_case!(typing_deque_methods, "import typing", "typing.Deque",
    ["append", "appendleft", "rotate", "extendleft", "extend", "popleft", "pop", "maxlen", "copy", "count", "index", "insert", "remove", "reverse", "clear"]);
alias_case!(typing_defaultdict_methods, "import typing", "typing.DefaultDict",
    ["default_factory", "copy", "fromkeys", "get", "items", "keys", "values", "pop", "popitem", "setdefault", "update", "clear"]);
alias_case!(typing_defaultdict_dunder_missing, "import typing", "typing.DefaultDict", ["__missing__"]);
alias_case!(typing_chainmap_methods, "import typing", "typing.ChainMap",
    ["new_child", "parents", "maps", "copy", "get", "items", "keys", "values", "pop", "popitem", "setdefault", "update", "clear", "fromkeys"]);
alias_case!(typing_dict_methods, "import typing", "typing.Dict",
    ["clear", "copy", "fromkeys", "get", "items", "keys", "pop", "popitem", "setdefault", "update", "values"]);
alias_case!(typing_list_methods, "import typing", "typing.List",
    ["append", "clear", "copy", "count", "extend", "index", "insert", "pop", "remove", "reverse", "sort"]);
alias_case!(typing_set_methods, "import typing", "typing.Set",
    ["add", "clear", "copy", "difference", "difference_update", "discard", "intersection", "intersection_update", "isdisjoint", "issubset", "issuperset", "pop", "remove", "symmetric_difference", "symmetric_difference_update", "union", "update"]);
alias_case!(typing_frozenset_methods, "import typing", "typing.FrozenSet",
    ["copy", "difference", "intersection", "isdisjoint", "issubset", "issuperset", "symmetric_difference", "union"]);
alias_case!(typing_tuple_methods, "import typing", "typing.Tuple", ["count", "index"]);
alias_case!(typing_mapping_mixins, "import typing", "typing.Mapping", ["get", "keys", "items", "values"]);
alias_case!(typing_mutable_mapping_mixins, "import typing", "typing.MutableMapping",
    ["get", "keys", "items", "values", "pop", "popitem", "clear", "update", "setdefault"]);
alias_case!(typing_sequence_mixins, "import typing", "typing.Sequence", ["index", "count"]);
alias_case!(typing_mutable_sequence_mixins, "import typing", "typing.MutableSequence",
    ["index", "count", "insert", "append", "reverse", "extend", "pop", "remove", "clear"]);
alias_case!(typing_mutable_sequence_iadd, "import typing", "typing.MutableSequence", ["__iadd__"]);
alias_case!(typing_abstract_set_private_mixins, "import typing", "typing.AbstractSet", ["_hash", "_from_iterable"]);
alias_case!(typing_mutable_set_mixins, "import typing", "typing.MutableSet",
    ["add", "discard", "remove", "pop", "clear", "isdisjoint", "_hash", "_from_iterable"]);
alias_case!(typing_keys_view_private, "import typing", "typing.KeysView", ["isdisjoint", "_from_iterable", "_hash"]);
alias_case!(typing_iterator_generator_methods, "import typing", "typing.Generator", ["send", "throw", "close"]);
alias_case!(typing_iterator_dunders, "import typing", "typing.Iterator", ["__next__", "__iter__"]);
alias_case!(typing_async_generator_methods, "import typing", "typing.AsyncGenerator", ["asend", "athrow", "aclose"]);
alias_case!(typing_coroutine_methods, "import typing", "typing.Coroutine", ["send", "throw", "close"]);
alias_case!(typing_namedtuple_methods, "import typing", "typing.NamedTuple",
    ["_asdict", "_replace", "_fields", "_make", "_field_defaults", "count", "index"]);
alias_case!(typing_typeddict_methods, "import typing", "typing.TypedDict",
    ["copy", "get", "items", "keys", "pop", "popitem", "setdefault", "update", "values", "clear", "fromkeys"]);
alias_case!(typing_io_methods, "import typing", "typing.IO",
    ["read", "write", "readline", "readlines", "writelines", "seek", "tell", "flush", "fileno", "isatty", "readable", "seekable", "writable", "truncate", "close", "closed", "mode", "name"]);
alias_case!(typing_textio_methods, "import typing", "typing.TextIO",
    ["read", "write", "readline", "seek", "buffer", "encoding", "errors", "line_buffering", "newlines"]);
alias_case!(typing_binaryio_methods, "import typing", "typing.BinaryIO", ["read", "write", "readline", "seek", "close"]);
alias_case!(typing_pattern_methods, "import typing", "typing.Pattern",
    ["search", "match", "fullmatch", "split", "findall", "finditer", "sub", "subn", "pattern", "flags", "groups", "groupindex"]);
alias_case!(typing_match_methods, "import typing", "typing.Match",
    ["group", "groups", "groupdict", "start", "end", "span", "expand", "string", "re", "pos", "endpos", "lastindex", "lastgroup", "regs"]);
alias_case!(typing_text_is_str, "import typing", "typing.Text",
    ["upper", "lower", "split", "strip", "join", "replace", "format", "encode", "startswith", "endswith", "find", "title"]);
alias_case!(typing_type_is_type, "import typing", "typing.Type", ["mro"]);
alias_case!(typing_awaitable_dunder, "import typing", "typing.Awaitable", ["__await__"]);
alias_case!(typing_context_manager_dunder, "import typing", "typing.ContextManager", ["__enter__", "__exit__"]);

// Spellings of the same member.
alias_case!(from_import_spelling_most_common, "from typing import Counter", "Counter", ["most_common"]);
alias_case!(aliased_module_spelling_pattern, "import typing as t", "t.Pattern", ["search"]);
alias_case!(subscripted_spelling_default_factory, "import typing", "typing.DefaultDict[str, int]", ["default_factory"]);
alias_case!(typing_extensions_counter_total, "import typing_extensions as te", "te.Counter", ["total", "most_common"]);
alias_case!(typing_extensions_text, "import typing_extensions as te", "te.Text", ["upper"]);

// A typing name the table has no row for must stay fully opaque: nothing is
// known about what it defines, so the walk may never pass it.
alias_case!(unknown_typing_name_is_opaque, "import typing", "typing.NoSuchTypingName", ["m"]);
alias_case!(typing_extensions_doc_is_opaque, "import typing_extensions as te", "te.Doc", ["documentation"]);
alias_case!(typing_extensions_future_name_is_opaque, "import typing_extensions as te", "te.FutureNamedThing", ["m"]);
alias_case!(typing_newtype_is_opaque, "import typing", "typing.NewType", ["m"]);
alias_case!(typing_any_is_opaque, "import typing", "typing.Any", ["m"]);
alias_case!(typing_forward_ref_is_opaque, "import typing", "typing.ForwardRef", ["_evaluate"]);

/// `self.most_common()` in a `Counter` subclass that lists an in-repo class
/// after it: the MRO stops at `Counter`. (Checked against 103272ea to say
/// whether a failure is a regression.)
#[test]
fn self_call_through_a_container_alias_stops_at_the_alias() {
    let got = alias_leaks_via("import typing", "typing.Counter", &["most_common"], true);
    assert!(got.is_empty(), "{got:#?}");
}

/// The table is a union, so a `Mapping` base is asked "might ANY alias define
/// `append`". CPython: `Mapping` defines no `append`, the walk reaches
/// `Base.append`. Recall loss of the union design (not a wrong edge).
#[test]
fn mapping_has_no_append_so_the_walk_reaches_base() {
    let base = "class Base:\n    def append(self):\n        return 1\n";
    let c = "import typing\nfrom base import Base\n\nclass C(typing.Mapping, Base):\n    def append(self):\n        return super().append()\n";
    let r = resolve(&[("base.py", base), ("c.py", c)]);
    assert_eq!(methods(&r, "c.py::C.append"), ["base.py::Base.append"]);
}

/// The provenance comment says a name outside the union "is not defined by
/// any of them". Test the claim against the named classes' own `dir()`: every
/// public (non-dunder) attribute CPython 3.12 gives the class must be in the
/// table. This list is the audit output of `dir()` for the aliases that miss.
#[test]
fn table_covers_every_public_attribute_of_the_classes_it_names() {
    let missing: &[(&str, &str)] = &[
        ("typing.DefaultDict", "default_factory"),
        ("typing.TextIO", "errors"),
        ("typing.TextIO", "line_buffering"),
        ("typing.TextIO", "newlines"),
        ("typing.Pattern", "search"),
        ("typing.Match", "group"),
        ("typing.AbstractSet", "_hash"),
        ("typing.AbstractSet", "_from_iterable"),
    ];
    let mut leaks = Vec::new();
    for (head, name) in missing {
        leaks.extend(alias_leaks("import typing", head, &[name]));
    }
    assert!(leaks.is_empty(), "{leaks:#?}");
}

// ---------------------------------------------------- 2. structural-only set

#[test]
fn protocol_subclass_default_body_is_reached_by_super() {
    let src = "from typing import Protocol\n\nclass P(Protocol):\n    def m(self):\n        return 1\n\nclass C(P):\n    def m(self):\n        return super().m()\n";
    let r = resolve(&[("c.py", src)]);
    assert_eq!(methods(&r, "c.py::C.m"), ["c.py::P.m"]);
}

#[test]
fn protocol_spellings_are_all_structural() {
    for import_and_head in [
        ("from typing import Protocol as P", "P"),
        ("import typing_extensions as te", "te.Protocol"),
        ("from typing_extensions import Protocol", "Protocol"),
        ("import typing as t", "t.Protocol"),
        ("from typing import Protocol as Generic", "Generic"),
    ] {
        let (import, head) = import_and_head;
        let src = format!(
            "{import}\nfrom base import Base\n\nclass C({head}, Base):\n    def m(self):\n        return super().m()\n"
        );
        let r = resolve(&[("base.py", "class Base:\n    def m(self):\n        return 1\n"), ("c.py", src.as_str())]);
        assert_eq!(methods(&r, "c.py::C.m"), ["base.py::Base.m"], "{import_and_head:?}");
    }
}

#[test]
fn abc_init_subclass_reaches_base() {
    // ABC defines no `__init_subclass__`; `object`'s is last.
    let base = "class Base:\n    def __init_subclass__(cls, **kw):\n        pass\n";
    let c = "import abc\nfrom base import Base\n\nclass C(abc.ABC, Base):\n    def __init_subclass__(cls, **kw):\n        super().__init_subclass__(**kw)\n";
    let r = resolve(&[("base.py", base), ("c.py", c)]);
    assert_eq!(methods(&r, "c.py::C.__init_subclass__"), ["base.py::Base.__init_subclass__"]);
}

#[test]
fn abcmeta_register_stays_opaque() {
    let base = "class Base:\n    def register(self):\n        return 1\n";
    let c = "import abc\nfrom base import Base\n\nclass C(abc.ABCMeta, Base):\n    def register(cls, k):\n        return super().register(k)\n";
    let r = resolve(&[("base.py", base), ("c.py", c)]);
    assert!(methods(&r, "c.py::C.register").is_empty());
}

#[test]
fn a_local_typing_module_is_not_the_stdlib_structural_marker() {
    // `typing.py` is in the corpus, and `Generic` there is a real class with `m`.
    let typing = "class Generic:\n    def m(self):\n        return 1\n";
    let c = "from typing import Generic\nfrom base import Base\n\nclass C(Generic, Base):\n    def m(self):\n        return super().m()\n";
    let r = resolve(&[("base.py", "class Base:\n    def m(self):\n        return 2\n"), ("typing.py", typing), ("c.py", c)]);
    assert_eq!(methods(&r, "c.py::C.m"), ["typing.py::Generic.m"]);
}

// ----------------------------------------------------- scope / rebinding marks

fn field_go(extra: &[(&str, &str)], h: &str) -> Vec<String> {
    let mut files: Vec<(&str, &str)> =
        vec![("a.py", "class A:\n    def run(self):\n        return 1\n")];
    files.extend_from_slice(extra);
    files.push(("h.py", h));
    go_edges(&resolve(&files))
}

/// The in-class call targets of every `…H.go` in `h.py` (a class nested in a
/// function is `build.H`, so the caller is not always `h.py::H.go`).
fn go_edges(r: &ResolutionResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .edges
        .iter()
        .filter(|e| {
            e.edge_kind == EdgeKind::Calls
                && e.source_symbol.starts_with("h.py::")
                && e.source_symbol.ends_with("H.go")
        })
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

const H_TAIL: &str = "\nclass H:\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n";
const A_RUN: [&str; 1] = ["a.py::A.run"];

fn h_with(prefix: &str) -> Vec<String> {
    field_go(&[], &format!("from a import A\n{prefix}{H_TAIL}"))
}

// Marks that must NOT poison (recall).

#[test]
fn del_inside_a_function_does_not_mark_module_scope() {
    assert_eq!(h_with("def f(A):\n    del A\n"), A_RUN);
}

#[test]
fn for_inside_a_function_does_not_mark_module_scope() {
    assert_eq!(h_with("def f(xs):\n    for A in xs:\n        pass\n"), A_RUN);
}

#[test]
fn global_declared_but_only_read_does_not_mark_module_scope() {
    assert_eq!(h_with("def f():\n    global A\n    return A\n"), A_RUN);
}

#[test]
fn comprehension_target_does_not_mark_module_scope() {
    assert_eq!(h_with("L = [A for A in range(3)]\n"), A_RUN);
}

#[test]
fn with_alias_in_a_function_does_not_mark_module_scope() {
    assert_eq!(h_with("def f():\n    with g() as A:\n        pass\n"), A_RUN);
}

#[test]
fn dunder_all_does_not_rebind() {
    assert_eq!(h_with("__all__ = ['A']\n"), A_RUN);
}

#[test]
fn class_body_for_does_not_mark_module_scope() {
    assert_eq!(h_with("class K:\n    for A in range(3):\n        pass\n"), A_RUN);
}

#[test]
fn star_import_before_the_from_import_still_binds() {
    let got = field_go(
        &[("m.py", "X = 1\n")],
        &format!("from m import *\nfrom a import A\n{H_TAIL}"),
    );
    assert_eq!(got, A_RUN);
}

#[test]
fn star_import_of_a_module_that_cannot_define_the_name_still_binds() {
    // `m` exports only `X` (`__all__`): the star cannot rebind `A`.
    let got = field_go(
        &[("m.py", "__all__ = ['X']\nX = 1\nA = 2\n")],
        &format!("from a import A\nfrom m import *\n{H_TAIL}"),
    );
    assert_eq!(got, A_RUN, "recall loss: star after the import abstains the whole file");
}

#[test]
fn star_import_of_an_unrelated_module_does_not_poison_a_typing_skip() {
    // `Generic` is imported, then a star import of a module that has no
    // `Generic`: the skip over `Generic` must still hold.
    let src = "from typing import Generic, TypeVar\nfrom m import *\nfrom base import Base\nT = TypeVar('T')\n\nclass C(Generic[T], Base):\n    def m(self):\n        return super().m()\n";
    let r = resolve(&[
        ("base.py", "class Base:\n    def m(self):\n        return 2\n"),
        ("m.py", "X = 1\n"),
        ("c.py", src),
    ]);
    assert_eq!(methods(&r, "c.py::C.m"), ["base.py::Base.m"]);
}

// Marks that MUST poison (wrong edge if they do not).

#[test]
fn match_capture_pattern_rebinds_the_name() {
    for case in ["[A, _]", "{'k': A}", "P(x=A)", "_ as A", "(1, A)", "[*A]", "{**A}"] {
        let got = h_with(&format!("match V:\n    case {case}:\n        pass\n"));
        assert!(got.is_empty(), "`case {case}` binds A at module scope: {got:?}");
    }
}

#[test]
fn annotated_and_chained_and_augmented_assignment_rebind() {
    for prefix in ["A: type = object\n", "A = B = object\n", "B, A = 1, object\n", "A, = (object,)\n", "[A] = [object]\n"] {
        let got = h_with(prefix);
        assert!(got.is_empty(), "{prefix:?}: {got:?}");
    }
}

#[test]
fn parenthesised_and_list_del_for_with_targets_rebind() {
    for prefix in [
        "del (A)\n",
        "del [A]\n",
        "del (A,)\n",
        "for [A] in X:\n    pass\n",
        "for (A) in X:\n    pass\n",
        "with f() as [A]:\n    pass\n",
        "with f() as (A):\n    pass\n",
        "with f() as (B, A):\n    pass\n",
        "try:\n    pass\nexcept* E as A:\n    pass\n",
        "async def _f():\n    pass\n[A := 1 for _ in X]\n",
    ] {
        let got = h_with(prefix);
        assert!(got.is_empty(), "{prefix:?}: {got:?}");
    }
}

#[test]
fn type_alias_statement_rebinds_the_name() {
    let got = h_with("type A = int\n");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn global_declared_in_a_method_and_assigned_rebinds() {
    let got = h_with("class K:\n    def f(self):\n        global A\n        A = None\n");
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn try_import_fallback_import_rebinds() {
    let got = field_go(
        &[("b.py", "class A:\n    def run(self):\n        return 2\n")],
        &format!("try:\n    from a import A\nexcept ImportError:\n    from b import A\n{H_TAIL}"),
    );
    assert!(got.is_empty(), "two imports of A, bound one: {got:?}");
}

// ----------------------------------------- 5. class enclosing scope (`build.H`)

#[test]
fn nested_class_two_functions_deep_sees_the_outer_local() {
    let got = field_go(
        &[],
        "from a import A\n\ndef outer():\n    A = object\n    def inner():\n        class H:\n            def __init__(self):\n                self.x = A()\n            def go(self):\n                return self.x.run()\n        return H\n    return inner\n",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn class_inside_a_method_named_build_sees_the_methods_local() {
    let got = field_go(
        &[],
        "from a import A\n\nclass K:\n    def build(self):\n        A = object\n        class H:\n            def __init__(self):\n                self.x = A()\n            def go(self):\n                return self.x.run()\n        return H\n",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn function_build_and_module_class_build_with_nested_h_collide() {
    // `build.H` is the identity of both the class in the function and the
    // nested class of the module-level `class build`. The function's `A`
    // local poisons only the function's `H`.
    let got = field_go(
        &[],
        "from a import A\n\ndef build():\n    A = object\n    class H:\n        def __init__(self):\n            self.x = A()\n        def go(self):\n            return self.x.run()\n    return H\n\nclass build:\n    class H:\n        def go(self):\n            return 1\n",
    );
    assert!(got.is_empty(), "the function's H reads its local A: {got:?}");
}

#[test]
fn module_class_build_nested_h_does_not_inherit_a_functions_local() {
    // Module-level `class build: class H` reads the MODULE `A`. A sibling
    // function `build` with a local `A` must not poison it (recall).
    let src = "from a import A\n\ndef build():\n    A = object\n    return A\n\nclass build:\n    class H:\n        def __init__(self):\n            self.x = A()\n        def go(self):\n            return self.x.run()\n";
    let r = resolve(&[("a.py", "class A:\n    def run(self):\n        return 1\n"), ("h.py", src)]);
    assert_eq!(go_edges(&r), A_RUN);
}

#[test]
fn a_function_named_like_a_module_class_keeps_its_locals() {
    // `def build` then `class build`: the scope `h.py::build` is read as a
    // class scope, so the function's local `A` is skipped as a "class body".
    let got = field_go(
        &[],
        "from a import A\n\ndef build():\n    A = object\n    class H:\n        def __init__(self):\n            self.x = A()\n        def go(self):\n            return self.x.run()\n    return H\n\nclass build:\n    pass\n",
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn redefined_function_build_second_has_no_local_binds() {
    // Two `build` functions; `H` lives in the second, which has no local `A`.
    let src = "from a import A\n\ndef build():\n    A = object\n    return A\n\ndef build():\n    class H:\n        def __init__(self):\n            self.x = A()\n        def go(self):\n            return self.x.run()\n    return H\n";
    let r = resolve(&[("a.py", "class A:\n    def run(self):\n        return 1\n"), ("h.py", src)]);
    assert_eq!(go_edges(&r), A_RUN);
}

#[test]
fn class_in_a_lambda_default_is_not_the_lambdas_local() {
    let got = field_go(
        &[],
        "from a import A\n\nF = lambda A=object: A\n\nclass H:\n    def __init__(self):\n        self.x = A()\n    def go(self):\n        return self.x.run()\n",
    );
    assert_eq!(got, A_RUN);
}

// ------------------------------------------------- 7. python_class_named guard

#[test]
fn class_in_one_branch_function_in_the_other_is_not_one_class() {
    let src = "from typing import TYPE_CHECKING\nfrom base import Base\n\nif TYPE_CHECKING:\n    class Impl:\n        def m(self):\n            return 1\nelse:\n    def Impl(*a):\n        return None\n\nclass C(Impl):\n    def m(self):\n        return super().m()\n";
    let r = resolve(&[("base.py", "class Base:\n    pass\n"), ("c.py", src)]);
    assert!(
        methods(&r, "c.py::C.m").is_empty(),
        "`Impl` is a function when TYPE_CHECKING is false: {:?}",
        methods(&r, "c.py::C.m")
    );
}

#[test]
fn type_checking_import_with_no_else_still_binds() {
    let src = "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from base import Base\n\nclass C(Base):\n    def m(self):\n        return super().m()\n";
    let r = resolve(&[("base.py", "class Base:\n    def m(self):\n        return 1\n"), ("c.py", src)]);
    assert_eq!(methods(&r, "c.py::C.m"), ["base.py::Base.m"]);
}

#[test]
fn type_checking_import_else_assignment_abstains() {
    let src = "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from base import Base\nelse:\n    Base = object\n\nclass C(Base):\n    def m(self):\n        return super().m()\n";
    let r = resolve(&[("base.py", "class Base:\n    def m(self):\n        return 1\n"), ("c.py", src)]);
    assert!(methods(&r, "c.py::C.m").is_empty());
}

#[test]
fn type_checking_import_else_other_import_abstains() {
    let src = "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from base import Base\nelse:\n    from other import Base\n\nclass C(Base):\n    def m(self):\n        return super().m()\n";
    let r = resolve(&[
        ("base.py", "class Base:\n    def m(self):\n        return 1\n"),
        ("other.py", "class Base:\n    def m(self):\n        return 2\n"),
        ("c.py", src),
    ]);
    assert!(methods(&r, "c.py::C.m").is_empty(), "{:?}", methods(&r, "c.py::C.m"));
}

#[test]
fn type_checking_import_else_def_with_self_call_abstains() {
    // The same, through a `self.m()` call rather than `super()`.
    let src = "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from base import Base\nelse:\n    def Base():\n        pass\n\nclass C(Base):\n    def go(self):\n        return self.m()\n";
    let r = resolve(&[("base.py", "class Base:\n    def m(self):\n        return 1\n"), ("c.py", src)]);
    assert!(methods(&r, "c.py::C.go").is_empty());
}
