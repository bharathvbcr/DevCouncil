//! Dead-code analysis must be correct for the C family, in both directions.
//!
//! Before this, no C-family symbol could ever be reported dead. Two independent
//! causes: `generic_is_exported` falls back to `!name.starts_with('_')`, which
//! made 1,393 of 1,464 C-family symbols on a real 183-file corpus "exported"
//! and therefore exempt; and the family extracted no calls at all, so nothing
//! could be shown reachable either. Measured on that corpus, `dev map dead`
//! returned 1,094 C-family rows, **every one of them at confidence 0.3
//! "Exported or exempt"** — a result carrying no information.
//!
//! The dangerous direction of fixing that is reporting reachable code as dead,
//! so the entry-point and header cases are asserted here alongside the finding.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    analyze_liveness(&extractions, &resolution)
}

fn report_for<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|report| report.symbol_name == symbol)
}

/// Every "must not be reported" test carries a control that *must* be reported.
///
/// Without one such a test passes for the wrong reason: before this work the
/// analysis reported no C-family symbol at all, so every "is not a confident
/// finding" assertion held vacuously. The control makes the test prove that the
/// analysis was actually running and still discriminating.
fn assert_control_is_reported(reports: &[DeadSymbolReport], control: &str) {
    let found = report_for(reports, control).unwrap_or_else(|| {
        panic!("control {control} must be reported dead, or this test proves nothing: {reports:?}")
    });
    assert!(
        !found.is_exempt && found.confidence > 0.8,
        "control {control} must be a confident finding: {found:?}"
    );
}

/// The defect, end to end: an unused C++ function is a finding, a used one is
/// not, and `main` is exempt rather than absent.
///
/// This is the minimal reproduction from the defect report. Pre-fix, `dead`
/// listed the Python symbols and never `unusedHelperCpp`, although both are
/// equally unused — the C++ half of the file was invisible to the analysis.
#[test]
fn an_unused_c_family_function_is_reported_and_a_used_one_is_not() {
    let reports = reports(&[(
        "main.cpp",
        concat!(
            "int unusedHelperCpp(int a) { return a + 1; }\n",
            "int usedHelper(int a) { return a + 2; }\n",
            "int main() { return usedHelper(1); }\n",
        ),
    )]);

    let unused = report_for(&reports, "unusedHelperCpp")
        .expect("an uncalled C++ function must be a dead-code candidate");
    assert!(
        !unused.is_exempt && unused.confidence > 0.8,
        "an uncalled, unexported C++ function is a confident finding: {unused:?}"
    );

    assert!(
        report_for(&reports, "usedHelper").is_none_or(|report| report.is_exempt),
        "a function with a real caller must never be a confident finding: {:?}",
        report_for(&reports, "usedHelper")
    );

    let main = report_for(&reports, "main").expect("main should be visible as exempt evidence");
    assert!(main.is_exempt, "the program entry point is not dead");
    assert_eq!(
        main.exemption_reason.as_deref(),
        Some("Program entry point")
    );
}

/// The same conclusion must hold for every language in the family, not only for
/// the one in the reproduction.
#[test]
fn every_c_family_language_can_report_dead_code() {
    for (path, source, dead, live) in [
        (
            "unit.c",
            "static int unusedC(int a){return a;}\nint usedC(int a){return a;}\nint main(void){ return usedC(1); }\n",
            "unusedC",
            "usedC",
        ),
        (
            "unit.cu",
            "__device__ int unusedCu(int a){return a;}\n__device__ int usedCu(int a){return a;}\n__global__ void kern(){ usedCu(1); }\n",
            "unusedCu",
            "usedCu",
        ),
        (
            "unit.metal",
            "float unusedM(float a){return a;}\nfloat usedM(float a){return a;}\nkernel void reduce(){ usedM(1.0); }\n",
            "unusedM",
            "usedM",
        ),
    ] {
        let reports = reports(&[(path, source)]);
        let found = report_for(&reports, dead)
            .unwrap_or_else(|| panic!("{path}: {dead} must be a candidate; got {reports:?}"));
        assert!(
            !found.is_exempt && found.confidence > 0.8,
            "{path}: {dead} is uncalled and unexported: {found:?}"
        );
        assert!(
            report_for(&reports, live).is_none_or(|report| report.is_exempt),
            "{path}: {live} has a caller and must not be a confident finding"
        );
    }
}

/// A GPU entry point is dispatched by the host by name and must stay live.
///
/// This is the direction that breaks builds if it is wrong: a Metal shader or a
/// CUDA kernel has no call site anywhere in the corpus, so the visibility change
/// would make every one of them a confident deletion candidate without these
/// annotations.
#[test]
fn gpu_and_runtime_entry_points_never_become_confident_findings() {
    for (path, source, symbol) in [
        (
            "s.metal",
            "float controlDead(float a){ return a; }\nkernel void reduce(){ }\n",
            "reduce",
        ),
        (
            "v.metal",
            "float controlDead(float a){ return a; }\nvertex float4 vmain(){ return 0; }\n",
            "vmain",
        ),
        (
            "k.cu",
            "__device__ int controlDead(int a){ return a; }\n__global__ void kern(int* o){ }\n",
            "kern",
        ),
        (
            "fuzz.c",
            "static int controlDead(int a){ return a; }\nint LLVMFuzzerTestOneInput(const char* d, long s){ return 0; }\n",
            "LLVMFuzzerTestOneInput",
        ),
        (
            "obj.m",
            "static int controlDead(int a){ return a; }\n@implementation Foo\n- (int)neverCalledDirectly { return 1; }\n@end\n",
            "Foo.neverCalledDirectly",
        ),
    ] {
        let reports = reports(&[(path, source)]);
        let found = report_for(&reports, symbol);
        assert!(
            found.is_none_or(|report| report.is_exempt),
            "{path}: {symbol} is reached by a runtime this extractor cannot observe \
             and must never be a confident finding: {found:?}"
        );
        assert_control_is_reported(&reports, "controlDead");
    }
}

