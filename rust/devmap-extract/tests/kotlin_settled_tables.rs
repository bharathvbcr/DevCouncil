//! The tables of a Kotlin file with a charged statement (extract-v98) equal the
//! tables of the same file without it, outside the statement.
//!
//! A statement the loss detector charged and the recovery did not prove is
//! replaced by nothing; one it recovered is replaced by what its re-parse says.
//! Either way a table derived from names (wiring, exports, scope locals, scope
//! receivers) must come out as it would in a file with no such statement, not
//! patched by a name that a phantom shares with a real symbol. The control
//! method: a file with the statement is compared with the same file with that
//! statement deleted (or, for a recovered statement, with its one-annotation twin
//! that the grammar reads correctly), over each table as a sorted multiset of
//! span-free rows.
//!
//! The `break_` tests were written by an adversarial pass at `833cc524` and fail
//! there. Ported from `attack2/v98-kotlin` @ `1df8c5cb`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use devmap_extract::extract_file_with_budget;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

/// Far above the default: a budget refusal is not what these tests are about.
const BUDGET: Duration = Duration::from_secs(600);

fn extract(path: &str, source: &str) -> Extraction {
    let extraction = extract_file_with_budget(path, source, BUDGET);
    assert!(
        !matches!(extraction.parse_outcome, ParseOutcome::Failed { .. }),
        "budget refusal, re-run with more budget: {:?}",
        extraction.parse_outcome
    );
    extraction
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

/// Fields that legitimately differ between a file and its control.
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

/// Each top-level field of an extraction as a sorted multiset of span-free JSON.
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

/// What the file says that its control does not (`+`, a phantom) and what the
/// control says that the file lost (`-`), per field, for `fields` (all when
/// empty).
fn diff_in(full: &Extraction, control: &Extraction, fields: &[&str]) -> Vec<String> {
    let (nf, nc) = (normalized(full), normalized(control));
    let keys: BTreeSet<&String> = nf.keys().chain(nc.keys()).collect();
    let mut out = Vec::new();
    for key in keys {
        if !fields.is_empty() && !fields.contains(&key.as_str()) {
            continue;
        }
        let a = nf.get(key).cloned().unwrap_or_default();
        let b = nc.get(key).cloned().unwrap_or_default();
        if a == b {
            continue;
        }
        let mut extra = a.clone();
        for row in &b {
            if let Some(at) = extra.iter().position(|x| x == row) {
                extra.remove(at);
            }
        }
        let mut missing = b.clone();
        for row in &a {
            if let Some(at) = missing.iter().position(|x| x == row) {
                missing.remove(at);
            }
        }
        out.push(format!(
            "field `{key}`:\n      + (file only, a phantom) {extra:?}\n      - (control only, lost) {missing:?}"
        ));
    }
    out
}

/// `prefix + swallowed + suffix` against `prefix + suffix`.
fn against_control(
    prefix: &str,
    swallowed: &str,
    suffix: &str,
    fields: &[&str],
) -> (Extraction, Vec<String>) {
    let full = extract("A.kt", &format!("{prefix}{swallowed}{suffix}"));
    let control = extract("A.kt", &format!("{prefix}{suffix}"));
    let diffs = diff_in(&full, &control, fields);
    (full, diffs)
}

/// A recovered statement against its single-annotation twin (the grammar reads
/// that one correctly). The annotation reference rows differ by construction.
fn against_twin(prefix: &str, stmt: &str, suffix: &str, fields: &[&str]) -> Vec<String> {
    let full = extract("A.kt", &format!("{prefix}@A\n@B\n{stmt}{suffix}"));
    let twin = extract("A.kt", &format!("{prefix}@A\n{stmt}{suffix}"));
    let strip_refs = |e: &Extraction| {
        let mut e = e.clone();
        e.references.retain(|r| r.name != "A" && r.name != "B");
        e
    };
    diff_in(&strip_refs(&full), &strip_refs(&twin), fields)
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

const TAIL: &str = "\nfun tail() { helper() }\nfun helper() {}\n";
const AFTER: &str = "fun after(a: Int) {\n    val loc = 1\n    val d = helper()\n}\n";

/// A swallowed one-line class whose body holds a member. The recovery cannot
/// prove it (the wrapper reads the one-line body as an expression), so it stays
/// charged. The expression reading files `m` as a top-level function.
const U_CLASS: &str = "@A\n@B\nprivate class Q { fun m(a: Int) { val loc = helper() } }";

/// Shapes the recovery cannot prove, as (label, swallowed text).
fn unrecoverable() -> Vec<(&'static str, &'static str)> {
    vec![
        ("one-line-class-member", U_CLASS),
        ("one-line-interface", "@A\n@B\nprivate interface I { fun m(): Int }"),
        ("nested-one-liner", "@A\n@B\nprivate class Q { class In { fun d() {} } }"),
        ("one-line-object-lambda", "@A\n@B\nprivate object O { val x = run { helper() } }"),
        (
            "lambda-with-calls",
            "@A\n@B\nprivate class Q { val x = listOf(1).map { helper(it) }\n fun m() { Foo().apply { bar() } } }",
        ),
        (
            "inner-swallowed-in-fun",
            "@C\n@D\nprivate fun f() {\n@E\n@F\nprivate fun g() {}\nlast()\n}",
        ),
    ]
}

/// Real code before the region, chosen to collide with the phantoms' names or
/// to sit right against the region.
fn prefixes() -> Vec<(&'static str, &'static str)> {
    vec![
        ("empty", ""),
        ("prop", "val before = 0\n"),
        ("real-class-m", "class Real {\n    fun m() {}\n}\n"),
        (
            "overloads",
            "fun m(a: Int, c: Int) = a\nfun m(d: Long) {}\n",
        ),
        ("expr-fun", "fun a() = foo(1)\n"),
        ("no-body-fun", "fun a()\n"),
        ("no-body-val", "val p: Int\n"),
        ("bare-class", "class C\n"),
        ("getter", "var g: Int = 0\n    get() = 1\n"),
        ("comment", "// note\n/* block */\n"),
        ("kdoc", "/** doc */\n"),
        ("import", "import x.y\n"),
    ]
}

/// BREAK. `Staged::apply` removes `wiring` where `target_symbol` is the
/// qualified name of a dropped phantom, with no exception for a surviving real
/// symbol of the same name (the `scope_locals` branch has one). The member `m`
/// of a swallowed one-line class is filed as a top-level `A.kt::m`; a real
/// top-level `@Preview fun m()` loses its `RuntimeEntryPoint` and reads as dead.
#[test]
fn break_attack1_wiring_of_a_real_symbol_is_removed_with_a_same_named_phantom() {
    let mut failures = Vec::new();
    for (label, prefix, suffix) in [
        ("real m before", "@Preview\nfun m() {}\n", TAIL),
        (
            "real m after",
            "",
            "\n@Preview\nfun m() {}\nfun tail() {}\n",
        ),
    ] {
        let (_, diffs) = against_control(prefix, U_CLASS, suffix, &["wiring"]);
        failures.extend(diffs.into_iter().map(|d| format!("{label}: {d}")));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK. The same, for a region that IS recovered: the outer tree's phantom
/// `m` (the member of the swallowed class) is in `dropped`, so the real
/// top-level `@Test fun m()`'s wiring goes with it. The control is the
/// single-annotation twin.
#[test]
fn break_attack1_wiring_of_a_real_symbol_is_removed_by_a_recovered_regions_phantom() {
    let stmt = "private class Q {\n    fun m() { helper() }\n}";
    let mut failures = Vec::new();
    for annotation in ["Preview", "Test"] {
        let prefix = format!("@{annotation}\nfun m() {{}}\n");
        let diffs = against_twin(&prefix, stmt, TAIL, &["wiring"]);
        failures.extend(diffs.into_iter().map(|d| format!("@{annotation}: {d}")));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK. `exports` is built once per exported symbol *name*, from `symbols` in
/// document order, so when the phantom `m` precedes a real `fun m` the phantom
/// takes the one export slot; the region's removal then deletes it by span and
/// the real `m` is left with no export. The control publishes `m`.
#[test]
fn break_attack1_export_of_a_real_symbol_declared_after_the_phantom_is_lost() {
    let suffix = "\nfun m(b: Int) { val z = helper() }\nfun tail() {}\nfun helper() {}\n";
    let (full, diffs) = against_control("fun a() {}\n", U_CLASS, suffix, &["exports"]);
    assert!(
        diffs.is_empty()
            && full
                .exports
                .iter()
                .any(|export| export.exported_name == "m"),
        "the real top-level `m` is a symbol ({:?}) but is not exported:\n{}",
        names(&full),
        diffs.join("\n")
    );
}

/// BREAK. `scope_locals` of the phantom member are kept whenever a real symbol
/// has the phantom's qualified name (`surviving` is checked by exact name, and
/// the real one's rows cannot be told from the phantom's): the real
/// `m(b: Int) { val z }` gets `a` and `loc`, which belong to a function that is
/// not in the file. A bare call `loc()` inside the real `m` then reads as a call
/// to a local value instead of a possible defect.
#[test]
fn break_attack2_phantom_locals_survive_under_a_real_symbols_scope() {
    let mut failures = Vec::new();
    for (label, prefix, suffix) in [
        (
            "real m before",
            "fun m(b: Int) {\n    val z = helper()\n}\n",
            TAIL.to_string(),
        ),
        (
            "real m after",
            "",
            format!("{TAIL}fun m(b: Int) {{\n    val z = helper()\n}}\n"),
        ),
        ("overload", "fun m(a: Int, c: Int) = a\n", TAIL.to_string()),
    ] {
        let (_, diffs) = against_control(prefix, U_CLASS, &suffix, &["scope_locals"]);
        failures.extend(diffs.into_iter().map(|d| format!("{label}: {d}")));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK. A region nested in a function body is not a symbol-bearing statement:
/// the phantom local function `g` of `class Q { fun g(a: Int) .. }` is not in
/// `symbols`, so there is no dropped symbol and no ghost. Its scope
/// `A.kt::g.g` (with `a` and `loc`) is published for a function that does not
/// exist, and the trailing lambda of the swallowed `h() { .. }` gives the REAL
/// enclosing `A.kt::g` the locals `$0`, `$1`, `$2` and `it`.
#[test]
fn break_attack2_nested_region_leaves_phantom_scopes_and_locals() {
    let prefix = "fun g() {\n    val keep = 1\n    before()\n";
    let suffix = "\n    after()\n}\nfun tail() {}\n";
    let mut failures = Vec::new();
    for (label, shape) in [
        (
            "class member named like the enclosing function",
            "@A\n@B\nprivate class Q { fun g(a: Int) { val loc = helper() } }",
        ),
        (
            "fun with a trailing lambda",
            "@A\n@B\nprivate fun h() { inner() }",
        ),
    ] {
        let (_, diffs) = against_control(prefix, shape, suffix, &["scope_locals"]);
        failures.extend(diffs.into_iter().map(|d| format!("{label}: {d}")));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// BREAK. `scope_receivers` (the implicit `this` of a lambda body) is not one of
/// the lists `Staged` settles: `with(Foo()) { bar() }` inside an unrecovered
/// region leaves a receiver row for a lambda that is no longer published.
#[test]
fn break_attack2_phantom_scope_receiver_survives() {
    let shape = "@A\n@B\nprivate class Q { fun m() { with(Foo()) { bar() } } }";
    let (_, diffs) = against_control("val before = 1\n", shape, TAIL, &["scope_receivers"]);
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}

/// LOW BREAK. `Staged` appends a recovered region's exports without the
/// by-name de-duplication the ordinary pass applies, so a recovered member `m`
/// beside a real top-level `m` publishes `m` twice where the twin publishes it
/// once.
#[test]
fn break_attack2_recovery_publishes_an_export_twice() {
    let stmt = "private class Q {\n    fun m() { helper() }\n}";
    let diffs = against_twin("fun m(b: Int) {}\n", stmt, TAIL, &["exports"]);
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}

/// Every shape the recovery cannot prove, against every prefix, equals its
/// control on EVERY table: symbols, calls, references, imports, exports, wiring,
/// scope locals, scope receivers, local bindings and the JVM side tables.
#[test]
fn every_unrecovered_shape_equals_its_control_on_every_table() {
    let suffix = format!("{TAIL}{AFTER}");
    let mut failures = Vec::new();
    for (shape_label, shape) in unrecoverable() {
        for (prefix_label, prefix) in prefixes() {
            let (_, diffs) = against_control(prefix, shape, &suffix, &[]);
            for diff in diffs {
                failures.push(format!("{shape_label} / {prefix_label}: {diff}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A recovered statement equals its one-annotation twin on every table, with a
/// real symbol of the same name before and after it.
#[test]
fn a_recovered_statement_equals_its_twin_on_every_table() {
    let statements = [
        "private class Q {\n    fun m() { helper() }\n}",
        "private fun m() { with(Foo()) { bar() } }",
        "private object O {\n    fun m(a: Int) { val loc = helper() }\n}",
    ];
    let mut failures = Vec::new();
    for stmt in statements {
        for (prefix_label, prefix) in [
            ("real m before", "@Preview\nfun m() {}\n"),
            (
                "real m with a local",
                "fun m(b: Int) { val z = helper() }\n",
            ),
            ("nothing", ""),
        ] {
            let suffix = format!("{TAIL}fun m(c: Int) {{ val y = helper() }}\n");
            let diffs = against_twin(prefix, stmt, &suffix, &[]);
            failures.extend(
                diffs
                    .into_iter()
                    .map(|d| format!("{prefix_label} / {stmt}: {d}")),
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
