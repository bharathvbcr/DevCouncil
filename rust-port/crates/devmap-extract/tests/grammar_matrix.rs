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
        ("a.cbl", "IDENTIFICATION DIVISION.\n"),
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

    // The one documented exception: no tree-sitter VB.NET grammar exists on
    // crates.io or in any reachable upstream repository. It is declared in the
    // registry, and reports `Unavailable` honestly rather than pretending.
    let vb = extract_file("a.vb", "Class A\nEnd Class\n");
    assert!(
        format!("{:?}", vb.engine).contains("Unavailable"),
        "VB.NET has no grammar and must report so rather than silently degrade"
    );
}
