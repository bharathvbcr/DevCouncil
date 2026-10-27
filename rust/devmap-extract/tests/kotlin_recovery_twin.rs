//! A swallowed Kotlin statement, once recovered, must say what the same
//! statement says when the grammar reads it correctly.
//!
//! Each case is written twice: behind two annotations (the grammar reads it as
//! an expression and the extractor recovers it) and behind one (an ordinary
//! declaration, the control twin). Every field of the two extractions must
//! agree apart from spans and the recovery's own records.

use std::time::Duration;

use devmap_extract::extract_file_with_budget;
use devmap_extract::model::{Extraction, ParseOutcome};

/// Far above the default: a budget refusal is not what this test is about.
const BUDGET: Duration = Duration::from_secs(300);

fn extract(path: &str, source: &str) -> Extraction {
    extract_file_with_budget(path, source, BUDGET)
}

fn text(source: &str, a: usize, b: usize) -> String {
    source
        .get(a..b)
        .map(|s| s.chars().take(60).collect::<String>().replace('\n', "\\n"))
        .unwrap_or_else(|| format!("<bad span {a}..{b}>"))
}

fn outcome_label(e: &Extraction, source: &str) -> String {
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
                    text(source, r.start_byte, r.end_byte)
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        other => format!("{other:?}"),
    }
}

/// Whether the locked Kotlin grammar itself reports a syntax error for `source`.
fn grammar_error(source: &str) -> bool {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
        .expect("kotlin grammar");
    parser
        .parse(source, None)
        .expect("tree")
        .root_node()
        .has_error()
}

fn is_clean(e: &Extraction) -> bool {
    matches!(e.parse_outcome, ParseOutcome::Clean)
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

/// Each top-level field of an extraction, as a sorted multiset of span-free JSON.
fn normalized(e: &Extraction) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut value = serde_json::to_value(e).expect("serialize");
    strip(&mut value);
    let mut out = std::collections::BTreeMap::new();
    if let serde_json::Value::Object(map) = value {
        for (key, field) in map {
            let mut rows: Vec<String> = match field {
                serde_json::Value::Array(items) => items
                    .iter()
                    .filter(|i| {
                        // the annotation names themselves differ between the arms
                        !(key == "references"
                            && i.get("name")
                                .and_then(|n| n.as_str())
                                .is_some_and(|n| ["A", "B", "Preview", "Composable"].contains(&n)))
                    })
                    .map(|i| i.to_string())
                    .collect(),
                other => vec![other.to_string()],
            };
            rows.sort();
            out.insert(key, rows);
        }
    }
    out
}

pub(crate) fn diff_against_twin(label: &str, recovered_src: &str, twin_src: &str) -> Vec<String> {
    let r = extract("A.kt", recovered_src);
    let t = extract("A.kt", twin_src);
    let mut out = Vec::new();
    if grammar_error(twin_src) || !is_clean(&t) {
        out.push(format!(
            "[{label}] TWIN is not clean ({}) - bad case",
            outcome_label(&t, twin_src)
        ));
        return out;
    }
    if !is_clean(&r) {
        out.push(format!(
            "[{label}] recovered file is {} (twin Clean)",
            outcome_label(&r, recovered_src)
        ));
    }
    let (nr, nt) = (normalized(&r), normalized(&t));
    for key in nr
        .keys()
        .chain(nt.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if [
            "parse_outcome",
            "decl_gaps",
            "recovered_decl_ranges",
            "diagnostics",
        ]
        .contains(&key.as_str())
        {
            continue;
        }
        let a = nr.get(key).cloned().unwrap_or_default();
        let b = nt.get(key).cloned().unwrap_or_default();
        if a != b {
            let only_r: Vec<&String> = a.iter().filter(|x| !b.contains(x)).collect();
            let only_t: Vec<&String> = b.iter().filter(|x| !a.contains(x)).collect();
            let dup_r = a.len() != b.len() && only_r.is_empty() && only_t.is_empty();
            out.push(format!(
                "[{label}] field `{key}` differs (recovered {} rows, twin {}){}\n    only recovered: {:?}\n    only twin: {:?}",
                a.len(), b.len(), if dup_r { " [MULTIPLICITY ONLY]" } else { "" }, only_r, only_t
            ));
        }
    }
    out
}

