//! Regression coverage for C, C++, Objective-C, CUDA and Metal extraction.
//!
//! Two family-wide defects motivated this file, both measured on real code
//! before being fixed:
//!
//! 1. **No C-family call was extracted at all.** The family reached
//!    `extract_node`'s generic arm, which emits declarations and nothing else,
//!    so a 183-file first-party corpus produced **0** `Calls` edges and a
//!    9,082-file C++ header corpus produced 0. Every consumer of the call graph
//!    — `impact`, `trace`, dead code, the PDG — answered for the whole family
//!    from nothing.
//! 2. **No C-family symbol could be reported dead.** `generic_is_exported`
//!    falls back to `!name.starts_with('_')`, which answered `true` for 1,393
//!    of 1,464 C-family symbols on that corpus, so every one was exempt.
//!    Separately, `callable_binding_name` looks for a `name` field no C-family
//!    declaration carries, so every reference and call made inside a C-family
//!    function was attributed to the *file* rather than to the function: all
//!    117 C-family `References` edges on that corpus had the file as source.

use devmap_extract::extract_file;
use devmap_extract::model::*;
use std::collections::HashSet;

fn callees(extraction: &Extraction) -> Vec<&str> {
    extraction
        .calls
        .iter()
        .map(|call| call.callee_name.as_str())
        .collect()
}

fn symbol_names(extraction: &Extraction) -> Vec<&str> {
    extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect()
}

/// Every C-family grammar must extract calls.
///
/// Pre-fix this asserted nothing that could pass: `extraction.calls` was empty
/// for all five languages. Metal is included because it borrows the C++ grammar
/// and is therefore the language most likely to be missed by a fix aimed at
/// C++ alone.
#[test]
fn every_c_family_language_extracts_calls() {
    for (path, source, expected) in [
        (
            "unit.c",
            "int helper(int a){return a;}\nint run(void){ return helper(1); }\n",
            "helper",
        ),
        (
            "unit.cpp",
            "int helper(int a){return a;}\nint run(){ return helper(1); }\n",
            "helper",
        ),
        (
            "unit.m",
            "@implementation Foo\n- (int)run { return cHelper(1); }\n@end\n",
            "cHelper",
        ),
        (
            "unit.cu",
            "__device__ int helper(int a){return a;}\n__global__ void kern(){ helper(1); }\n",
            "helper",
        ),
        (
            "unit.metal",
            "float helper(float a){ return a; }\nkernel void reduce(){ helper(1.0); }\n",
            "helper",
        ),
        (
            "unit.mm",
            "int helper(int a){return a;}\nint run(){ return helper(1); }\n",
            "helper",
        ),
        (
            "unit.cuh",
            "__device__ int helper(int a){return a;}\n__device__ int run(){ return helper(1); }\n",
            "helper",
        ),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            callees(&extraction).contains(&expected),
            "{path}: the C family must extract calls; got {:?}",
            callees(&extraction)
        );
    }
}

