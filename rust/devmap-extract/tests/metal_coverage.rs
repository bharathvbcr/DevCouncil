//! Metal coverage under the C++ grammar (SC19).
//!
//! Metal has no tree-sitter grammar and is routed onto `tree-sitter-cpp` by its
//! `LanguageSpec`. The open question was never grammar availability but
//! *coverage*: how much of a real shader survives that substitution. These
//! tests pin the answer, in both directions — what Metal must recover, and what
//! must still be reported as a degraded parse.

use devmap_extract::{extract_file, Extraction, ParseOutcome, SymbolKind, WiringKind};

/// A realistic shader: compute kernels with `[[buffer(n)]]` and
/// `[[thread_position_in_grid]]`, vertex/fragment stages with `[[stage_in]]`
/// and `[[position]]`, `[[attribute(n)]]` struct members, address-space
/// qualified pointers, `constexpr constant` globals, templates and namespaces.
const REALISTIC: &str = include_str!("data/realistic.metal");

fn outcome(extraction: &Extraction) -> String {
    match &extraction.parse_outcome {
        ParseOutcome::Clean => "Clean".to_string(),
        ParseOutcome::Partial { error_ranges } => format!("Partial({})", error_ranges.len()),
        ParseOutcome::Failed { .. } => "Failed".to_string(),
        // Tier-2 pattern recovery. Metal is routed onto the C++ grammar, so it
        // should never reach this arm; naming it here rather than matching `_`
        // keeps that a checked expectation instead of an assumption.
        ParseOutcome::Fallback { .. } => "Fallback".to_string(),
        // Nor this one: the skip rule matches minified bundles by name and a
        // `.metal` file is not one. Named for the same reason as `Fallback`.
        ParseOutcome::Skipped { .. } => "Skipped".to_string(),
    }
}

fn declared(extraction: &Extraction) -> Vec<String> {
    extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| format!("{:?}:{}", symbol.kind, symbol.name))
        .collect()
}

/// Every declaration in a realistic shader is recovered, and the file is not
/// reported as degraded.
///
/// The declaration list is spelled out rather than counted: a count passes just
/// as well when the extractor swaps one symbol for another, and the historical
/// failure mode here was exactly that — the frozen Python baseline names this
/// language's one fixture function `kernel`, taking the qualifier for the name.
#[test]
fn a_realistic_metal_shader_is_fully_recovered_and_parses_clean() {
    let extraction = extract_file("shaders/realistic.metal", REALISTIC);

    assert_eq!(
        outcome(&extraction),
        "Clean",
        "Metal's own declaration qualifiers are the only thing tree-sitter-cpp \
         cannot place; a file containing nothing else must not be marked degraded"
    );
    assert_eq!(
        declared(&extraction),
        vec![
            "Struct:VertexIn",
            "Struct:VertexOut",
            "Struct:Uniforms",
            "Function:luminance",
            "Function:tonemap",
            "Function:lerpValue",
            "Function:safeDivide",
            "Function:residual_add",
            "Function:reduce_max",
            "Function:blur_texture",
            "Function:scene_vertex",
            "Function:scene_fragment",
            "Function:tagged_entry",
        ],
        "every struct, helper and shader entry point must be recovered under its \
         own name — never under the qualifier that precedes it"
    );
}

/// The recovered declaration set is the same one the equivalent C++ yields.
///
/// This is the load-bearing comparison for SC19: it establishes that routing
/// Metal onto the C++ grammar costs nothing at the declaration level, so any
/// remaining shortfall (no calls, no `#include` imports, a pointer return type
/// absorbed into the name) belongs to the C family as a whole and not to Metal.
#[test]
fn metal_recovers_exactly_what_the_equivalent_cpp_recovers() {
    for (metal_source, cpp_source) in [
        (
            "kernel void entry(device float *v [[buffer(0)]]) { v[0] = 1.0f; }\n",
            "void entry(float *v) { v[0] = 1.0f; }\n",
        ),
        (
            "struct S { float a [[attribute(0)]]; };\nfragment float4 shade(S in [[stage_in]]) { return float4(in.a); }\n",
            "struct S { float a; };\nfloat4 shade(S in) { return float4(in.a); }\n",
        ),
        (
            "constexpr constant float kA = 1.0f;\nfloat after() { return kA; }\n",
            "constexpr float kA = 1.0f;\nfloat after() { return kA; }\n",
        ),
    ] {
        let metal = extract_file("a.metal", metal_source);
        let cpp = extract_file("a.cpp", cpp_source);
        assert_eq!(
            declared(&metal),
            declared(&cpp),
            "Metal and the equivalent C++ must recover the same declarations\n\
             metal: {metal_source}\ncpp:   {cpp_source}"
        );
        assert_eq!(outcome(&metal), "Clean");
        assert_eq!(outcome(&cpp), "Clean");
    }
}