/// A declaration in a header is public API and is never a confident finding.
///
/// The header is the declaration surface of a C-family unit, so this is the
/// mechanism that keeps a header-heavy repository from filling with false
/// positives when implementation-file definitions stop counting as exports.
#[test]
fn header_declarations_are_public_api() {
    let reports = reports(&[
        (
            "api.h",
            "struct PublicType { int x; };\nstatic inline int inlineApi(int a){ return a; }\n",
        ),
        ("api.c", "static int controlDead(int a){ return a; }\n"),
    ]);
    assert_control_is_reported(&reports, "controlDead");
    for symbol in ["PublicType", "inlineApi"] {
        assert!(
            report_for(&reports, symbol).is_none_or(|report| report.is_exempt),
            "{symbol} is declared in a header and is public API: {:?}",
            report_for(&reports, symbol)
        );
    }
}

/// A cross-file C call keeps its callee live.
///
/// The whole point of extracting calls is that reachability now crosses files.
/// Without this, the visibility change alone would report every non-`main`
/// function in a multi-file C program as dead.
#[test]
fn a_cross_file_c_call_keeps_its_callee_live() {
    let reports = reports(&[
        (
            "lib.c",
            "int libHelper(int a){ return a + 1; }\nstatic int controlDead(int a){ return a; }\n",
        ),
        (
            "main.c",
            "int libHelper(int);\nint main(void){ return libHelper(1); }\n",
        ),
    ]);
    assert_control_is_reported(&reports, "controlDead");
    assert!(
        report_for(&reports, "libHelper").is_none_or(|report| report.is_exempt),
        "a function called from another translation unit is live: {:?}",
        report_for(&reports, "libHelper")
    );
}

/// An out-of-line C++ member definition is reached by a call on its type.
///
/// The definition and the in-class declaration must be one symbol, or the
/// definition is unreachable and becomes a confident deletion candidate for code
/// that is plainly called.
#[test]
fn an_out_of_line_member_is_reached_by_a_method_call() {
    let reports = reports(&[(
        "member.cpp",
        concat!(
            "struct S { int compute(int a); };\n",
            "int S::compute(int a) { return a; }\n",
            "int driver(S s) { return s.compute(1); }\n",
            "static int controlDead(int a){ return a; }\n",
        ),
    )]);
    assert_control_is_reported(&reports, "controlDead");
    assert!(
        report_for(&reports, "S.compute").is_none_or(|report| report.is_exempt),
        "an out-of-line member with a call site is live: {:?}",
        report_for(&reports, "S.compute")
    );
}

/// The extractor and the analyzer must recognise exactly the same set of
/// C-family header extensions.
///
/// The two tables are separate because `devmap-analyze` cannot reach into
/// `devmap-extract`'s private helpers, and a silent disagreement is the kind
/// SC15 warned about: a file the extractor calls a header would publish its
/// prototypes as exports while the analyzer still treated its own symbols as
/// private, so the join would half fire with no test noticing.
///
/// Asserted behaviourally, through the only two observable effects: a
/// declaration in a header is never a confident finding (the extractor's
/// `is_exported`), and a prototype in a header exempts the matching definition
/// in an implementation file (the analyzer's join).
#[test]
fn the_two_header_tables_agree_extension_by_extension() {
    for extension in [".h", ".hh", ".hpp", ".hxx", ".cuh"] {
        let header = format!("api{extension}");
        let reports = reports(&[
            (header.as_str(), "int publishedApi(int);\n"),
            (
                "impl.c",
                "int publishedApi(int a){ return a; }\nstatic int controlDead(int a){ return a; }\n",
            ),
        ]);
        assert_control_is_reported(&reports, "controlDead");
        assert!(
            report_for(&reports, "publishedApi").is_none_or(|report| report.is_exempt),
            "{extension}: a prototype in a header must exempt its definition, \
             so both tables must call {extension} a header: {:?}",
            report_for(&reports, "publishedApi")
        );
    }

    // An implementation extension must NOT behave like a header in either half,
    // or the join would exempt everything and the analysis would be worthless.
    for extension in [".c", ".cpp", ".cc", ".m", ".mm", ".cu", ".metal"] {
        let other = format!("other{extension}");
        let reports = reports(&[
            (other.as_str(), "int notApi(int);\n"),
            ("impl2.c", "int notApi(int a){ return a; }\n"),
        ]);
        assert!(
            report_for(&reports, "notApi").is_some_and(|report| !report.is_exempt),
            "{extension}: a prototype in an implementation file publishes nothing: {:?}",
            report_for(&reports, "notApi")
        );
    }
}