/// Every C++ call shape a real codebase uses.
///
/// Each of these recorded either nothing or a whole expression as the callee
/// before the fix. They are asserted together because they share one owner,
/// `split_call_target`, and fixing one shape while leaving another is the
/// failure mode SC26 recorded for Rust.
#[test]
fn cpp_call_shapes_all_name_the_callee() {
    let extraction = extract_file(
        "shapes.cpp",
        concat!(
            "namespace ns { int nsFn(int); }\n",
            "template <typename T> T tmplFn(T);\n",
            "struct S { int method(int); static int stat(int); };\n",
            "int freeFn(int);\n",
            "int driver(S s, S* p, int (*fp)(int)) {\n",
            "  int a = freeFn(1);\n",
            "  int b = s.method(2);\n",
            "  int c = p->method(3);\n",
            "  int d = ns::nsFn(4);\n",
            "  int e = tmplFn<int>(5);\n",
            "  int f = S::stat(6);\n",
            "  int g = fp(7);\n",
            "  int h = (*fp)(8);\n",
            "  S* made = new S();\n",
            "  return a+b+c+d+e+f+g+h;\n",
            "}\n",
        ),
    );
    let found: HashSet<&str> = callees(&extraction).into_iter().collect();
    for expected in [
        "freeFn", "method", "nsFn", "tmplFn", "stat",
        "fp", // function pointer, plain and deref
        "S",  // `new S()` constructs S
    ] {
        assert!(
            found.contains(expected),
            "call shape for {expected:?} must name its callee; got {found:?}"
        );
    }
    // A namespaced or static-member call keeps its qualifier as the receiver,
    // which is what lets resolution find the owning type rather than guessing.
    let receiver_of = |callee: &str| -> Option<String> {
        extraction
            .calls
            .iter()
            .find(|call| call.callee_name == callee)
            .and_then(|call| call.receiver_expr.clone())
    };
    assert_eq!(receiver_of("nsFn").as_deref(), Some("ns"));
    assert_eq!(receiver_of("stat").as_deref(), Some("S"));
    assert_eq!(receiver_of("method").as_deref(), Some("s"));
    // No callee may keep type arguments or a whole path in its name.
    for callee in callees(&extraction) {
        assert!(
            !callee.contains('<') && !callee.contains("::") && !callee.contains('*'),
            "a callee must be a name, not an expression: {callee:?}"
        );
    }
}

/// A call made inside a C-family function is attributed to that function.
///
/// Pre-fix `callable_binding_name` read a `name` field that no C-family
/// declaration has, so `caller_symbol` was `None` and every call and reference
/// was recorded against the file. That is the SC9/SC10 unjoinable-edge shape:
/// a traversal out of a C function node found none of its calls.
#[test]
fn a_c_family_call_names_its_enclosing_function() {
    let extraction = extract_file(
        "scope.c",
        "int helper(int a){return a;}\nint caller(void){ return helper(1); }\n",
    );
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "helper")
        .expect("the call must be extracted at all");
    assert_eq!(
        call.caller_symbol.as_deref(),
        Some("scope.c::caller"),
        "a call inside a function belongs to the function, not to the file"
    );
}

/// Identity and call scope must agree for every C-family callable.
///
/// SC14 recorded that deriving a member's identity and its call scope through
/// two independent walks is what makes them silently disagree, and SC9/SC10
/// recorded the cost: edges naming a source symbol no node carries. This asserts
/// the invariant directly — for an in-class definition, an out-of-line
/// definition, and an Objective-C method.
#[test]
fn every_c_family_call_source_matches_a_real_symbol() {
    for (path, source) in [
        (
            "member.cpp",
            concat!(
                "int helper(int);\n",
                "struct S {\n",
                "  int inClass(int a) { return helper(a); }\n",
                "  int outOfLine(int a);\n",
                "};\n",
                "int S::outOfLine(int a) { return helper(a); }\n",
            ),
        ),
        (
            "member.m",
            concat!(
                "@implementation Foo\n",
                "- (int)a:(int)x b:(int)y { return cHelper(x); }\n",
                "- (int)run { return [self a:1 b:2]; }\n",
                "@end\n",
            ),
        ),
        (
            "free.cu",
            "__device__ int helper(int);\n__global__ void kern(){ helper(1); }\n",
        ),
    ] {
        let extraction = extract_file(path, source);
        let identities: HashSet<&str> = symbol_names(&extraction).into_iter().collect();
        assert!(
            !extraction.calls.is_empty(),
            "{path}: expected calls to exist"
        );
        for call in &extraction.calls {
            let Some(caller) = call.caller_symbol.as_deref() else {
                continue; // file scope is legitimate
            };
            assert!(
                identities.contains(caller),
                "{path}: call to {:?} names source {caller:?}, which matches no symbol: {identities:?}",
                call.callee_name
            );
        }
    }
}

