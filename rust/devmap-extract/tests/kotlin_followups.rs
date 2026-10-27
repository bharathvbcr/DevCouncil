//! Kotlin follow-ups to extract-v98 rows 15 and 16, from the final adversarial
//! round (branch `attack3/v98-kotlin` @ 876e27e0).
//!
//! Method: a one-line class body is compared with its multi-line twin, written
//! by hand, which moves only the closing `}` of the class-like body onto its own
//! line. Every table (symbols, calls, references, exports, wiring,
//! scope_locals, scope_receivers, ...) is compared as a span-free multiset.
//!
//! The `followupN_*` tests are the five follow-ups; each failed on main
//! a0371296. The rest guard what the repair must keep: a real error beside a
//! one-line body is never Clean, a cap or a deadline is never Clean, and the
//! rewrite is what reads the grid. The module sits behind
//! `cfg(feature = "parse")` in `it.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use devmap_extract::extract_file_with_budget;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

/// Far above the default: a budget refusal is not what these tests are about.
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
                        .map(|s| s.chars().take(40).collect::<String>().replace('\n', "\\n"))
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        other => format!("{other:?}"),
    }
}

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
];

const IGNORED_FIELDS: &[&str] = &[
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
            for inner in items {
                strip(inner);
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

const SUFFIX: &str = "fun top() { helper() }\nfun helper() {}\n";

const EXPRS: &[&str] = &[
    "1",
    "x",
    "x.y",
    "f(1)",
    "f { a() }",
    "f { it }",
    "f { x -> x }",
    "{ 1 }",
    "{ x -> x }",
    "{ }",
    "if (c) { a() } else { b() }",
    "if (c) a() else b()",
    "if (c) { a() }",
    "if (c) 1 else { 2 }",
    "if (c) { 1 } else 2",
    "when (x) { 1 -> a() else -> b() }",
    "when (x) { 1 -> { a() } else -> { b() } }",
    "try { a() } catch (e: E) { b() }",
    "try { a() } finally { b() }",
    "object : T { fun g() {} }",
    "object { val q = 1 }",
    "run { a() }.b()",
    "f(g { 1 })",
    "\"s${a }\"",
    "\"${f { 1 } }\"",
    "'}'",
    "a ?: run { b() }",
    "x?.let { it }",
    "l@{ return@l 1 }",
    "f({ 1 }, { 2 })",
    "listOf(1).map { it }.filter { it > 0 }",
    "a + b",
    "a as? B",
    "a is B",
    "f { g { h() } }",
    "f { if (c) { a() } else { b() } }",
    "with(x) { y() }",
    "x.apply { y = 1 }",
    "\"} }\"",
    "\"\\\"} }\"",
    "\"\"\"} } \"\"\"",
    "\"\"\"${ a } } \"\"\"",
    "x /* } */ + 1",
    "f(/* } */ 1)",
    "\"${ \"} }\" }\"",
    "'\\u007D'",
    "\"$a }\"",
    "\"${a}} }\"",
    "f { \"}\" }",
    "f { '}' }",
    "f { /* } */ }",
    "listOf(\"}\", \"{ }\")",
    "mapOf(\"a\" to { 1 })",
    "f { x -> \"${x }\" }",
];

const STMTS: &[&str] = &[
    "a()",
    "if (c) { a() } else { b() }",
    "if (c) { a() }",
    "for (i in 1..2) { a() }",
    "while (c) { a() }",
    "do { a() } while (c)",
    "try { a() } catch (e: E) { b() }",
    "when (x) { 1 -> a() else -> b() }",
    "run { a() }",
    "f { g { h() } }",
    "val l = { 1 }",
    "return",
    "a(); b()",
    "x = { 1 }",
    "l@ for (i in 1..2) { break@l }",
    "if (c) { a() } else if (d) { b() } else { e() }",
    "{ a() }()",
    "val o = object : T { fun g() {} }",
    "fun inner() { a() }",
    "x?.let { it.y() }",
];

/// `(label, text with E for an expression and S for a statement)`.
const MEMBERS: &[(&str, &str)] = &[
    ("val-init", "val v = E"),
    ("fun-expr", "fun f() = E"),
    ("fun-block", "fun f() { S }"),
    ("fun-return", "fun f(): Int { return E }"),
    ("init", "init { S }"),
    ("getter-expr", "val v: Int get() = E"),
    ("getter-block", "val v: Int get() { return E }"),
    ("setter", "var v = 0; set(x) { S }"),
    ("delegate", "val v by E"),
    ("companion", "companion object { val k = E }"),
    ("nested-class", "class N { val k = E }"),
    ("fun-local", "fun f() { val l = E }"),
    ("fun-call-stmt", "fun f() { g(E) }"),
];

/// `(label, open, close)`: the wrapper around the member text.
const WRAPPERS: &[(&str, &str, &str)] = &[
    ("class", "class A", ""),
    ("object", "object A", ""),
    ("interface", "interface A", ""),
    ("anon-object", "val o = object", ""),
    ("open-class", "open class A(val q: Int) : Base()", ""),
];

struct Cell {
    wrapper: &'static str,
    member: &'static str,
    detail: String,
    one: String,
    twin: String,
}

fn fill(template: &str, e: &str, s: &str) -> String {
    template.replace('E', e).replace('S', s)
}

/// The same grid behind a prefix: with `class K { val x = 1 }` ahead of it every
/// shape is inside the first parse's ERROR, so the rewrite is what reads it.
fn generated_cells_with(one_prefix: &str, twin_prefix: &str) -> Vec<Cell> {
    let mut cells = Vec::new();
    for (wlabel, open, _) in WRAPPERS {
        for (mlabel, template) in MEMBERS {
            let uses_e = template.contains('E');
            let uses_s = template.contains('S');
            let es: Vec<&str> = if uses_e { EXPRS.to_vec() } else { vec![""] };
            let ss: Vec<&str> = if uses_s { STMTS.to_vec() } else { vec![""] };
            for e in &es {
                for s in &ss {
                    let member = fill(template, e, s);
                    // A nested body is expanded in the twin too, so the twin
                    // has no one-line body anywhere.
                    let expanded = if *mlabel == "companion" || *mlabel == "nested-class" {
                        member
                            .replacen(" { ", " {\n        ", 1)
                            .replacen(" }", "\n    }", 1)
                    } else {
                        member.clone()
                    };
                    let expanded = if expanded != member {
                        // restore the closing brace of the nested body only
                        let mut m = member.clone();
                        let at = m.find(" { ").unwrap();
                        m.replace_range(at..at + 3, " {\n        ");
                        let end = m.rfind(" }").unwrap();
                        m.replace_range(end..end + 2, "\n    }");
                        m
                    } else {
                        expanded
                    };
                    cells.push(Cell {
                        wrapper: wlabel,
                        member: mlabel,
                        detail: format!("E=`{e}` S=`{s}`"),
                        one: format!("{one_prefix}{open} {{ {member} }}\n{SUFFIX}"),
                        twin: format!("{twin_prefix}{open} {{\n    {expanded}\n}}\n{SUFFIX}"),
                    });
                }
            }
        }
    }
    cells
}

#[derive(Default)]
struct Tally {
    total: usize,
    ok: usize,
    repaired: usize,
    both_clean_tables_differ: Vec<String>,
    false_partial: Vec<String>,
    twin_not_clean: usize,
    one_clean_twin_partial: Vec<String>,
}

fn run_cells(cells: &[Cell]) -> Tally {
    let mut t = Tally::default();
    for cell in cells {
        t.total += 1;
        let a = ex("A.kt", &cell.one);
        let b = ex("A.kt", &cell.twin);
        let id = format!("{} / {} / {}", cell.wrapper, cell.member, cell.detail);
        if !a.recovered_decl_ranges.is_empty() {
            t.repaired += 1;
        }
        match (clean(&a), clean(&b)) {
            (true, true) => {
                let d = table_diffs(&a, &b);
                if d.is_empty() {
                    t.ok += 1;
                } else {
                    t.both_clean_tables_differ.push(format!(
                        "{id} (repaired={}): {}",
                        !a.recovered_decl_ranges.is_empty(),
                        d.join(" ; ")
                    ));
                }
            }
            (false, true) => t
                .false_partial
                .push(format!("{id}: {}", label(&a, &cell.one))),
            (true, false) => t
                .one_clean_twin_partial
                .push(format!("{id}: twin {}", label(&b, &cell.twin))),
            (false, false) => t.twin_not_clean += 1,
        }
    }
    t
}

const ATOMS: &[&str] = &[
    "val",
    "var",
    "fun",
    "class",
    "x",
    "f()",
    "=",
    "1",
    "(",
    ")",
    "{",
    "}",
    ";",
    ",",
    ":",
    "->",
    "if",
    "else",
    "@A",
    "\"s\"",
    "get()",
    "by",
    "object",
    "companion",
    "init",
    "this",
    "T",
    "<",
    ">",
    "+",
    ".",
    "g { 1 }",
    "h()",
    "in",
    "return",
];

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const FUZZ_WRAPPERS: &[&str] = &[
    "class A",
    "object A",
    "interface A",
    "val o = object",
    "enum class E",
];

fn mutated_fragment(rng: &mut Lcg) -> String {
    let members = 1 + rng.pick(2);
    let mut parts = Vec::new();
    for _ in 0..members {
        let (_, template) = MEMBERS[rng.pick(MEMBERS.len())];
        parts.push(fill(
            template,
            EXPRS[rng.pick(EXPRS.len())],
            STMTS[rng.pick(STMTS.len())],
        ));
    }
    let base = parts.join(if rng.pick(2) == 0 { "; " } else { " " });
    let mut tokens: Vec<String> = base.split(' ').map(str::to_string).collect();
    let edits = rng.pick(3); // 0, 1 or 2 mutations
    for _ in 0..edits {
        if tokens.is_empty() {
            break;
        }
        let at = rng.pick(tokens.len());
        match rng.pick(4) {
            0 => {
                tokens.remove(at);
            }
            1 => tokens.insert(at, ATOMS[rng.pick(ATOMS.len())].to_string()),
            2 => tokens[at] = ATOMS[rng.pick(ATOMS.len())].to_string(),
            _ => {
                let t = tokens[at].clone();
                tokens.insert(at, t);
            }
        }
    }
    tokens.join(" ")
}

/// Errors at file level, each carrying the marker `ZZERR`.
const FILE_ERRORS: &[&str] = &[
    "fun ZZERR( {",
    "val ZZERR = ",
    "val = ZZERR",
    "ZZERR ZZERR ZZERR )",
    "class ZZERR :",
    "@ZZERR(",
    "fun ZZERR(a: ) {}",
    "import ZZERR.",
];

/// Errors that are members of a class body.
const MEMBER_ERRORS: &[&str] = &[
    "val = ZZERR",
    "fun ZZERR(",
    "ZZERR ZZERR",
    "class ZZERR :",
    "val ZZERR: = 1",
    "fun ZZERR() = ",
];

/// Whether an error range of `e` touches the marker's line or the next one: the
/// grammar reports an unfinished declaration at the token that cannot continue
/// it, which is on the following line.
fn covers_marker(e: &Extraction, source: &str, marker: &str) -> bool {
    let ParseOutcome::Partial { error_ranges, .. } = &e.parse_outcome else {
        return false;
    };
    let mut from = 0;
    while let Some(at) = source[from..].find(marker) {
        let at = from + at;
        let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
        let after = source[at..].find('\n').map_or(source.len(), |i| at + i + 1);
        let window_end = source[after..]
            .find('\n')
            .map_or(source.len(), |i| after + i);
        if error_ranges
            .iter()
            .any(|r| r.start_byte < window_end && line_start < r.end_byte.max(r.start_byte + 1))
        {
            return true;
        }
        from = at + marker.len();
    }
    false
}

/// `(label, source)` pairs with one real error and at least one one-line body.
fn error_beside_body_cases() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for err in FILE_ERRORS {
        for (place, text) in [
            (
                "before",
                format!("{err}\nclass K {{ val x = 1 }}\n{SUFFIX}"),
            ),
            ("after", format!("class K {{ val x = 1 }}\n{err}\n{SUFFIX}")),
            (
                "between",
                format!("class K {{ val x = 1 }}\n{err}\nclass L {{ fun g() {{ }} }}\n{SUFFIX}"),
            ),
            (
                "same line after",
                format!("class K {{ val x = 1 }} {err}\n{SUFFIX}"),
            ),
            (
                "inside a method",
                format!("class K {{ fun f() {{ {err} }} }}\n{SUFFIX}"),
            ),
        ] {
            out.push((format!("file error `{err}` {place}"), text));
        }
    }
    for err in MEMBER_ERRORS {
        for (place, text) in [
            (
                "member after",
                format!("class K {{ val x = 1; {err} }}\n{SUFFIX}"),
            ),
            (
                "member before",
                format!("class K {{ {err}; val x = 1 }}\n{SUFFIX}"),
            ),
            (
                "member of a nested one-liner",
                format!("class K {{ class N {{ {err} }} }}\n{SUFFIX}"),
            ),
            (
                "member, second class",
                format!("class K {{ val x = 1 }}\nclass L {{ {err} }}\n{SUFFIX}"),
            ),
            (
                "interface member",
                format!("interface I {{ fun f(); {err} }}\n{SUFFIX}"),
            ),
        ] {
            out.push((format!("member error `{err}` {place}"), text));
        }
    }
    out
}

#[test]
fn a_real_error_beside_a_one_line_body_is_never_reported_clean() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for (what, source) in error_beside_body_cases() {
        let e = ex("A.kt", &source);
        checked += 1;
        if clean(&e) {
            failures.push(format!("{what}: reported Clean\n{source}"));
        } else if !covers_marker(&e, &source, "ZZERR") {
            failures.push(format!(
                "{what}: Partial but no error range covers the marker: {}\n{source}",
                label(&e, &source)
            ));
        }
    }
    assert_eq!(checked, 70);
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// Unbalanced braces around a one-line body: a stray `}` or a missing one is a
/// real error whatever separators are written.
#[test]
fn unbalanced_braces_around_a_one_line_body_stay_partial() {
    let cases = [
        "class K { val x = 1 } }\n",
        "class K { val x = 1 } }\nclass L { val y = 2 } }\n",
        "class K { fun f() { g() }\n",
        "class K { fun f() { g() } \n",
        "class K { class N { val x = 1 } \n",
        "class K { val x = 1 }\n}\n",
        "}\nclass K { val x = 1 }\n",
        "class K { val x = 1 } } }\n",
        "class K { fun f() { if (a) { b() } }\n",
        "class K { val s = \"${ a }\n",
        "class K { val x = 1 }\nfun f() {\n",
        "class K { val x = 1 }\nfun f() { a( }\n",
    ];
    let mut failures = Vec::new();
    for case in cases {
        let source = format!("{case}{SUFFIX}");
        let e = ex("A.kt", &source);
        if clean(&e) {
            failures.push(format!("Clean:\n{source}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// The missing separator the brief names: it is between two members, not before
/// the `}`, so the rewrite cannot reach it and the file stays Partial over the
/// real error (and equals what its multi-line twin says).
#[test]
fn a_missing_separator_between_members_is_not_fixed_by_the_rewrite() {
    for (one, twin) in [
        (
            "class A { val x = 1 val y = 2 }\n",
            "class A {\n    val x = 1 val y = 2\n}\n",
        ),
        (
            "class A { fun f() = 1 fun g() = 2 }\n",
            "class A {\n    fun f() = 1 fun g() = 2\n}\n",
        ),
        (
            "class K { val x = 1 }\nclass A { val x = 1 val y = 2 }\n",
            "class K {\n    val x = 1\n}\nclass A {\n    val x = 1 val y = 2\n}\n",
        ),
    ] {
        let (a, b) = (
            ex("A.kt", &format!("{one}{SUFFIX}")),
            ex("A.kt", &format!("{twin}{SUFFIX}")),
        );
        assert_eq!(
            clean(&a),
            clean(&b),
            "{one:?}: one-line {} twin {}",
            label(&a, one),
            label(&b, twin)
        );
        assert!(!clean(&a), "{one:?} was reported Clean");
        assert!(a.recovered_decl_ranges.is_empty(), "{one:?}");
    }
}

/// Dangling prefixes a `;` could be mistaken for the missing rest of.
#[test]
fn a_dangling_prefix_before_the_closing_brace_stays_partial() {
    let prefixes = [
        "private",
        "override",
        "abstract",
        "companion",
        "const",
        "@A",
        "@A(1)",
        "val",
        "var",
        "fun",
        "class",
        "object",
        "interface",
        "init",
        "get",
        "set",
        "constructor",
        "typealias",
        "enum",
        "sealed",
        "data",
        "inner",
        "lateinit",
        "operator",
        "vararg",
        "fun f",
        "val x",
        "val x: Int =",
        "fun f() =",
        "val x by",
        "class B :",
        "class B(",
        "fun <T>",
        "fun f(x:",
    ];
    let mut failures = Vec::new();
    for prefix in prefixes {
        for wrapper in ["class K", "object K", "interface K", "enum class K"] {
            let one = format!("{wrapper} {{ {prefix} }}\n{SUFFIX}");
            let twin = format!("{wrapper} {{\n    {prefix}\n}}\n{SUFFIX}");
            let (a, b) = (ex("A.kt", &one), ex("A.kt", &twin));
            if clean(&a) && !clean(&b) {
                failures.push(format!(
                    "`{wrapper} {{ {prefix} }}` is Clean, its twin is {}",
                    label(&b, &twin)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn nested_shape(kind: usize, depth: usize) -> String {
    // Different nesting constructs, so that a pass count is not an artefact of
    // one of them.
    let (open, mid, close) = match kind {
        0 => ("class C{i} { ", "val x = 1", " }"),
        1 => ("object C{i} { ", "val x = 1", " }"),
        2 => ("class C{i} { val v{i} = object { ", "val x = 1", " } }"),
        3 => ("class C{i} { fun f{i}() { run { ", "x()", " } } }"),
        4 => ("class C{i} { companion object { ", "val x = 1", " } }"),
        _ => ("interface C{i} { class N{i} { ", "val x = 1", " } }"),
    };
    let mut s = String::new();
    for i in 0..depth {
        s.push_str(&open.replace("{i}", &i.to_string()));
    }
    s.push_str(mid);
    for _ in 0..depth {
        s.push_str(close);
    }
    format!("{s}\n{SUFFIX}")
}

fn many_classes(n: usize, one_line: bool) -> String {
    let mut s = String::new();
    for i in 0..n {
        if one_line {
            s.push_str(&format!("class C{i} {{ val x = 1 }}\n"));
        } else {
            s.push_str(&format!("class C{i} {{\n    val x = 1\n}}\n"));
        }
    }
    s.push_str(SUFFIX);
    s
}

fn note_of(e: &Extraction) -> String {
    e.diagnostics
        .iter()
        .find(|n| n.contains("one-line class body"))
        .cloned()
        .unwrap_or_default()
}
/// One one-line class whose method holds `k` lambdas: every lambda close is a
/// candidate in the first pass (the whole body is one flat ERROR), so the
/// separator count is `k + 2` without needing more than one pass.
fn lambda_body_class(k: usize, one_line: bool) -> String {
    let body = (0..k).map(|_| "a { b() };").collect::<Vec<_>>().join(" ");
    if one_line {
        format!("class K {{ val x = 1 }}\nclass A {{ fun f() {{ {body} }} }}\n{SUFFIX}")
    } else {
        format!("class K {{\n    val x = 1\n}}\nclass A {{\n    fun f() {{ {body} }}\n}}\n{SUFFIX}")
    }
}
fn announced(e: &Extraction, at: usize) -> bool {
    let in_range = |r: &devmap_extract::model::TextRange| r.start_byte <= at && at < r.end_byte;
    e.decl_gaps.iter().any(in_range)
        || matches!(&e.parse_outcome, ParseOutcome::Partial { error_ranges, .. } if error_ranges.iter().any(in_range))
}

/// BREAK (MEDIUM, false Partial). Each pass of the rewrite reveals one more
/// sibling one-line class, so `MAX_PASSES = 6` is reached by seven plain
/// one-line classes, not by an exotic shape. The multi-line twin is Clean.
#[test]
#[ignore = "follow-up 3, fixed in its own commit"]
fn followup3_seven_sibling_one_line_classes_are_repaired() {
    let mut failures = Vec::new();
    for n in [6usize, 7, 8, 10] {
        let one = many_classes(n, true);
        let e = ex("A.kt", &one);
        if !clean(&e) {
            failures.push(format!(
                "{n} one-line classes: {} ({})",
                label(&e, &one).chars().take(40).collect::<String>(),
                note_of(&e)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HELD. A file the rewrite gives up on stays Partial, publishes nothing the
/// multi-line twin does not, and every declaration it does not publish lies
/// inside a range it announces (an error range or a decl gap).
#[test]
fn a_file_the_rewrite_gives_up_on_publishes_nothing_extra_and_announces_what_it_lost() {
    let mut failures = Vec::new();
    for n in [7usize, 8, 9, 10, 11, 12, 14, 15, 40, 100, 520, 600] {
        let one = many_classes(n, true);
        let twin = many_classes(n, false);
        let (a, b) = (ex("A.kt", &one), ex("A.kt", &twin));
        assert!(clean(&b), "twin of {n}: {}", label(&b, &twin));
        if clean(&a) {
            continue;
        }
        if !a.recovered_decl_ranges.is_empty() {
            failures.push(format!("{n}: Partial yet repaired ranges recorded"));
        }
        let twin_names: BTreeSet<String> = names(&b).into_iter().collect();
        for name in names(&a) {
            if !twin_names.contains(&name) {
                failures.push(format!("{n}: published `{name}`, absent from the twin"));
            }
        }
        let have: BTreeSet<String> = names(&a).into_iter().collect();
        for i in 0..n {
            let q = format!("A.kt::C{i}");
            if !have.contains(&q) {
                let at = one.find(&format!("class C{i} {{")).unwrap();
                if !announced(&a, at) {
                    failures.push(format!(
                        "{n}: `{q}` is neither published nor inside an announced range"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HELD. Past the cap the file stays Partial and says which bound stopped it.
#[test]
fn past_the_separator_cap_the_file_stays_partial() {
    for n in [520usize, 600, 1000] {
        let one = many_classes(n, true);
        let e = ex("A.kt", &one);
        assert!(!clean(&e), "{n}");
        assert!(e.recovered_decl_ranges.is_empty(), "{n}");
        assert!(note_of(&e).contains("not repaired"), "{n}: {}", note_of(&e));
    }
}

fn json(e: &Extraction) -> String {
    serde_json::to_string(e).unwrap()
}

/// A deadline that expires at any point of the rewrite never yields a Clean
/// result that differs from the unconstrained one: it is a budget refusal
/// (Failed), a Partial, or exactly the unconstrained answer.
#[test]
fn a_deadline_that_expires_mid_rewrite_never_yields_a_half_repaired_clean() {
    let sources = [
        ("six classes (six passes)", many_classes(6, true)),
        (
            "one class",
            "class A { val x = 1 }\nfun top() {}\n".to_string(),
        ),
        ("lambda body", lambda_body_class(300, true)),
        ("nested four", nested_shape(0, 4)),
    ];
    let micros = [
        0u64, 1, 5, 20, 50, 100, 200, 400, 800, 1_500, 3_000, 6_000, 12_000, 25_000, 50_000,
        100_000,
    ];
    let mut failures = Vec::new();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (name, source) in &sources {
        let full = ex("A.kt", source);
        assert!(clean(&full), "{name}: {}", label(&full, source));
        let full_json = json(&full);
        for us in micros {
            let e = extract_file_with_budget("A.kt", source, Duration::from_micros(us));
            let kind = match &e.parse_outcome {
                ParseOutcome::Clean => {
                    if json(&e) != full_json {
                        failures.push(format!(
                            "{name} @ {us}us: Clean but not what the unconstrained run says:\n  got   {:?}\n  names {:?}\n  want  {:?}",
                            e.diagnostics,
                            names(&e),
                            names(&full)
                        ));
                    }
                    "clean-equal"
                }
                ParseOutcome::Partial { .. } => "partial",
                ParseOutcome::Failed { .. } => "failed",
                _ => "other",
            };
            *seen.entry(kind).or_default() += 1;
            if matches!(e.parse_outcome, ParseOutcome::Partial { .. })
                && !e.recovered_decl_ranges.is_empty()
            {
                failures.push(format!("{name} @ {us}us: Partial with recovered ranges"));
            }
        }
    }
    println!("deadline outcomes {seen:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (attack 1, severity MEDIUM-HIGH). The rewrite is gated on a *visible*
/// error, on the claim that a hidden zero-width `_class_member_semi` "costs
/// nothing". It costs the parse of an `if` used as an expression: the grammar
/// reads `{ a() }` after `if (c)` as a `lambda_literal`, so the enclosing
/// function gets the implicit parameters of a lambda it does not contain
/// (`it`, `$0`..`$2`) and a call in the branch is bound to the property. The
/// same text behind an unrelated one-liner (visible error, so rewritten) and
/// in its multi-line twin is read correctly: the tables depend on whether an
/// unrelated class happens to precede it.
#[test]
fn followup1_a_hidden_missing_separator_reads_an_if_branch_as_a_lambda() {
    let mut failures = Vec::new();
    for (label_, one, twin) in [
        (
            "expression body",
            "class A { fun f() = if (c) { a() } else { b() } }\nfun top() {}\n",
            "class A {\n    fun f() = if (c) { a() } else { b() }\n}\nfun top() {}\n",
        ),
        (
            "property initializer",
            "class F { val v = if (c) { A() } else { B() } }\nfun top() {}\n",
            "class F {\n    val v = if (c) { A() } else { B() }\n}\nfun top() {}\n",
        ),
    ] {
        let (a, b) = (ex("A.kt", one), ex("A.kt", twin));
        assert!(clean(&a) && clean(&b), "{label_}");
        failures.extend(
            table_diffs(&a, &b)
                .into_iter()
                .map(|d| format!("{label_}: {d}")),
        );
        // The control: the same one-liner behind a visible-error one is right.
        let behind = format!("class K {{ val x = 1 }}\n{one}");
        let behind_twin = format!("class K {{\n    val x = 1\n}}\n{twin}");
        let (c, d) = (ex("A.kt", &behind), ex("A.kt", &behind_twin));
        assert!(
            table_diffs(&c, &d).is_empty(),
            "{label_}: behind a visible-error one-liner the file must already equal its twin"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (attack 5, severity MEDIUM). A class with an annotation that takes
/// arguments and a one-line body is read by the grammar as a *clean* expression
/// (`annotated_expression` + `infix_expression`), so there is no visible error
/// for the rewrite to start from; the loss detector charges it, and the
/// row-15 wrapper parse (a raw parse, no rewrite) cannot prove it. `A` is lost
/// (announced as Partial, but lost) although `;` before the brace reads it
/// correctly and the annotation-free spelling is repaired.
#[test]
#[ignore = "follow-up 2, fixed in its own commit"]
fn followup2_an_annotation_with_arguments_before_a_one_line_class_is_repaired() {
    let mut failures = Vec::new();
    for (label_, source, want) in [
        (
            "class",
            "@Suppress(\"unused\") class A { val x = 1 }\nfun top() {}\n",
            "A.kt::A",
        ),
        (
            "class with a method",
            "@Ann(1) class A { fun f() { g() } }\nfun top() {}\n",
            "A.kt::A.f",
        ),
        (
            "object",
            "@Suppress(\"unused\") object O { val x = 1 }\nfun top() {}\n",
            "A.kt::O",
        ),
        (
            "interface",
            "@Suppress(\"unused\") interface I { fun f() }\nfun top() {}\n",
            "A.kt::I.f",
        ),
    ] {
        let e = ex("A.kt", source);
        if !clean(&e) || !names(&e).contains(&want.to_string()) {
            failures.push(format!(
                "{label_}: {} names={:?} {:?}",
                label(&e, source).chars().take(60).collect::<String>(),
                names(&e),
                e.diagnostics
            ));
        }
    }
    // The control: no argument list, same shape, is repaired.
    let control = ex("A.kt", "@Ann class A { val x = 1 }\nfun top() {}\n");
    assert!(clean(&control) && names(&control).contains(&"A.kt::A".to_string()));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (attack 1/4, severity LOW). `candidates` looks only at the root's
/// children, so a tree whose *root* is the ERROR node (not a `source_file`
/// holding one) has no candidate at all and the file stays Partial with the
/// diagnostic "no space before a closing brace is left" although there is one.
/// Five nested one-line classes, and `open class` with a supertype call around
/// a local object expression, both hit it.
#[test]
#[ignore = "follow-up 4, fixed in its own commit"]
fn followup4_a_tree_whose_root_is_the_error_node_is_repaired() {
    let mut failures = Vec::new();
    for (label_, source) in [
        (
            "open class with a supertype call and `return object`",
            "open class A : B() { fun f(): Int { return object { val q = 1 } } }\nfun top() {}\n"
                .to_string(),
        ),
        (
            "class with a local object expression",
            "class A { val v = 1; fun f() { val o = object : T { fun g() {} } } }\nfun top() {}\n"
                .to_string(),
        ),
        ("five nested one-line classes", nested_shape(0, 5)),
        ("five nested one-line objects", nested_shape(1, 5)),
    ] {
        let e = ex("A.kt", &source);
        if !clean(&e) {
            failures.push(format!(
                "{label_}: {} {:?}",
                label(&e, &source).chars().take(50).collect::<String>(),
                note_of(&e)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK (attack 1, severity LOW). `ends_a_member` is a byte test: a member that
/// ends in a non-ASCII letter, in `++`/`--`, or in a block comment is never
/// given a separator, so a one-line body that needs one stays Partial.
#[test]
#[ignore = "follow-up 5, fixed in its own commit"]
fn followup5_a_member_ending_in_a_non_ascii_letter_or_a_comment_is_repaired() {
    let mut failures = Vec::new();
    for (label_, source) in [
        (
            "non-ASCII identifier",
            "class K { companion object { val k = caf\u{e9} } }\nfun top() {}\n",
        ),
        (
            "post-increment",
            "class K { val x = 1 }\nclass L { val v = x++ }\nfun top() {}\n",
        ),
        (
            "block comment before the brace",
            "class A { val x = 1 /* c */ }\nfun top() {}\n",
        ),
    ] {
        let e = ex("A.kt", source);
        if !clean(&e) {
            failures.push(format!(
                "{label_}: {}",
                label(&e, source).chars().take(60).collect::<String>()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// HELD (attack 1). Behind `class K { val x = 1 }` every shape of the grid is
/// read through the rewrite; every table equals the table of the multi-line twin.
/// 3,000 cells: member templates x expressions (including braces in strings,
/// chars, raw strings and comments, lambdas followed by calls, `when`, `if`)
/// x five wrappers.
#[test]
fn every_repaired_cell_of_the_grid_equals_its_multi_line_twin_on_every_table() {
    let cells = generated_cells_with("class K { val x = 1 }\n", "class K {\n    val x = 1\n}\n");
    let t = run_cells(&cells);
    assert!(
        t.repaired > 2500,
        "the rewrite must be what read the cells: {}",
        t.repaired
    );
    assert!(
        t.both_clean_tables_differ.is_empty(),
        "{}",
        t.both_clean_tables_differ.join("\n")
    );
    assert!(
        t.one_clean_twin_partial.is_empty(),
        "{}",
        t.one_clean_twin_partial.join("\n")
    );
}

/// HELD (attack 2). 60,000 mutated fragments (token deleted, inserted, replaced
/// or duplicated) in one-line bodies, with and without a one-liner ahead of
/// them: a fragment whose multi-line twin is not Clean is never reported Clean.
#[test]
fn a_mutated_fragment_is_never_clean_when_its_multi_line_twin_is_not() {
    for (seed, prefixed) in [(11u64, false), (12, true), (13, true)] {
        let mut rng = Lcg(seed);
        let (pre_one, pre_twin) = if prefixed {
            ("class K { val x = 1 }\n", "class K {\n    val x = 1\n}\n")
        } else {
            ("", "")
        };
        let mut masked = Vec::new();
        for _ in 0..20_000 {
            let frag = mutated_fragment(&mut rng);
            let open = FUZZ_WRAPPERS[rng.pick(FUZZ_WRAPPERS.len())];
            let one = format!("{pre_one}{open} {{ {frag} }}\n{SUFFIX}");
            let twin = format!("{pre_twin}{open} {{\n{frag}\n}}\n{SUFFIX}");
            let (a, b) = (ex("A.kt", &one), ex("A.kt", &twin));
            if clean(&a) && !clean(&b) {
                masked.push(format!("{open} `{frag}` twin {}", label(&b, &twin)));
            }
        }
        assert!(masked.is_empty(), "seed {seed}: {}", masked.join("\n"));
    }
}
