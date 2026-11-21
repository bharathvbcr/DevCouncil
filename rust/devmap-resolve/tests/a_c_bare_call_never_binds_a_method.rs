//! C has no methods and no implicit `this`, so a bare call in a `.c` file
//! never names a method of C++, Objective-C or CUDA, nor a Java or C# method.
//! The bare-name rung used to bind it anyway whenever the method's spelling was
//! the one namesake in the repository (S2, criterion 10): `CStyle` is one family
//! for c, cpp, objc and cuda, and a C++ method is a `SymbolKind::Method`
//! candidate of that family.
//!
//! A C call to a C++ or CUDA *free function* (`extern "C"`) is legitimate and
//! keeps binding; a C++ method calling a sibling bare (implicit `this`) keeps
//! binding too.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, ExtractedCall, Extraction, Span};
use devmap_resolve::model::LimitPolicy;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::{ResolutionLimits, Resolver};

fn resolve(all: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = all
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn call_targets(result: &ResolutionResult, from_file: &str) -> Vec<(String, String)> {
    result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_file == from_file)
        .map(|e| (e.source_symbol.clone(), e.target_symbol.clone()))
        .collect()
}

fn no_call_edge_out_of(result: &ResolutionResult, from_file: &str) {
    let out = call_targets(result, from_file);
    assert!(out.is_empty(), "{from_file} must bind nothing: {out:?}");
}

fn calls(result: &ResolutionResult, from_file: &str, to_file: &str) -> bool {
    result.edges.iter().any(|e| {
        e.edge_kind == EdgeKind::Calls && e.source_file == from_file && e.target_file == to_file
    })
}

const C_CALLER: &str = "int caller(void) { return NAME(); }\n";

fn c_caller(name: &str) -> String {
    C_CALLER.replace("NAME", name)
}