/// An out-of-line C++ member definition is the same symbol as its declaration.
///
/// `int S::outOfLine(int)` previously became `file::S::outOfLine` — a name
/// carrying `::`, which no `s.outOfLine()` call site can ever match, so the
/// definition was unreachable and would have become a dead-code candidate the
/// moment C-family visibility started meaning anything.
#[test]
fn an_out_of_line_member_definition_is_owned_by_its_type() {
    let extraction = extract_file(
        "outofline.cpp",
        "struct S { int m(int a); };\nint S::m(int a) { return a; }\nnamespace ns { struct T { void q(); }; }\nvoid ns::T::q() {}\n",
    );
    let names = symbol_names(&extraction);
    assert!(
        names.contains(&"outofline.cpp::S.m"),
        "an out-of-line member belongs to its type: {names:?}"
    );
    assert!(
        names.contains(&"outofline.cpp::T.q"),
        "the innermost scope owns the member, not the namespace: {names:?}"
    );
    for name in &names {
        assert!(
            !name.trim_start_matches("outofline.cpp::").contains("::"),
            "a qualified declarator must be split, not kept as a name: {name:?}"
        );
    }
}

/// Visibility comes from the header, not from the identifier's first character.
///
/// `generic_is_exported`'s fallback is `!name.starts_with('_')`, which made
/// 1,393 of 1,464 C-family symbols "exported" and therefore exempt from
/// dead-code reporting. A header is the declaration surface of a C-family unit;
/// a definition in an implementation file is not itself an export.
#[test]
fn c_family_visibility_follows_the_header_and_explicit_markers() {
    let header = extract_file("api.h", "struct Api { int x; };\n");
    assert!(
        header
            .symbols
            .iter()
            .filter(|symbol| symbol.kind != SymbolKind::File)
            .all(|symbol| symbol.is_exported),
        "everything declared in a header is public API"
    );

    let implementation = extract_file(
        "api.c",
        concat!(
            "int ordinary(int a){return a;}\n",
            "static int internal(int a){return a;}\n",
            "extern \"C\" int ffi(int a){return a;}\n",
            "__attribute__((visibility(\"default\"))) int shared(int a){return a;}\n",
        ),
    );
    let exported = |name: &str| {
        implementation
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("{name} must be extracted"))
            .is_exported
    };
    assert!(
        !exported("ordinary"),
        "a definition in an implementation file is not an export; the header publishes it"
    );
    assert!(!exported("internal"), "`static` has internal linkage");
    assert!(
        exported("ffi"),
        "`extern \"C\"` exists to be called from outside this corpus"
    );
    assert!(
        exported("shared"),
        "an explicit visibility attribute is direct evidence of an external interface"
    );
}

/// An attribute's contents are not calls.
///
/// `__attribute__((visibility("default")))` parses its argument list as a real
/// `call_expression`, so without a guard every such attribute emits a call to
/// `visibility` — a callee naming nothing in the program, the phantom-callee
/// class SC17 closed for JSX and Go.
#[test]
fn an_attribute_is_not_a_call() {
    let extraction = extract_file(
        "attr.c",
        concat!(
            "int helper(int);\n",
            "__attribute__((visibility(\"default\"))) int f(int a){ return helper(a); }\n",
        ),
    );
    // Asserted together so the test cannot pass merely because no call was
    // extracted at all, which is exactly what it would have done before the
    // family had any call extraction.
    assert!(
        callees(&extraction).contains(&"helper"),
        "the real call in the body must be extracted: {:?}",
        callees(&extraction)
    );
    assert!(
        !callees(&extraction).contains(&"visibility"),
        "an attribute's contents must never be recorded as a call: {:?}",
        callees(&extraction)
    );
}

