//! Adversarial probes for the Kotlin follow-ups lane (`lane/v98-kotlin-followups`
//! @ 9836aa32, on main a0371296). Each test states the CORRECT behaviour; a
//! failing test is a BREAK.
//!
//! Contract: a file reported Clean carries no phantom locals or symbols; a
//! deadline or cap hit is never Clean; the repair keeps a re-parse only if it
//! is fully clean; output is deterministic; and the cross-language change in
//! `parse_outcome_of` (a node whose own error cost sits on no child is now
//! charged) must not flip a genuinely clean file in ANY language to Partial.
//!
//! The cross-language tests were found by running the lane's `extract_file`
//! over about 26,000 source files in the user's own trees (apps, devtools,
//! scholarlm, upstream mlx) against the same build with the one new
//! `parse_outcome_of` rule switched off, then re-checked on a build of main
//! a0371296 itself (the minimal snippets below and the five real files named
//! in the doc comments): every flip below is a file main reported Clean and the
//! lane reports Partial, and each snippet is valid in its language.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use devmap_extract::extract_file_with_budget;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

const BUDGET: Duration = Duration::from_secs(600);

fn ex(path: &str, source: &str) -> Extraction {
    extract_file_with_budget(path, source, BUDGET)
}

fn clean(e: &Extraction) -> bool {
    matches!(e.parse_outcome, ParseOutcome::Clean)
}