#[test]
fn a_c_call_does_not_bind_a_java_method_of_the_same_name() {
    let result = resolve(&[
        ("c/main.c", &c_caller("foo")),
        ("java/J.java", "class J { int foo() { return 1; } }\n"),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

#[test]
fn a_c_call_does_not_bind_a_cpp_method_of_the_same_name() {
    let result = resolve(&[
        ("c/main.c", &c_caller("bar")),
        (
            "cpp/w.cpp",
            "class Widget { public: int bar() { return 1; } };\n",
        ),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

#[test]
fn a_c_call_does_not_bind_an_out_of_line_cpp_method() {
    let result = resolve(&[
        ("c/main.c", &c_caller("bar")),
        (
            "cpp/w.cpp",
            "class Widget { public: int bar(); };\nint Widget::bar() { return 1; }\n",
        ),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

#[test]
fn a_c_call_does_not_bind_an_objc_method_of_the_same_name() {
    let result = resolve(&[
        ("c/main.c", &c_caller("baz")),
        (
            "objc/T.m",
            "@implementation Thing\n- (int)baz { return 1; }\n@end\n",
        ),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

#[test]
fn a_c_call_does_not_bind_a_cuda_method_of_the_same_name() {
    let result = resolve(&[
        ("c/main.c", &c_caller("run")),
        (
            "cu/k.cu",
            "struct Kernel { __host__ int run() { return 1; } };\n",
        ),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

// ---- controls: what must keep binding -----------------------------------------

#[test]
fn a_c_call_still_binds_a_cpp_or_cuda_free_function() {
    let result = resolve(&[
        (
            "c/main.c",
            "int a(void) { return cpp_free(); }\nint b(void) { return cu_free(); }\n",
        ),
        ("cpp/f.cpp", "extern \"C\" int cpp_free() { return 1; }\n"),
        ("cu/f.cu", "int cu_free() { return 2; }\n"),
    ]);
    assert!(
        calls(&result, "c/main.c", "cpp/f.cpp"),
        "C -> extern C++ fn"
    );
    assert!(calls(&result, "c/main.c", "cu/f.cu"), "C -> CUDA fn");
}

#[test]
fn a_c_call_still_binds_a_c_function() {
    let result = resolve(&[
        ("c/main.c", &c_caller("helper")),
        ("c/lib.c", "int helper(void) { return 1; }\n"),
    ]);
    assert!(calls(&result, "c/main.c", "c/lib.c"));
}

#[test]
fn a_cpp_method_still_binds_a_sibling_method_by_implicit_this() {
    let result = resolve(&[(
        "cpp/w.cpp",
        "class W { public: int a() { return b(); } int b() { return 2; } };\n",
    )]);
    assert!(
        call_targets(&result, "cpp/w.cpp")
            .iter()
            .any(|(from, to)| from.ends_with("W.a") && to.ends_with("W.b")),
        "implicit-this sibling call: {:?}",
        call_targets(&result, "cpp/w.cpp")
    );
}

// ---- the gate fires only for provably plain C source (a lowercase `.c`) ------
//
// Extraction reads `.h` as C (`languages.rs`, spec "C"), and `detect_language`
// lowercases the extension, so a `.C` file (conventionally C++) is "c" too.
// A header can be C++ and a `.C` file is C++, so neither proves the caller has
// no implicit `this`. The residual (C++ headers read as C) is carded to
// ft-915ee323 (extractor output, owner-gated); until then these callers bind
// exactly as before the gate.

#[test]
fn a_h_caller_binds_a_method_namesake_as_before_the_gate() {
    let result = resolve(&[
        ("h/main.h", &c_caller("bar")),
        (
            "cpp/w.cpp",
            "class Widget { public: int bar() { return 1; } };\n",
        ),
    ]);
    assert!(
        calls(&result, "h/main.h", "cpp/w.cpp"),
        "a .h caller may be C++ (ft-915ee323): {:?}",
        call_targets(&result, "h/main.h")
    );
}

#[test]
fn a_capital_c_caller_binds_a_method_namesake_as_before_the_gate() {
    let result = resolve(&[
        ("cpp/main.C", &c_caller("bar")),
        (
            "cpp/w.cpp",
            "class Widget { public: int bar() { return 1; } };\n",
        ),
    ]);
    assert!(
        calls(&result, "cpp/main.C", "cpp/w.cpp"),
        ".C is C++ by convention: {:?}",
        call_targets(&result, "cpp/main.C")
    );
}

/// One method and one free function share the name: two candidates, so the
/// call is ambiguous and binds nothing, exactly as before the gate. The gate
/// only removes an edge; it never filters a candidate set to make a different
/// target win (that would add an edge the base never had).
#[test]
fn a_c_call_with_a_method_and_a_free_function_namesake_stays_unbound() {
    let result = resolve(&[
        ("c/main.c", &c_caller("dup")),
        (
            "cpp/w.cpp",
            "class Widget { public: int dup() { return 1; } };\n",
        ),
        ("c/lib.c", "int dup(void) { return 2; }\n"),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

// ---- the gate adds no cache-key bit and no budget charge ----------------------

/// `leaf` has 40 free-function declarations and no method namesake, called
/// from a `.c` file and from a `.h` file. The gate cannot change that set, so
/// the two callers share one cache entry and the 40 visits are charged once:
/// a limit of 40 admits it, and charging per caller kind would need 80.
#[test]
fn a_name_called_from_c_and_h_with_no_method_namesake_is_charged_once() {
    let mut inputs: Vec<Extraction> = (0..40)
        .map(|i| extract_file(&format!("d/p{i}.c"), "int leaf(void) { return 0; }\n"))
        .collect();
    for file in ["d/caller_a.c", "d/caller_b.h"] {
        let mut caller = extract_file(file, "int run(void) { return 0; }\n");
        caller.calls = vec![ExtractedCall {
            caller_symbol: Some(format!("{file}::run")),
            callee_name: "leaf".into(),
            receiver_expr: None,
            span: Span {
                start_byte: 0,
                end_byte: 1,
            },
        }];
        inputs.push(caller);
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let limits = |candidate_visits| ResolutionLimits {
        candidate_visits,
        on_exceed: LimitPolicy::Refuse,
        ..Default::default()
    };
    let refused = resolver
        .resolve_all_with_limits(&inputs, limits(39))
        .unwrap_err();
    assert_eq!(refused.resource, "candidate visits", "the limit is live");
    resolver
        .resolve_all_with_limits(&inputs, limits(40))
        .expect("one shared candidate set is charged once, 40 visits");
}

fn caller_extraction(file: &str, callee: &str) -> Extraction {
    let mut caller = extract_file(file, "int run(void) { return 0; }\n");
    caller.calls = vec![ExtractedCall {
        caller_symbol: Some(format!("{file}::run")),
        callee_name: callee.into(),
        receiver_expr: None,
        span: Span {
            start_byte: 0,
            end_byte: 1,
        },
    }];
    caller
}

fn visits_within(inputs: &[Extraction], limit: u64) -> Result<(), String> {
    let mut resolver = Resolver::new();
    resolver.index_extractions(inputs);
    resolver
        .resolve_all_with_limits(
            inputs,
            ResolutionLimits {
                candidate_visits: limit,
                on_exceed: LimitPolicy::Refuse,
                ..Default::default()
            },
        )
        .map(|_| ())
        .map_err(|error| error.resource.to_string())
}

/// P3b. `bar` has a method namesake (one visit) and is called from a `.h` and
/// a `.c` file. One cache entry serves both, so one visit is charged; a gate
/// that split the key on the caller kind would need two.
#[test]
fn a_name_with_a_method_namesake_called_from_h_and_c_is_charged_once() {
    let inputs = [
        extract_file(
            "cpp/w.cpp",
            "class Widget { public: int bar() { return 1; } };\n",
        ),
        caller_extraction("d/caller_a.c", "bar"),
        caller_extraction("d/caller_b.h", "bar"),
    ];
    visits_within(&inputs, 1).expect("one shared set, one visit");
    assert_eq!(visits_within(&inputs, 0), Err("candidate visits".into()));
}

/// P3b with a mixed set (method plus free function, two visits): limit 2 is
/// what base needs and must still be enough.
#[test]
fn a_mixed_set_called_from_h_and_c_is_charged_once() {
    let inputs = [
        extract_file(
            "cpp/w.cpp",
            "class Widget { public: int bar() { return 1; } };\n",
        ),
        extract_file("lib/free.c", "int bar(void) { return 2; }\n"),
        caller_extraction("d/caller_a.c", "bar"),
        caller_extraction("d/caller_b.h", "bar"),
    ];
    visits_within(&inputs, 2).expect("two visits, charged once for both callers");
    assert_eq!(visits_within(&inputs, 1), Err("candidate visits".into()));
}

/// The gate adds no cache-key bit, so the answer for each caller does not
/// depend on which caller reached the shared cache entry first.
#[test]
fn the_gate_does_not_depend_on_which_caller_fills_the_cache_first() {
    let method = (
        "cpp/w.cpp",
        "class Widget { public: int bar() { return 1; } };\n",
    );
    let c = ("d/caller_a.c", "int run(void) { return bar(); }\n");
    let h = ("d/caller_b.h", "int run(void) { return bar(); }\n");
    for order in [
        [method, c, h],
        [method, h, c],
        [h, c, method],
        [c, h, method],
    ] {
        let result = resolve(&order);
        no_call_edge_out_of(&result, "d/caller_a.c");
        assert!(
            calls(&result, "d/caller_b.h", "cpp/w.cpp"),
            "the .h caller keeps its base binding in order {order:?}"
        );
    }
}

/// P5. With a `static` function of another translation unit and a C++ method
/// of the same name there are two namesakes: ambiguous at base, so unbound.
#[test]
fn a_static_in_another_c_file_beside_a_method_namesake_stays_unbound() {
    let result = resolve(&[
        ("c/main.c", &c_caller("helper")),
        ("c/lib.c", "static int helper(void) { return 2; }\n"),
        (
            "cpp/w.cpp",
            "class W { public: int helper() { return 1; } };\n",
        ),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

/// A C function written inside an `@implementation` block is a free function,
/// not a method of the class, so the plain-C gate must not decline it: the call
/// binds exactly as it did before the gate.
#[test]
fn a_c_call_still_binds_a_c_function_written_inside_an_objc_implementation() {
    let result = resolve(&[
        ("d/a.c", &c_caller("bar")),
        (
            "o/T.m",
            "@implementation T\nint bar(void){ return 1; }\n- (int)x{ return bar(); }\n@end\n",
        ),
    ]);
    assert!(
        calls(&result, "d/a.c", "o/T.m"),
        "the C function inside the block keeps its edge: {:?}",
        call_targets(&result, "d/a.c")
    );
}