/// A mention of an elaborated type is a use, not a declaration.
///
/// C spells `struct arena_chunk *prev` with the same node kind as
/// `struct arena_chunk { … }`; only the definition has a body. Emitting a symbol
/// for each mention gave `swift-cmark`'s `arena.c` **eight** `arena_chunk` nodes
/// for one struct and `cmark_node` nine across the corpus. Duplicate qualified
/// names are a broken join key, and they also made every reference to the type
/// ambiguous: fixing this recovered 841 real type-reference edges.
#[test]
fn an_elaborated_type_use_does_not_redeclare_the_type() {
    let extraction = extract_file(
        "arena.c",
        concat!(
            "struct chunk { int sz; struct chunk *prev; };\n",
            "static struct chunk *alloc(struct chunk *prev) {\n",
            "  struct chunk *c = 0;\n",
            "  return c;\n",
            "}\n",
            "enum kind { A };\n",
            "enum kind pick(enum kind k) { return k; }\n",
        ),
    );
    for type_name in ["chunk", "kind"] {
        let declared = extraction
            .symbols
            .iter()
            .filter(|symbol| symbol.name == type_name)
            .count();
        assert_eq!(
            declared, 1,
            "{type_name} is declared once and used many times; got {declared} symbols"
        );
    }
    // The invariant this protects, stated directly.
    let mut seen = HashSet::new();
    for symbol in &extraction.symbols {
        assert!(
            seen.insert(symbol.qualified_name.as_str()),
            "duplicate identity {:?} is a broken join key",
            symbol.qualified_name
        );
    }
}

/// A function-like macro with a block argument is not a function.
///
/// `PYBIND11_MODULE(NAME, m) { … }` and `TORCH_LIBRARY(ops, m) { … }` have a
/// function's shape and the grammar runs no preprocessor, so each parsed as a
/// `function_definition` and — once C-family visibility started meaning
/// something — each became a confident dead-code candidate for a function that
/// does not exist. The discriminator is the missing return type.
#[test]
fn a_function_like_macro_is_not_a_function_symbol() {
    let extraction = extract_file(
        "binding.cpp",
        concat!(
            "void real(int a);\n",
            "PYBIND11_MODULE(TORCH_EXTENSION_NAME, m) { m.def(\"forward\", &real); }\n",
            "int typed(int a) { return a; }\n",
        ),
    );
    let names = symbol_names(&extraction);
    assert!(
        !names.contains(&"binding.cpp::PYBIND11_MODULE"),
        "a macro invocation is not a function definition: {names:?}"
    );
    assert!(
        names.contains(&"binding.cpp::typed"),
        "a real definition with a return type is unaffected: {names:?}"
    );
    // Suppressing the symbol must also suppress the scope, or the calls inside
    // the macro body name a source symbol no node carries (SC9/SC10).
    let identities: HashSet<&str> = names.iter().copied().collect();
    for call in &extraction.calls {
        if let Some(caller) = call.caller_symbol.as_deref() {
            assert!(
                identities.contains(caller),
                "call to {:?} names source {caller:?}, which matches no symbol",
                call.callee_name
            );
        }
    }
}

/// A construct the grammar could not parse is not a function definition.
///
/// `.h` is ambiguous between C and C++ and routes to the C grammar, which has
/// no notion of a namespace, so `namespace at { … }` parses as a
/// `function_definition` with `namespace` as the return type and `at` as the
/// whole declarator. LibTorch declares `namespace at` in 2,486 of 3,028 sampled
/// headers; taking each as a function put 2,486 nodes named `at` into one
/// corpus, and a call to any name that large a candidate set covers then
/// materialises the resolver's full ambiguity cross-product. `struct TORCH_API
/// Foo { … }` misparses the same way, the macro standing where the grammar
/// expects a declarator.
///
/// The discriminator is the parameter list: every real definition declares one.
#[test]
fn a_construct_without_a_parameter_list_is_not_a_function() {
    let extraction = extract_file(
        "ops.h",
        concat!(
            "namespace at {\n",
            "namespace _ops {\n",
            "struct TORCH_API cumulative_trapezoid_x {\n",
            "  static constexpr const char* name = \"aten\";\n",
            "};\n",
            "}}\n",
            "int realFunction(int a);\n",
        ),
    );
    let names = symbol_names(&extraction);
    for misparsed in ["ops.h::at", "ops.h::_ops"] {
        assert!(
            !names.contains(&misparsed),
            "a namespace is not a function definition: {names:?}"
        );
    }
    // And the invariant that made this matter: one name, one node.
    let mut seen = HashSet::new();
    for symbol in &extraction.symbols {
        assert!(
            seen.insert(symbol.qualified_name.as_str()),
            "duplicate identity {:?}",
            symbol.qualified_name
        );
    }

    // The rule must not swallow real definitions, in either grammar.
    let real = extract_file(
        "real.cpp",
        "int withParams(int a) { return a; }\nint noArgs() { return 1; }\n",
    );
    for expected in ["real.cpp::withParams", "real.cpp::noArgs"] {
        assert!(
            symbol_names(&real).contains(&expected),
            "a real definition declares parameters, even an empty list: {:?}",
            symbol_names(&real)
        );
    }
}

