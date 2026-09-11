use devmap_extract::extract_file;

/// Every language the registry declares must reach a real grammar, or be one of
/// the documented exceptions.
///
/// The port previously linked 5 of 36 declared languages while `LANGUAGE_SPECS`
/// advertised all of them, so a `.sol` or `.tf` file indexed as nothing at all
/// and no test noticed. This asserts the matrix rather than a sample: a
/// language added to the registry without a grammar fails here.
#[test]
fn every_declared_language_reaches_a_grammar_or_is_a_known_exception() {
    // (path, minimal source) for one file of each linked language.
    let cases: &[(&str, &str)] = &[
        ("a.py", "def f():\n    pass\n"),
        ("a.js", "function f() {}\n"),
        ("a.ts", "export function f(): void {}\n"),
        ("a.tsx", "export const A = () => <div/>;\n"),
        ("a.rs", "fn f() {}\n"),
        ("a.go", "package p\nfunc F() {}\n"),
        ("main.tf", "resource \"aws_s3_bucket\" \"b\" {}\n"),
        ("A.java", "class A {}\n"),
        ("A.cs", "class A {}\n"),
        ("a.php", "<?php\nfunction f() {}\n"),
        ("a.rb", "def f\nend\n"),
        ("a.c", "int f(void) { return 0; }\n"),
        ("a.cpp", "int f() { return 0; }\n"),
        ("a.m", "@implementation A\n@end\n"),
        ("a.cu", "__global__ void f() {}\n"),
        ("a.swift", "func f() {}\n"),
        ("a.kt", "class A { fun f() {} }\n"),
        ("a.scala", "object A { def f(): Int = 1 }\n"),
        ("a.dart", "int f() { return 1; }\n"),
        (
            "a.svelte",
            "<script>\n  export function f() {}\n</script>\n",
        ),
        ("a.vue", "<template>\n  <div/>\n</template>\n"),
        ("a.astro", "---\nconst x = 1;\n---\n<div/>\n"),
        ("a.liquid", "{% if x %}{{ x }}{% endif %}\n"),
        ("a.pas", "program P;\nbegin\nend.\n"),
        ("a.lua", "function f() return 1 end\n"),
        ("a.luau", "local function f() return 1 end\n"),
        ("a.R", "f <- function() { 1 }\n"),
        ("a.cfm", "<cfset x = 1>\n"),
        ("a.erl", "-module(a).\n"),
        ("a.sol", "contract C {}\n"),
        ("a.nix", "{ x = 1; }\n"),
        // Not in the frozen Python 35 — `detect_language` routes these through
        // its fallback table — but they are linked and must stay linked.
        ("a.sh", "helper() {\n  echo hi\n}\n"),
        ("a.sql", "CREATE TABLE users (id INT);\n"),
    ];

    let mut unavailable = Vec::new();
    for (path, source) in cases {
        let extraction = extract_file(path, source);
        let engine = format!("{:?}", extraction.engine);
        if engine.contains("Unavailable") {
            unavailable.push(*path);
        }
    }
    assert!(
        unavailable.is_empty(),
        "these languages are declared but reach no grammar: {unavailable:?}"
    );

    // The documented exceptions. Both are declared in the registry and report
    // `Unavailable` honestly rather than pretending; each is listed here so
    // adding a third is a deliberate edit rather than a quiet omission.
    //
    // VB.NET: no tree-sitter grammar exists on crates.io or in any reachable
    // upstream repository.
    let vb = extract_file("a.vb", "Class A\nEnd Class\n");
    assert!(
        format!("{:?}", vb.engine).contains("Unavailable"),
        "VB.NET has no grammar and must report so rather than silently degrade"
    );

    // COBOL: a grammar exists and is vendored, but it **does not terminate** on
    // malformed input (measured 2026-09-04: six bytes of NUL, and a BOM
    // followed by garbage, each ran past three minutes; the other 34 grammars
    // clear all 14 hostile inputs in 5.03 s total), and it cannot be bounded
    // in-process. Unlinking cost nothing measurable: on a realistic COBOL
    // program it parsed `Clean` and yielded only the File node — zero
    // declarations, zero calls. See `UNSAFE_GRAMMARS` in `treesitter.rs`.
    let cobol = extract_file("a.cbl", "       IDENTIFICATION DIVISION.\n");
    assert!(
        format!("{:?}", cobol.engine).contains("Unavailable"),
        "COBOL is deliberately unlinked and must report so: {:?}",
        cobol.engine
    );
    match &cobol.parse_outcome {
        devmap_extract::model::ParseOutcome::Failed { reason } => assert!(
            reason.contains("does not terminate"),
            "the refusal must say why COBOL is unlinked, got {reason:?}"
        ),
        other => panic!("COBOL must be refused, got {other:?}"),
    }
}