/// A shader entry point records the host dispatch that reaches it; a helper in
/// the same file records nothing.
///
/// The generic C-family extractor has no visibility keyword to read and so
/// marks every declaration exported. Without this annotation an entry point and
/// a private helper are indistinguishable in the dead-code output, both landing
/// under the same blanket "Exported or exempt" — an assumption where there
/// should be evidence.
#[test]
fn a_metal_shader_entry_point_is_annotated_and_a_helper_is_not() {
    let extraction = extract_file("shaders/realistic.metal", REALISTIC);
    let annotated: Vec<(&str, &str)> = extraction
        .wiring
        .iter()
        .filter(|annotation| annotation.kind == WiringKind::RuntimeEntryPoint)
        .map(|annotation| {
            (
                annotation.target_symbol.as_str(),
                annotation.details.as_str(),
            )
        })
        .collect();

    assert_eq!(
        annotated,
        vec![
            (
                "shaders/realistic.metal::residual_add",
                "compute shader dispatched by the host by name"
            ),
            (
                "shaders/realistic.metal::reduce_max",
                "compute shader dispatched by the host by name"
            ),
            (
                "shaders/realistic.metal::blur_texture",
                "compute shader dispatched by the host by name"
            ),
            (
                "shaders/realistic.metal::scene_vertex",
                "vertex shader bound to a render pipeline by the host"
            ),
            (
                "shaders/realistic.metal::scene_fragment",
                "fragment shader bound to a render pipeline by the host"
            ),
            // `[[kernel]] void tagged_entry(...)` — the attribute spelling of the
            // same qualifier, which must not be missed for being written the
            // other legal way.
            (
                "shaders/realistic.metal::tagged_entry",
                "compute shader dispatched by the host by name"
            ),
        ],
        "exactly the kernel/vertex/fragment entry points carry a runtime-entry \
         annotation; `luminance`, `tonemap`, `lerpValue` and `safeDivide` are \
         ordinary helpers and must carry none"
    );
}

/// The constructs found in real shaders that C++ has no production for at all.
///
/// These are the three shapes a 55-file corpus of real Metal still failed on
/// after the first, simplest form of the rule was in place, and each fails
/// differently from the plain `kernel void f` case: the address-space *cast*
/// makes the grammar reject the keyword itself rather than the token after it,
/// and an atomic in an address space makes it insert a MISSING `::` with no
/// text for a length check to judge. Handling one spelling of the atomic and
/// not the other would leave the same defect half-fixed.
#[test]
fn metal_constructs_with_no_cpp_equivalent_parse_clean() {
    for (label, source, expected) in [
        (
            "address-space cast",
            "kernel void c(device bfloat *O [[buffer(0)]]) {\n  device bfloat *Ob = (device bfloat *)O;\n}\n",
            vec!["Function:c"],
        ),
        (
            "atomic in an address space",
            "kernel void a(device atomic_float *d [[buffer(0)]]) { d[0] = 1.0f; }\n",
            vec!["Function:a"],
        ),
        (
            "templated atomic in an address space",
            "kernel void t(device atomic<float> *w [[buffer(4)]]) {}\n",
            vec!["Function:t"],
        ),
        (
            "unsigned atomic in an address space",
            "kernel void u(device atomic_uint *fail [[buffer(9)]]) {}\n",
            vec!["Function:u"],
        ),
    ] {
        let extraction = extract_file("a.metal", source);
        assert_eq!(outcome(&extraction), "Clean", "{label}");
        assert_eq!(declared(&extraction), expected, "{label}");
    }
}