/// A C++ constructor keeps its symbol even though it has no return type.
///
/// The macro rule above keys on the missing return type, and constructors,
/// destructors and conversion operators are the definitions that legitimately
/// lack one. This is the guard against that rule over-reaching.
#[test]
fn a_constructor_is_not_mistaken_for_a_macro() {
    let extraction = extract_file(
        "ctor.cpp",
        concat!(
            "struct S {\n",
            "  S() { init(); }\n",
            "  ~S() { teardown(); }\n",
            "  void init();\n",
            "  void teardown();\n",
            "};\n",
            "struct T { T(); };\n",
            "T::T() { setup(); }\n",
        ),
    );
    let names = symbol_names(&extraction);
    assert!(
        names.contains(&"ctor.cpp::S.S"),
        "an in-class constructor is a definition: {names:?}"
    );
    assert!(
        names.contains(&"ctor.cpp::T.T"),
        "an out-of-line constructor is a definition: {names:?}"
    );
}

/// Objective-C identity is the whole selector, on both sides of the call.
///
/// `-a:b:` and `-a:c:` are different methods, so the first selector part is not
/// an identity. The definition and the message expression are built by matching
/// rules precisely so the two join.
#[test]
fn objective_c_methods_are_identified_by_their_full_selector() {
    let extraction = extract_file(
        "sel.m",
        concat!(
            "@implementation Foo\n",
            "- (int)a:(int)x b:(int)y { return x; }\n",
            "- (int)run { return [self a:1 b:2]; }\n",
            "@end\n",
        ),
    );
    let names = symbol_names(&extraction);
    assert!(
        names.contains(&"sel.m::Foo.a:b:"),
        "a keyword method is named by its whole selector: {names:?}"
    );
    assert!(
        names.contains(&"sel.m::Foo.run"),
        "a zero-argument selector carries no colon: {names:?}"
    );
    assert!(
        callees(&extraction).contains(&"a:b:"),
        "the message must name the same selector the definition carries: {:?}",
        callees(&extraction)
    );
    // `@interface Foo` and `@implementation Foo` are two halves of one class and
    // must not become two nodes carrying one name.
    let both = extract_file(
        "pair.m",
        "@interface Foo : NSObject\n- (int)run;\n@end\n@implementation Foo\n- (int)run { return 1; }\n@end\n",
    );
    assert_eq!(
        both.symbols
            .iter()
            .filter(|symbol| symbol.name == "Foo")
            .count(),
        1,
        "an interface and its implementation are one class: {:?}",
        symbol_names(&both)
    );
}