fn twin_cases() -> Vec<(&'static str, &'static str)> {
    vec![
        ("fun-block", "private fun f() {\n    val y = helper(1)\n    other(y)\n}"),
        ("fun-expr", "private fun f() = helper()"),
        ("class-members", "private class Q {\n    fun m() = helper()\n    companion object {\n        fun cf() = helper()\n    }\n    class In {\n        fun deep() = helper()\n    }\n}"),
        ("val-init", "private val y = helper()"),
        ("object", "private object O {\n    val x = helper()\n    fun m() { helper() }\n}"),
        ("lambdas", "private fun lam() {\n    listOf(1).forEach { x -> helper(x) }\n    with(Foo()) { bar() }\n    Foo().apply { baz() }\n}"),
        ("enum-body", "private enum class E {\n    X {\n        override fun m() { helper() }\n    };\n    open fun m() {}\n}"),
        ("interface", "private interface I {\n    fun m(): Int\n    fun n() = helper()\n}"),
        ("typealias", "private typealias T = List<Int>"),
        ("ui", "private fun ui() {\n    Button(onClick = { onClick() }) { Text(\"x\") }\n}"),
        ("control-flow", "private fun cf() {\n    try {\n        when (1) { 1 -> helper(); else -> other(2) }\n    } catch (e: Exception) { } finally { after() }\n}"),
        ("local-bindings", "private fun lb() {\n    val a: Foo = Foo()\n    val b = Foo.make()\n    a.run(); b.run()\n}"),
        ("local-fun", "private fun nested() {\n    fun inner() = helper()\n    inner()\n}"),
        ("template", "private fun st() = \"n=${helper()}\""),
        ("receiver-lambda", "private fun rl() = Foo().apply { bar() }"),
        ("getter-setter", "private var gs: Int = 0\n    get() = field + helper()\n    set(v) { field = v }"),
        ("literal", "private fun lit() {\n    val lit = \"hello\"\n    println(lit + helper())\n}"),
        ("internal-fun", "internal fun gg() { helper() }"),
        ("open-class", "open class Base2 {\n    fun a() = helper()\n}"),
        ("plain-class", "class Plain2 {\n    fun a() = helper()\n}"),
        ("lambda-params", "private fun lp() {\n    listOf(1).map { it + 1 }.forEach { println(it) }\n}"),
        ("nested-lambda-locals", "private fun nl() {\n    val a = 1\n    run {\n        val b = a\n        helper(b)\n    }\n}"),
        ("scope-receiver", "private fun sr() {\n    Foo().apply {\n        bar()\n    }\n}"),
        ("typed-property", "private val ty: Foo = Foo()"),
        ("typed-nullable-property", "private var tn: List<Foo>? = null"),
        ("ext-property", "private val Foo.extp: Int get() = 1"),
        ("class-supertype", "private class Q2 : Base() {\n    fun m() = helper()\n}"),
        ("object-supertype", "private object O2 : Base() {\n    fun m() = helper()\n}"),
        ("interface-supertype", "private interface I2 : Marker {\n    fun m()\n}"),
        ("class-primary-ctor", "private class W {\n    constructor(a: Int)\n    fun m() = helper()\n}"),
        ("class-with-property-types", "private class P {\n    val a: Foo = Foo()\n    var b: Base? = null\n    fun m(x: Foo): Base = x\n}"),
    ]
}

#[test]
fn a_recovered_statement_equals_its_control_twin() {
    let tail = "\nfun helper() = 1\nfun other(x: Int) = x\nfun after() {}\nopen class Base\ninterface Marker\nclass Foo {\n    fun bar() {}\n    fun baz() {}\n    fun run() {}\n    companion object {\n        fun make() = Foo()\n    }\n}\n";
    let mut diffs = Vec::new();
    let mut notes = Vec::new();
    let mut compared = 0usize;
    for (label, stmt) in twin_cases() {
        for (ann_r, ann_t) in [
            ("@A\n@B\n", "@A\n"),
            ("@Preview\n@Composable\n", "@Preview\n"),
        ] {
            let rec = format!("val before = 0\n{ann_r}{stmt}{tail}");
            let twin = format!("val before = 0\n{ann_t}{stmt}{tail}");
            let r = extract("A.kt", &rec);
            if r.recovered_decl_ranges.is_empty() && !grammar_error(&twin) && is_clean(&r) {
                // the stack did not even trigger; not a recovery case
                notes.push(format!("[{label}] no recovery happened for {ann_r:?}"));
                continue;
            }
            compared += 1;
            diffs.extend(diff_against_twin(
                &format!("{label}/{}", ann_r.trim().replace('\n', " ")),
                &rec,
                &twin,
            ));
        }
    }
    for n in &notes {
        println!("NOTE {n}");
    }
    for d in &diffs {
        println!("{d}");
    }
    assert!(
        compared >= 30,
        "only {compared} swallowed statements were compared; the cases no longer reach the recovery"
    );
    assert!(
        diffs.is_empty(),
        "{} differences between recovered and control twin",
        diffs.len()
    );
}