/// The exemption is keyed to the language, not to the grammar.
///
/// Metal and C++ both answer `"cpp"` for their grammar, so a rule written
/// against the grammar key would silently declare a genuinely broken C++ file
/// healthy. In C++, `kernel void f()` really is a syntax error.
#[test]
fn the_metal_qualifier_exemption_does_not_leak_into_cpp() {
    let source = "kernel void f(device float *v [[buffer(0)]]) {}\n";
    assert_eq!(outcome(&extract_file("a.metal", source)), "Clean");
    assert_ne!(
        outcome(&extract_file("a.cpp", source)),
        "Clean",
        "a C++ file is not entitled to Metal's qualifiers; this must stay a \
         degraded parse"
    );
    assert_ne!(outcome(&extract_file("a.cc", source)), "Clean");
    assert_ne!(outcome(&extract_file("a.c", source)), "Clean");
}

/// A Metal file that is genuinely broken still reports a degraded parse.
///
/// The exemption exists to stop one known, measured, cost-free error class from
/// permanently masking every other one. If it swallowed real breakage it would
/// do the opposite of its purpose — the point of clearing the background is
/// that a signal against it means something.
#[test]
fn real_metal_breakage_still_reports_partial() {
    for (label, source) in [
        (
            "unterminated body",
            "kernel void f(device float *v [[buffer(0)]]) {\n  v[0] = 1.0f;\n",
        ),
        (
            "garbage between declarations",
            "kernel void f(device float *v [[buffer(0)]]) {}\n@@@ ### @@@\nvoid g() {}\n",
        ),
        (
            "malformed statement inside a kernel body",
            "kernel void f(device float *v [[buffer(0)]]) {\n  v[0] = * / 3;\n}\n",
        ),
        (
            "broken parameter list after a qualifier",
            "kernel void f(device float *v [[buffer(0)]], , ,) {}\n",
        ),
        (
            "stray closing brace",
            "kernel void f(device float *v [[buffer(0)]]) {}\n}\nvoid g() {}\n",
        ),
        // This one is what the single-token bound exists for: the ERROR node
        // begins immediately after a real Metal qualifier — satisfying the
        // adjacency test — but spans multiple lines of junk. Without the bound
        // this file reports Clean.
        (
            "multi-token junk directly after a qualifier",
            "kernel\n@@@ ###\n@@@ ###\nvoid g() {}\n",
        ),
    ] {
        assert_ne!(
            outcome(&extract_file("a.metal", source)),
            "Clean",
            "{label}: genuine breakage must survive the Metal qualifier exemption"
        );
    }
}

/// The exemption reaches one type token past the qualifier and no further.
///
/// `kernel void f` errors on the token immediately after the qualifier,
/// `constant float kA` and `device float *p` one token further along. Anything
/// beyond that is no longer attributable to the qualifier, and claiming it
/// would turn a bounded exemption into a file-wide amnesty for any shader whose
/// signature happens to mention an address space.
#[test]
fn the_metal_exemption_is_bounded_to_the_qualified_declaration() {
    // Directly adjacent, and one type token away: both benign.
    assert_eq!(
        outcome(&extract_file("a.metal", "kernel void f() {}\n")),
        "Clean"
    );
    assert_eq!(
        outcome(&extract_file(
            "a.metal",
            "constexpr constant float kA = 1.0f;\nfloat after() { return kA; }\n"
        )),
        "Clean"
    );
    // A qualifier earlier in the same declaration does not license an error in
    // the body that follows it.
    assert_ne!(
        outcome(&extract_file(
            "a.metal",
            "kernel void f(device float *v [[buffer(0)]]) {\n  int x = ) ( ;\n}\n"
        )),
        "Clean",
        "an error inside the body is not attributable to the signature's qualifiers"
    );
}

/// The one shipped fixture is recovered under the shader's own name.
///
/// Recorded because the frozen Python baseline for this fixture
/// (`testdata/golden/languages/metal/nodes.json`) names the symbol `kernel` —
/// it took the qualifier for the function. The Rust port names it `add`, which
/// is the correct answer and a deliberate divergence from that baseline.
#[test]
fn the_shipped_metal_fixture_is_named_for_its_function_not_its_qualifier() {
    let extraction = extract_file(
        "main.metal",
        include_str!("../../testdata/fixtures/languages/metal/main.metal"),
    );
    assert_eq!(outcome(&extraction), "Clean");
    assert_eq!(declared(&extraction), vec!["Function:add"]);
}