/// Entry points the runtime reaches without a call site stay live.
///
/// The dangerous direction of the visibility change is reporting reachable code
/// as dead. Each of these is invoked by name by something outside the corpus:
/// the C runtime, the CUDA host API, the Metal host API, the Objective-C
/// runtime, and libFuzzer's driver.
#[test]
fn runtime_entry_points_are_annotated() {
    for (path, source, symbol, expected) in [
        (
            "app.c",
            "int main(void){ return 0; }\n",
            "app.c::main",
            "Program entry point",
        ),
        (
            "k.cu",
            "__global__ void kern(int* o){ }\n",
            "k.cu::kern",
            "CUDA kernel launched by name from host code",
        ),
        (
            "fuzz.c",
            "int LLVMFuzzerTestOneInput(const char* d, long s){ return 0; }\n",
            "fuzz.c::LLVMFuzzerTestOneInput",
            "libFuzzer harness entry point",
        ),
        (
            "obj.m",
            "@implementation Foo\n- (int)run { return 1; }\n@end\n",
            "obj.m::Foo.run",
            "Objective-C method reachable by runtime selector dispatch",
        ),
    ] {
        let extraction = extract_file(path, source);
        let annotation = extraction
            .wiring
            .iter()
            .find(|wiring| wiring.target_symbol == symbol)
            .unwrap_or_else(|| {
                panic!(
                    "{path}: {symbol} must carry an entry-point annotation; got {:?}",
                    extraction.wiring
                )
            });
        assert!(matches!(annotation.kind, WiringKind::RuntimeEntryPoint));
        assert_eq!(annotation.details, expected);
    }

    // A Metal shader qualifier keeps its own, more specific reason.
    let metal = extract_file("s.metal", "kernel void reduce(){ }\n");
    assert!(
        metal
            .wiring
            .iter()
            .any(|wiring| wiring.target_symbol == "s.metal::reduce"
                && matches!(wiring.kind, WiringKind::RuntimeEntryPoint)),
        "a Metal shader entry point must stay live: {:?}",
        metal.wiring
    );

    // An ordinary helper next to an entry point must NOT be annotated, or the
    // exemption would be worthless.
    let plain = extract_file("plain.c", "int helper(int a){ return a; }\n");
    assert!(
        plain.wiring.is_empty(),
        "an ordinary function is not an entry point: {:?}",
        plain.wiring
    );
    let device_only = extract_file("d.cu", "__device__ int helper(int a){ return a; }\n");
    assert!(
        device_only.wiring.is_empty(),
        "`__device__` is callable only from the GPU side, not launched by the host: {:?}",
        device_only.wiring
    );
}

/// A prototype declares; it does not define.
///
/// Emitting a symbol for both a header prototype and its definition would put
/// two nodes carrying one name into the graph, and an ambiguous resolution
/// downgrades every call to that name to the speculative tier.
#[test]
fn a_prototype_does_not_become_a_second_symbol() {
    let header = extract_file("api.h", "int publicApi(int);\nstruct Api { int x; };\n");
    let names = symbol_names(&header);
    assert!(
        !names.contains(&"api.h::publicApi"),
        "a prototype is a declaration, not a definition: {names:?}"
    );
    assert!(
        names.contains(&"api.h::Api"),
        "a type definition in a header is still a symbol: {names:?}"
    );
}

/// A function-like macro is a callable; an object-like macro is a constant.
///
/// SC31 made this observable rather than creating it. Once C-family calls are
/// extracted, `ACTIONS(1)` is recorded as a call — and with no symbol behind
/// the `#define`, the graph held a call whose target it had never emitted.
/// Measured on this repository: **13,630** rows entered the defect tier from
/// that asymmetry alone, 7,372 of them `ACTIONS` inside a single generated
/// `parser.c` dispatch table, drowning the signal SC18/SC30 built the tier to
/// carry.
///
/// Both directions are pinned. Emitting *every* `#define` would add a node per
/// object-like constant for no resolution benefit, so `preproc_def` must stay
/// out — and a test that only asserted the positive would pass just as well
/// against a rule that emitted both.
#[test]
fn a_function_like_macro_is_a_symbol_and_an_object_like_macro_is_not() {
    let extraction = extract_file(
        "m.c",
        "#define ACTIONS(id) (id)\n#define MAXLEN 10\n\
         static int table[] = { ACTIONS(1), ACTIONS(2) };\n\
         int use(void) { return table[0] + MAXLEN; }\n",
    );
    let names = symbol_names(&extraction);
    assert!(
        names.contains(&"m.c::ACTIONS"),
        "a function-like macro is the target of a recorded call, so it must be a symbol: {names:?}"
    );
    assert!(
        !names.contains(&"m.c::MAXLEN"),
        "an object-like macro is a constant, never a callee: {names:?}"
    );
    assert!(
        callees(&extraction).contains(&"ACTIONS"),
        "the call this symbol exists to join must still be recorded: {:?}",
        callees(&extraction)
    );
}