fn label(e: &Extraction, source: &str) -> String {
    match &e.parse_outcome {
        ParseOutcome::Clean => "Clean".to_string(),
        ParseOutcome::Partial { error_ranges, .. } => format!(
            "Partial[{}]",
            error_ranges
                .iter()
                .map(|r| format!(
                    "{}..{} `{}`",
                    r.start_byte,
                    r.end_byte,
                    source
                        .get(r.start_byte..r.end_byte)
                        .map(|s| s.chars().take(30).collect::<String>().replace('\n', "\\n"))
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        other => format!("{other:?}"),
    }
}

// ------------------------------------------------- cross-language false Partials

/// BREAK (HIGH, false Partial in Swift). `x as? String ?? "default"` is the
/// commonest way to read a `[String: Any]` or an `Info.plist` value, and it is a
/// valid Swift expression. tree-sitter-swift recovers it with a hidden
/// zero-width token: `has_error()` is true on the statement and on none of its
/// children, no ERROR or MISSING node exists, and main reports the file Clean.
/// The new rule in `parse_outcome_of` charges the statement, so every Swift file
/// that spells the idiom once becomes Partial. Measured: 7 of about 2,300 real
/// Swift files in the user's own trees (Vani, AppLock, Curio, DevType). The
/// source is valid, but unlike the C and Go cases the extraction is not
/// identical to the parenthesised spelling: it carries an empty-named call (see
/// `a_swift_nil_coalesced_string_cast_publishes_no_call_with_an_empty_name`).
#[test]
fn a_swift_file_with_a_nil_coalesced_string_cast_is_clean() {
    let mut failures = Vec::new();
    for source in [
        "let a = b as? String ?? \"x\"\n",
        "func f(r: [String: Any]) -> String {\n    return r[\"error\"] as? String ?? \"x\"\n}\n",
        "let v = Bundle.main.object(forInfoDictionaryKey: \"K\") as? String ?? \"1.0.0\"\n",
        "let a = x?.y as? String ?? \"d\"\n",
        "let a = b as? String ?? \"x\" + \"y\"\n",
        "let a = f(b as? String ?? \"\", 1)\n",
        "let a = [b as? String ?? \"\"]\n",
        "let a = b as? Character ?? \"x\"\n",
        "let a = b as? NSString ?? \"\"\n",
        "let a = b as? Any ?? \"x\"\n",
    ] {
        let e = ex("a.swift", source);
        if !clean(&e) {
            failures.push(format!("{source:?}: {}", label(&e, source)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (HIGH, false Partial in C, C++, Objective-C, CUDA, Metal). A file whose
/// last line is a preprocessor directive with no newline after it
/// (`#include "x.h"`, `#define X 1`, `#pragma once`) is valid C; the directive
/// ends at the end of the file. The grammar closes it with a hidden zero-width
/// newline token, no ERROR or MISSING node is shown, and main reports Clean. The
/// new rule charges the directive's range, so the file is Partial. Real file:
/// `upstream/mlx/mlx/backend/metal/kernels/steel/conv/loader.h`, whose last line
/// is `#include "...loader_channel_n.h"`.
#[test]
fn a_c_family_file_ending_in_a_directive_without_a_newline_is_clean() {
    let mut failures = Vec::new();
    for (path, source) in [
        ("a.c", "#include \"x.h\""),
        ("b.cpp", "#include <x.h>"),
        ("c.h", "#pragma once"),
        ("d.h", "#define X 1"),
        ("e.m", "#import <a.h>"),
        ("f.cu", "#include \"x.cuh\""),
        ("g.metal", "#include <metal_stdlib>"),
        ("h.c", "int f(void);\n#include \"a.h\"\n#include \"b.h\""),
        ("i.c", "#include <a.h>\r\n#include <b.h>"),
        ("j.hpp", "#pragma once\nnamespace a {}\n#include <b>"),
        ("k.c", "#define A /* c */"),
        ("l.c", "#undef A"),
    ] {
        let e = ex(path, source);
        if !clean(&e) {
            failures.push(format!("{path} {source:?}: {}", label(&e, source)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (HIGH, false Partial in Go, whole file charged). A Go file whose last
/// declaration is a `type` (struct or interface), a `var (...)`, `const (...)`
/// or `type (...)` group and has no newline after it is valid: the spec inserts
/// the semicolon at the end of the file. The grammar recovers with a hidden
/// zero-width token, main reports Clean, and the lane charges the whole file
/// (`0..len`), so the file loses every consumer that treats Partial as unable
/// to prove absence. Found by stripping the final newline of 5,000 real Go
/// files: 39 flipped. A function ending in `}`, `const X = 1` and an `import
/// (...)` group are not affected, and the same files with the newline are Clean.
#[test]
fn a_go_file_ending_in_a_declaration_without_a_newline_is_clean() {
    let mut failures = Vec::new();
    for body in [
        "type T struct {\n\tA int\n}",
        "type T struct{ A int }",
        "type T interface {\n\tDo()\n}",
        "var (\n\ta = 1\n)",
        "const (\n\ta = iota\n)",
        "type (\n\tA int\n\tB string\n)",
        "func f() {}\n\ntype T struct{}",
    ] {
        let source = format!("package a\n\n{body}");
        let with_newline = format!("{source}\n");
        let (a, b) = (ex("a.go", &source), ex("a.go", &with_newline));
        assert!(
            clean(&b),
            "control with a newline: {}",
            label(&b, &with_newline)
        );
        if !clean(&a) {
            failures.push(format!("{source:?}: {}", label(&a, &source)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HELD in the scan (listed so a later grammar bump that adds a hidden token is
/// caught): valid idioms from the other grammars the brief names, each Clean on
/// main and on the lane.
#[test]
fn valid_idioms_in_the_other_grammars_stay_clean() {
    let mut failures = Vec::new();
    for (path, source) in [
        ("a.py", "def f():\n    return 1"),
        ("b.py", "match p:\n    case [1, 2, *rest]:\n        pass\n    case {'a': 1, **kw}:\n        pass\n    case _:\n        pass\n"),
        ("c.py", "if (n := len(a)) > 1:\n    print(f\"{n!r:>{10}} {n=}\")\n"),
        ("d.py", "def f():\n    if 1:\n        pass\n# c\n    else:\n        pass\n"),
        ("e.ts", "@Component({selector: 'a'})\nexport class A { constructor(@Inject(B) private b: B) {} }\n"),
        ("f.tsx", "const a = <T,>(x: T) => <div {...p}>{x} &amp; <>{a ? <b/> : null}</></div>;\n"),
        ("g.rs", "macro_rules! m { ($($x:expr),* $(,)?) => { vec![$($x),*] }; }\nfn main() { let s = r#\"a \"q\" b\"#; }"),
        ("h.go", "package main\nfunc Map[T, U any](s []T, f func(T) U) []U { return nil }\n"),
        ("i.rb", "x = <<~E\n  a\nE"),
        ("j.php", "<html><?php if ($a): ?>\n<b><?= $x ?></b>\n<?php endif; ?>\n"),
        ("k.sh", "cat <<EOF\nx\nEOF"),
        ("l.c", "#ifdef A\n#define B\n#endif"),
        ("m.swift", "let a = (b as? String) ?? \"x\"\n"),
        ("n.swift", "#if os(macOS)\nlet x = 1\n#endif"),
        ("o.kt", "val x = 1"),
    ] {
        let e = ex(path, source);
        if !clean(&e) {
            failures.push(format!("{path} {source:?}: {}", label(&e, source)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ------------------------------------------------------------ Kotlin: twins

const STRIP_KEYS: &[&str] = &[
    "body_signature",
    "declaration_hash",
    "span",
    "start_byte",
    "end_byte",
    "body_start",
    "body_end",
    "line",
    "content_hash",
    "header",
    "body",
    "extent",
    "source_code",
    // The text of a directive includes the newline that ends it, which the
    // no-newline spelling of the same file does not have.
    "raw_import",
];

const IGNORED_FIELDS: &[&str] = &[
    // Go: what a Partial file publishes as every name a selector could reach
    // (`package`, `type`, `struct`, ...). A consequence of the verdict, not of
    // the parse, so it is the one table a false Partial changes by itself.
    "go_member_names",
    "parse_outcome",
    "decl_gaps",
    "recovered_decl_ranges",
    "diagnostics",
    "content_hash",
    "source_code",
];

fn strip(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for key in STRIP_KEYS {
                map.remove(*key);
            }
            for (_, inner) in map.iter_mut() {
                strip(inner);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                strip(item);
            }
        }
        _ => {}
    }
}

fn normalized(e: &Extraction) -> BTreeMap<String, Vec<String>> {
    let mut value = serde_json::to_value(e).expect("serialize");
    strip(&mut value);
    let mut out = BTreeMap::new();
    if let serde_json::Value::Object(map) = value {
        for (key, field) in map {
            if IGNORED_FIELDS.contains(&key.as_str()) {
                continue;
            }
            let mut rows: Vec<String> = match field {
                serde_json::Value::Array(items) => items.iter().map(|i| i.to_string()).collect(),
                other => vec![other.to_string()],
            };
            rows.sort();
            out.insert(key, rows);
        }
    }
    out
}

fn table_diffs(a: &Extraction, b: &Extraction) -> Vec<String> {
    let (na, nb) = (normalized(a), normalized(b));
    let keys: BTreeSet<&String> = na.keys().chain(nb.keys()).collect();
    let mut out = Vec::new();
    for key in keys {
        let x = na.get(key).cloned().unwrap_or_default();
        let y = nb.get(key).cloned().unwrap_or_default();
        if x == y {
            continue;
        }
        let mut only_x = x.clone();
        for row in &y {
            if let Some(at) = only_x.iter().position(|r| r == row) {
                only_x.remove(at);
            }
        }
        let mut only_y = y.clone();
        for row in &x {
            if let Some(at) = only_y.iter().position(|r| r == row) {
                only_y.remove(at);
            }
        }
        out.push(format!(
            "`{key}`: only one-line {only_x:?}; only twin {only_y:?}"
        ));
    }
    out
}

fn names(e: &Extraction) -> Vec<String> {
    let mut v: Vec<String> = e
        .symbols
        .iter()
        .filter(|s| s.kind != SymbolKind::File)
        .map(|s| s.qualified_name.clone())
        .collect();
    v.sort();
    v
}

/// The multi-line twin: every `{ ` opens a line and every ` }` closes one. The
/// shapes below hold no braces in strings, so this moves no token.
fn twin(one_line: &str) -> String {
    one_line
        .replace("{ ", "{\n")
        .replace(" }", "\n}")
        .replace("} class", "}\nclass")
}

const MEMBERS: &[&str] = &[
    "fun f() = if (c) { a() } else { b() }",
    "fun f() = when (x) { 1 -> { a() } else -> { b() } }",
    "fun f() = try { a() } catch (e: Exception) { b() } finally { c() }",
    "fun f(): Int { return if (c) { 1 } else { 2 } }",
    "val v = if (c) { a() } else { b() }",
    "val v: Int by lazy { if (c) { 1 } else { 2 } }",
    "val v = run { if (c) { val z = 1; z } else { val w = 2; w } }",
    "val v = listOf(1).map { x -> if (x > 1) { x } else { -x } }",
    "val v = c ?: run { d() } ?: e",
    "fun f() = g(if (c) { a() } else { b() }, if (d) { e() } else { h() })",
    "fun f() = if (c) { a() } else if (d) { b() } else { e() }",
    "fun f() { val q = if (c) { a() } else { b() }; use(q) }",
    "fun f() = if (c) { val a = 1; a } else { val b = 2; b }",
    "fun f(): () -> Int = if (c) { { 1 } } else { { 2 } }",
    "init { if (c) { a() } else { b() } }",
    "constructor(x: Int) : this(if (x > 0) { x } else { 0 })",
    "val x get() = if (c) { 1 } else { 2 }",
    "fun f() = if (c) { a() } else { b() }; fun g() = if (d) { a() } else { b() }",
    "fun <T> f(x: T) = if (c) { x } else { x }",
    "fun f() = object : Runnable { override fun run() { if (c) { a() } else { b() } } }",
    "fun f() = if (c) { a() } else { b() } /* c */",
    "suspend fun f() = coroutineScope { if (c) { a() } else { b() } }",
    "fun f() = if (c) { a() } else { throw E() }",
    "fun f() = (if (c) { a() } else { b() })",
    "fun f() = !if (c) { true } else { false }",
    "fun f() = if (c) { it -> a(it) } else { b() }",
    "fun f() = listOf(1).forEach { if (it > 1) { a(it) } else { b(it) } }",
    "fun f() = x.let { y -> when (y) { 1 -> { a(y) } else -> { b(y) } } }",
    "fun f() { for (i in 1..3) { if (i > 1) { a(i) } else { b(i) } } }",
    "fun f() { while (c) { if (d) { a() } else { b() } } }",
    "fun f() { val l = { x: Int -> if (x > 1) { x } else { 0 } }; l(1) }",
    "fun f() { val l = fun(x: Int): Int { return if (x > 1) { x } else { 0 } }; l(1) }",
    "val v = when { c -> { 1 } else -> { 2 } }",
    "fun f() = try { a() } catch (e: A) { b() } catch (e: B) { c() }",
    "fun f() = synchronized(this) { if (c) { a() } else { b() } }",
    "fun f() = with(x) { if (c) { a() } else { b() } }",
    "fun f() = x?.let { if (c) { a() } else { b() } } ?: d()",
    "val v by lazy(LazyThreadSafetyMode.NONE) { when (x) { 1 -> { a() } else -> { b() } } }",
];

const WRAPPERS: &[(&str, &str)] = &[
    ("class A { ", " }"),
    ("object O { ", " }"),
    ("interface I { ", " }"),
    ("enum class E { X, Y; ", " }"),
    ("class A { companion object { ", " } }"),
    ("abstract class A { inner class B { ", " } }"),
    ("sealed class S { class N : S() { ", " } }"),
    ("class A(val p: Int) { ", " }"),
    ("fun g() { class L { ", " } }"),
    ("val o = object { ", " }"),
    ("@Ann(1) class A { ", " }"),
    ("@Ann(1) object O { ", " }"),
    ("@Ann(1) interface I { ", " }"),
    ("@Ann(\"x\") data class D(val a: Int) { ", " }"),
    ("@Ann(1) enum class E { X; ", " }"),
    ("@Ann(1) class A { companion object { ", " } }"),
    ("@Ann(a = [1, 2]) @Bnn(3) class A { ", " }"),
];

/// A one-line class-like body whose member is read through the hidden-token
/// rewrite must equal its multi-line twin on every table, and a file the
/// lane reports Clean must not hold the phantom locals of a lambda that is not
/// there (`it`, `$0`) that main held.
#[test]
fn every_hidden_separator_shape_equals_its_twin_on_every_table() {
    let mut failures = Vec::new();
    for (open, close) in WRAPPERS {
        for member in MEMBERS {
            let one = format!("{open}{member}{close}\nfun top() {{}}\n");
            let tw = twin(&one);
            let (a, b) = (ex("A.kt", &one), ex("A.kt", &tw));
            if !clean(&b) {
                // The twin is not valid Kotlin (an artefact of the shape): skip it.
                continue;
            }
            if !clean(&a) {
                // Partial is honest; it is the lane's choice. Only a Clean
                // answer is held to the twin.
                continue;
            }
            let diffs = table_diffs(&a, &b);
            if !diffs.is_empty() {
                failures.push(format!("{one:?}: {}", diffs.join("; ")));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same grid, held to Clean: a valid one-line body is not left Partial.
#[test]
fn every_hidden_separator_shape_is_repaired_to_clean() {
    let mut failures = Vec::new();
    for (open, close) in WRAPPERS {
        for member in MEMBERS {
            let one = format!("{open}{member}{close}\nfun top() {{}}\n");
            let tw = twin(&one);
            let (a, b) = (ex("A.kt", &one), ex("A.kt", &tw));
            if clean(&b) && !clean(&a) {
                failures.push(format!("{one:?}: {}", label(&a, &one)));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Two one-line classes on one physical line are valid Kotlin and the same
/// pair on two lines is clean, but the rewrite finds no candidate in the root
/// ERROR and the whole file is Partial with no symbol at all.
#[test]
fn two_one_line_classes_on_one_line_are_repaired() {
    let mut failures = Vec::new();
    for one in [
        "class A { val x = 1 } class B { val y = 2 }\nfun top() {}\n",
        "class A { fun f() {} } class B { fun g() {} }\nfun top() {}\n",
        "@Ann(1) class A { val x = 1 } class B { val y = 2 }\nfun top() {}\n",
    ] {
        let tw = twin(one);
        let (a, b) = (ex("A.kt", one), ex("A.kt", &tw));
        assert!(clean(&b), "twin: {}", label(&b, &tw));
        if !clean(&a) || !table_diffs(&a, &b).is_empty() {
            failures.push(format!("{one:?}: {} names {:?}", label(&a, one), names(&a)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (LOW, valid file left Partial). Follow-up 2 repaired an annotation
/// with arguments before a one-line class at the top level, through the wrapper
/// re-parse. The same declaration inside a function body (a local class), or
/// followed by another declaration on its line, is still lost: the detector
/// charges it, the wrapper repair is not reached or fails, and the symbols are
/// missing. Their un-annotated spellings are Clean.
#[test]
fn an_annotated_one_line_class_in_another_position_is_repaired() {
    let mut failures = Vec::new();
    for (source, want) in [
        (
            "fun f() { @Ann(1) class L { val x = 1 } }\nfun top() {}\n",
            "A.kt::L",
        ),
        (
            "@Ann(1) class A { val x = 1 } fun g() {}\nfun top() {}\n",
            "A.kt::A",
        ),
        (
            "@Ann(1) class A { fun f() { @Ann(2) class L { val q = 1 } } }\nfun top() {}\n",
            "A.kt::L",
        ),
    ] {
        let e = ex("A.kt", source);
        if !clean(&e) || !names(&e).contains(&want.to_string()) {
            failures.push(format!(
                "{source:?}: {} names {:?} {:?}",
                label(&e, source),
                names(&e),
                e.diagnostics
            ));
        }
    }
    let plain = ex("A.kt", "fun f() { class L { val x = 1 } }\nfun top() {}\n");
    assert!(clean(&plain), "control: {}", label(&plain, ""));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ------------------------------------------------- determinism and the cap

fn json(e: &Extraction) -> String {
    serde_json::to_string(e).unwrap()
}

#[test]
fn a_repaired_file_extracts_identically_every_time() {
    let one = "class A { fun f() = if (c) { a() } else { b() } }\nclass B { companion object { val v = 1 } }\nfun top() {}\n";
    let first = json(&ex("A.kt", one));
    for _ in 0..20 {
        assert_eq!(json(&ex("A.kt", one)), first);
    }
}

/// 512 siblings is the cap: the file is Clean and equals its twin; 513 is
/// Partial, never Clean, and says why.
#[test]
fn the_separator_cap_is_exact_and_a_cap_hit_is_partial() {
    for (n, want_clean) in [(100usize, true), (512, true), (513, false), (600, false)] {
        let one: String = (0..n)
            .map(|i| format!("class C{i} {{ val x = {i} }}\n"))
            .collect::<String>()
            + "fun top() {}\n";
        let e = ex("A.kt", &one);
        assert_eq!(
            clean(&e),
            want_clean,
            "{n}: {}",
            label(&e, &one).chars().take(120).collect::<String>()
        );
        if want_clean {
            let tw = twin(&one);
            let diffs = table_diffs(&e, &ex("A.kt", &tw));
            assert!(diffs.is_empty(), "{n}: {}", diffs.join("; "));
        } else {
            assert!(
                e.diagnostics.iter().any(|d| d.contains("512")),
                "{n}: a cap hit names the cap: {:?}",
                e.diagnostics
            );
        }
    }
}

/// Garbage, an empty file, a lone annotation, a lone brace: never Clean unless
/// empty, never a symbol out of nothing.
#[test]
fn a_garbage_or_fragment_file_is_never_clean_and_names_nothing() {
    let mut failures = Vec::new();
    for source in [
        "}}}{{",
        "}",
        "@Foo",
        "@Foo class",
        "{",
        "{{{{",
        "class",
        "class A {",
        "}\n}\n}\n",
    ] {
        let e = ex("A.kt", source);
        if clean(&e) || !names(&e).is_empty() {
            failures.push(format!("{source:?}: {} {:?}", label(&e, source), names(&e)));
        }
    }
    let empty = ex("A.kt", "");
    if !clean(&empty) || !names(&empty).is_empty() {
        failures.push("empty file".to_string());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HELD (this is what makes the C and Go BREAKs above pure false Partials): the
/// flipped source extracts exactly the same tables as its benign twin, which is
/// Clean on the lane (the same text with a trailing newline). The hidden
/// zero-width token costs the grammar a recovery and costs the extraction
/// nothing; the only table that differs is `go_member_names`, which the false
/// Partial itself fills with every token of the file (ignored above, and named
/// there). Swift is not in this list: its tables do differ, see the next test.
#[test]
fn the_flipped_files_extract_the_same_tables_as_their_clean_twins() {
    let mut failures = Vec::new();
    for (path, flipped, twin_source) in [
        ("a.c", "#include \"x.h\"", "#include \"x.h\"\n"),
        ("a.h", "#define X 1", "#define X 1\n"),
        (
            "a.go",
            "package a\n\ntype T struct {\n\tA int\n}",
            "package a\n\ntype T struct {\n\tA int\n}\n",
        ),
        (
            "a.go",
            "package a\n\ntype T interface {\n\tDo()\n}",
            "package a\n\ntype T interface {\n\tDo()\n}\n",
        ),
    ] {
        let (a, b) = (ex(path, flipped), ex(path, twin_source));
        if !clean(&b) {
            failures.push(format!(
                "{path} {twin_source:?}: the twin is not Clean: {}",
                label(&b, twin_source)
            ));
        }
        let diffs = table_diffs(&a, &b);
        if !diffs.is_empty() {
            failures.push(format!("{path} {flipped:?}: {}", diffs.join("; ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (MEDIUM, a phantom fact). The hidden token that makes `x as? String ??
/// "d"` a recovery is read as a call with no name: the Swift walk publishes a
/// `Call` with `callee_name == ""` (and a `Call` reference named `""`) that the
/// parenthesised spelling of the same expression does not. On main the file
/// was Clean and carried it; on the lane it is Partial and still carries it.
/// An empty name can never be a callee, so it should be neither published nor
/// the reason a file is distrusted.
#[test]
fn a_swift_nil_coalesced_string_cast_publishes_no_call_with_an_empty_name() {
    let mut failures = Vec::new();
    for source in [
        "let a = b as? String ?? \"x\"\n",
        "func f(r: [String: Any]) -> String {\n    return r[\"e\"] as? String ?? \"x\"\n}\n",
    ] {
        let e = ex("a.swift", source);
        let calls = e.calls.iter().filter(|c| c.callee_name.is_empty()).count();
        let refs = e.references.iter().filter(|r| r.name.is_empty()).count();
        if calls != 0 || refs != 0 {
            failures.push(format!(
                "{source:?}: {calls} empty-name call(s), {refs} empty-name reference(s)"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
