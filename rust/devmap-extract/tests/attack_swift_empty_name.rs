//! Adversarial probes for `lane/swift-empty-name` @ f26e1e97 (main 115266fc,
//! card ft-dbbc8c3c). Each test states the CORRECT behaviour; a failing test
//! is a BREAK. A test marked `#[ignore = "pre-existing on main: ..."]` fails
//! on main 115266fc as well, so the lane did not introduce it.
//!
//! Contract under test: the file stays Clean; no call or reference has an
//! empty name in any language; the bare and parenthesised spellings of one
//! expression yield the same facts, or the extractor abstains; a wrong edge is
//! worse than an abstention; the refusal diagnostic is one line per file, is
//! exact, and never leaks into another file.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_extract::{extract_all, extract_file, extract_file_with_budget, FileRef};

const BUDGET: Duration = Duration::from_secs(600);

fn ex(path: &str, source: &str) -> Extraction {
    extract_file_with_budget(path, source, BUDGET)
}

fn clean(e: &Extraction) -> bool {
    matches!(e.parse_outcome, ParseOutcome::Clean)
}

fn empty_names(e: &Extraction) -> (usize, usize) {
    (
        e.calls
            .iter()
            .filter(|c| c.callee_name.trim().is_empty())
            .count(),
        e.references
            .iter()
            .filter(|r| r.name.trim().is_empty())
            .count(),
    )
}

fn refusal_notes(e: &Extraction) -> Vec<&str> {
    e.diagnostics
        .iter()
        .map(String::as_str)
        .filter(|d| d.starts_with("swift: refused"))
        .collect()
}

fn refusal_note(n: usize) -> String {
    format!("swift: refused {n} call(s) on an operator the grammar inserted with no spelling")
}

// ------------------------------------------------------------ table diffing

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
    "raw_import",
];

const IGNORED_FIELDS: &[&str] = &[
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
            "`{key}`: only bare {only_x:?}; only twin {only_y:?}"
        ));
    }
    out
}

// ------------------------------------------------------------ twin judging

/// What a sweep found, so a failing run reports coverage beside its failures.
#[derive(Default)]
struct Tally {
    checked: usize,
    bare_clean: usize,
    bare_partial: usize,
    refused_sites: usize,
    failures: Vec<String>,
}

impl Tally {
    /// One bare source against its parenthesised twin under the lane's contract.
    fn judge(&mut self, label: &str, bare: &str, twin: &str) {
        let (e, t) = (ex("a.swift", bare), ex("a.swift", twin));
        self.checked += 1;
        let (c, r) = empty_names(&e);
        if c != 0 || r != 0 {
            self.failures.push(format!(
                "{label} {bare:?}: {c} empty call(s), {r} empty reference(s)"
            ));
        }
        let notes = refusal_notes(&e);
        if notes.len() > 1 {
            self.failures
                .push(format!("{label} {bare:?}: {} refusal lines", notes.len()));
        }
        if !notes.is_empty() {
            self.refused_sites += 1;
        }
        if !refusal_notes(&t).is_empty() {
            self.failures.push(format!(
                "{label} {twin:?}: the parenthesised twin was refused: {:?}",
                refusal_notes(&t)
            ));
        }
        if clean(&e) {
            self.bare_clean += 1;
            let diffs = table_diffs(&e, &t);
            if !diffs.is_empty() {
                self.failures.push(format!(
                    "{label} {bare:?} is Clean but differs from its twin {twin:?}: {}",
                    diffs.join("; ")
                ));
            }
        } else {
            self.bare_partial += 1;
        }
    }

    fn finish(self) {
        assert!(
            self.failures.is_empty(),
            "checked {} (bare Clean {}, bare Partial {}, refused {}); {} failure(s):\n{}",
            self.checked,
            self.bare_clean,
            self.bare_partial,
            self.refused_sites,
            self.failures.len(),
            self.failures.join("\n")
        );
        assert!(
            self.bare_clean > 0 && self.refused_sites > 0,
            "the sweep never reached a Clean refused site (checked {}); it proves nothing",
            self.checked
        );
    }
}

/// `{cast}` between a left side and a type, then a coalesce into `rhs`.
fn bare_and_twin(lhs: &str, cast: &str, ty: &str, rhs: &str) -> (String, String) {
    (
        format!("let a = {lhs} {cast} {ty} ?? {rhs}\n"),
        format!("let a = ({lhs} {cast} {ty}) ?? {rhs}\n"),
    )
}

const LHS_PLAIN: &[&str] = &["b", "b?.c", "b!.c", "self.b", "d[\"k\"]"];

const CASTS: &[&str] = &["as?", "as!"];

const TYPES_PLAIN: &[&str] = &[
    "String",
    "Int",
    "[String]",
    "[String: Any]",
    "(Int, Int)",
    "Array<String>",
    "Dictionary<String, Int>",
    "String?",
    "[Int]?",
    "any P",
    "(Int) -> Int",
    "Foo<Bar>",
];

const RHS_PLAIN: &[&str] = &[
    "\"x\"",
    "0",
    "nil",
    "[]",
    "[:]",
    "d",
    "d.e",
    "d ?? e",
    "\"x\".uppercased()",
    "g()",
    "Foo()",
    "x.y()",
    "0 + h()",
    "{ h() }()",
    "self.h()",
    "max(1, 2)",
];

// ------------------------------------------------------ 1. spelling variants

/// ATTACK 1a. Every plain left side, cast, type and right side, bare against
/// parenthesised. 1,920 pairs; the right sides include the call shapes the
/// brief names as Partial (`?? f()`, `?? Foo()`, `?? x.y()`, `?? 0 + h()`).
#[test]
fn every_plain_bare_cast_equals_its_twin_or_abstains() {
    let mut tally = Tally::default();
    for lhs in LHS_PLAIN {
        for cast in CASTS {
            for ty in TYPES_PLAIN {
                for rhs in RHS_PLAIN {
                    let (bare, twin) = bare_and_twin(lhs, cast, ty, rhs);
                    tally.judge("matrix", &bare, &twin);
                }
            }
        }
    }
    tally.finish();
}

/// ATTACK 1b. `try?` / `try!` with a coalesce, plus nested `??`, multiple
/// sites in one expression and optional chaining before the cast.
#[test]
fn try_nested_and_chained_spellings_equal_their_twins() {
    let pairs: &[(&str, &str)] = &[
        ("let a = try? g() ?? \"x\"", "let a = (try? g()) ?? \"x\""),
        ("let a = try! g() ?? \"x\"", "let a = (try! g()) ?? \"x\""),
        (
            "let a = b?.c as? String ?? \"x\"",
            "let a = (b?.c as? String) ?? \"x\"",
        ),
        (
            "let a = b as? String ?? \"x\" ?? \"y\"",
            "let a = (b as? String) ?? \"x\" ?? \"y\"",
        ),
        (
            "let a = b as? String ?? \"x\" == \"y\"",
            "let a = ((b as? String) ?? \"x\") == \"y\"",
        ),
        (
            "let a = b as? String ?? \"x\" + \"y\" + z",
            "let a = ((b as? String) ?? \"x\") + \"y\" + z",
        ),
        (
            "let a = b as? String ?? \"x\" ? 1 : 2",
            "let a = ((b as? String) ?? \"x\") ? 1 : 2",
        ),
        (
            "let a = b is String ?? \"x\"",
            "let a = (b is String) ?? \"x\"",
        ),
        (
            "let a = b as? String? ?? \"x\"",
            "let a = (b as? String?) ?? \"x\"",
        ),
        (
            "let a = x as?  String  ?? \"x\"",
            "let a = (x as?  String)  ?? \"x\"",
        ),
    ];
    let mut tally = Tally::default();
    for (bare, twin) in pairs {
        tally.judge("variant", &format!("{bare}\n"), &format!("{twin}\n"));
    }
    tally.finish();
}

/// ATTACK 1c. A cast to a qualified type (`Foo.Bar`, `String.Type`) is read by
/// the grammar as a member access on the cast expression in some contexts
/// (`(b as? Foo.Bar)`, and the coalesced form with a `nil` or `[]` right side
/// in several shapes), which publishes a Name reference `Bar` whose receiver is
/// the text `b as? Foo`. A type is not a member of a cast expression. This
/// happens without any coalesce (`(b as? Foo.Bar)`), so it is not the lane's,
/// and the file is Clean.
#[test]
#[ignore = "pre-existing on main: a cast to a qualified type can publish a Name reference whose receiver is the cast expression text"]
fn a_qualified_type_cast_never_names_a_member_of_the_cast_expression() {
    let mut failures = Vec::new();
    for lhs in LHS_PLAIN {
        for cast in CASTS {
            for ty in ["Foo.Bar", "String.Type", "Foo.Bar.Baz", "Foo.Bar?"] {
                for source in [
                    format!("let a = {lhs} {cast} {ty} ?? nil\n"),
                    format!("let a = {lhs} {cast} {ty} ?? []\n"),
                    format!("let a = ({lhs} {cast} {ty})\n"),
                    format!("let a = ({lhs} {cast} {ty}) ?? nil\n"),
                ] {
                    let e = ex("a.swift", &source);
                    if !clean(&e) {
                        continue;
                    }
                    for reference in &e.references {
                        if reference
                            .receiver_expr
                            .as_deref()
                            .is_some_and(|r| r.contains("as?") || r.contains("as!"))
                        {
                            failures.push(format!(
                                "{source:?}: `{}` ({:?}) has the cast as receiver {:?}",
                                reference.name, reference.kind, reference.receiver_expr
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 1d. Every token boundary of the bare site split by a newline, a
/// block comment or a line comment, against the twin with the same spacing.
#[test]
fn a_split_or_commented_bare_site_equals_its_twin() {
    let seps = [" ", "\n    ", " /* c */ ", " // c\n    "];
    let mut tally = Tally::default();
    for cast in CASTS {
        for s1 in seps {
            for s2 in seps {
                for s3 in seps {
                    for s4 in seps {
                        let bare = format!("let a = b{s1}{cast}{s2}String{s3}??{s4}\"x\"\n");
                        let twin = format!("let a = (b{s1}{cast}{s2}String){s3}??{s4}\"x\"\n");
                        tally.judge("split", &bare, &twin);
                    }
                }
            }
        }
    }
    tally.finish();
}

/// ATTACK 1e. The bare site in every syntactic position the brief lists:
/// `if let` / `guard let`, default parameter values, property initialisers,
/// result builders, `#if` blocks, closures, string interpolation, arguments,
/// collection literals, ternary branches, switch arms, tasks, nested closures.
#[test]
fn a_bare_site_in_any_context_equals_its_twin() {
    let contexts: &[&str] = &[
        "let a = {}\n",
        "var a: String = {}\n",
        "func f() -> String { return {} }\n",
        "func f() -> String {\n    {}\n}\n",
        "let a = items.map { {} }\n",
        "let a = \"\\({})\"\n",
        "func f() { if let a = {} { print(a) } }\n",
        "func f() { guard let a = {} else { return }\n print(a) }\n",
        "func f(p: String = {}) {}\n",
        "struct S { var p: String = {} }\n",
        "struct S { var p = {} }\n",
        "struct S { var p: String { {} } }\n",
        "struct S { init(p: String = {}) {} }\n",
        "#if os(macOS)\nlet a = {}\n#endif\n",
        "#if os(macOS)\nlet a = 1\n#else\nlet a = {}\n#endif\n",
        "@resultBuilder struct RB { static func buildBlock(_ x: String...) -> String { \"\" } }\n@RB func f() -> String { {} }\n",
        "var body: some View { Text({}) }\n",
        "let a = foo({})\n",
        "let a = foo(label: {})\n",
        "let a = c ? {} : \"y\"\n",
        "func f() { switch x { case 1: let a = {}\n print(a)\n default: break } }\n",
        "func f() { Task { let a = {}\n print(a) } }\n",
        "let a = { {} }()\n",
        "func f() { x = {} }\n",
        "func f() { print({}, terminator: \"\") }\n",
        "func f() { defer { cleanup({}) } }\n",
        "extension S { func f() -> String { {} } }\n",
        "enum E { case a\n var s: String { {} } }\n",
        "func f() async throws -> String { try await g({}) }\n",
        "let a = [\"k\": {}]\n",
        "func f() { for x in xs { _ = {} } }\n",
        "func f() { let g = { (x: Int) -> String in {} } }\n",
    ];
    let exprs: &[(&str, &str)] = &[
        ("b as? String ?? \"x\"", "(b as? String) ?? \"x\""),
        ("b as! Int ?? 0", "(b as! Int) ?? 0"),
        (
            "d[\"k\"] as? [String] ?? []",
            "(d[\"k\"] as? [String]) ?? []",
        ),
    ];
    let mut tally = Tally::default();
    for context in contexts {
        for (bare, twin) in exprs {
            tally.judge(
                "context",
                &context.replace("{}", bare),
                &context.replace("{}", twin),
            );
        }
    }
    tally.finish();
}

/// ATTACK 1f. After the coalesce, an implicit-member expression (`.foo()`,
/// `.red`, `.init()`). The grammar reads `String ??` as an optional type and
/// the member as a navigation off it, so the bare spelling publishes a call (or
/// a Name reference) on the receiver text `b as? String ??` — a fabricated
/// edge of the SC9 class. The parenthesised twin, and the leading-dot rule in
/// `swift_unwrap_prefix`, abstain. The file is Clean, so nothing says so.
#[test]
#[ignore = "pre-existing on main: an implicit member after `x as? T ??` publishes a call on the receiver text `x as? T ??`; the empty-name refusal does not cover it"]
fn an_implicit_member_after_the_coalesce_is_not_given_the_cast_as_receiver() {
    let mut failures = Vec::new();
    for rhs in [
        ".foo()",
        ".custom(1)",
        ".red",
        ".init()",
        ".zero",
        ".some(x)",
    ] {
        let (bare, twin) = bare_and_twin("b", "as?", "Color", rhs);
        let (e, t) = (ex("a.swift", &bare), ex("a.swift", &twin));
        for call in &e.calls {
            if call
                .receiver_expr
                .as_deref()
                .is_some_and(|r| r.contains("??") || r.contains("as?"))
            {
                failures.push(format!(
                    "{bare:?}: call `{}` has operator text as receiver {:?}",
                    call.callee_name, call.receiver_expr
                ));
            }
        }
        for reference in &e.references {
            if reference
                .receiver_expr
                .as_deref()
                .is_some_and(|r| r.contains("??") || r.contains("as?"))
            {
                failures.push(format!(
                    "{bare:?}: reference `{}` has operator text as receiver {:?}",
                    reference.name, reference.receiver_expr
                ));
            }
        }
        if clean(&e) {
            let diffs = table_diffs(&e, &t);
            if !diffs.is_empty() {
                failures.push(format!("{bare:?} vs twin: {}", diffs.join("; ")));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 1g. A parenthesised call after the coalesce (`?? (h())`): the bare
/// spelling drops the call's `assigned_to`, the twin keeps it, and both files
/// are Clean. The facts differ in one field of one reference.
#[test]
#[ignore = "pre-existing on main: `x as? T ?? (h())` drops the call reference's assigned_to that the twin carries"]
fn a_parenthesised_call_after_the_coalesce_equals_its_twin() {
    let mut tally = Tally::default();
    for cast in CASTS {
        for ty in ["String", "Int", "[String]"] {
            let (bare, twin) = bare_and_twin("b", cast, ty, "(h())");
            tally.judge("paren-call", &bare, &twin);
        }
    }
    tally.finish();
}

/// ATTACK 1h. A call on the left of the cast: the 4-ancestor walk of
/// `swift_assignment_binding` reaches the `let` for the bare spelling and not
/// for the parenthesised one (the extra tuple node), so `a` is bound to the
/// call's type in one spelling and not the other. Neither is right (`a` is a
/// `String`), and the file is Clean in both.
#[test]
#[ignore = "pre-existing on main: assigned_to binds a cast-and-coalesced local to the call on its left in one spelling only"]
fn a_call_on_the_left_of_the_cast_binds_the_same_in_both_spellings() {
    let mut tally = Tally::default();
    for lhs in ["f()", "x.f()", "f(a)?.g()", "await g()", "Foo()"] {
        for cast in CASTS {
            let (bare, twin) = bare_and_twin(lhs, cast, "String", "\"x\"");
            tally.judge("call-lhs", &bare, &twin);
        }
    }
    tally.finish();
}

// ------------------------------------------- 2. does the gate drop a real call

/// ATTACK 2a. Spelled operators, prefix/postfix forms, backtick names and
/// implicit members are all things the gate sees and must not refuse. A
/// refusal note on any of these is a real call or operator lost. `required`
/// names were present on main 115266fc in the same probe.
#[test]
fn the_gate_refuses_no_spelled_operator_call() {
    let cases: &[(&str, &[&str])] = &[
        ("let a = x <*> y\n", &["<*>"]),
        ("let a = x + y\n", &["+"]),
        ("let a = x == y\n", &["=="]),
        ("let a = x === y\n", &["==="]),
        ("let a = x ~= y\n", &["~="]),
        ("let a = x &+ y\n", &["&+"]),
        ("let a = x |> y\n", &["|>"]),
        ("let a = x <> y\n", &["<>"]),
        ("let a = x \u{2218} y\n", &["\u{2218}"]),
        ("let a = x \u{2248} y\n", &["\u{2248}"]),
        (
            "infix operator <*>\nfunc <*> (l: Int, r: Int) -> Int { l }\nlet a = 1 <*> 2\n",
            &["<*>"],
        ),
        (
            "struct V { static func + (l: V, r: V) -> V { l } }\nfunc f(a: V, b: V) -> V { a + b }\n",
            &["+"],
        ),
        ("let a = x - y\n", &[]),
        ("let a = x * y\n", &[]),
        ("let a = x / y\n", &[]),
        ("let a = x < y\n", &[]),
        ("let a = x <= y\n", &[]),
        ("let a = x != y\n", &[]),
        ("let a = !x\n", &[]),
        ("let a = x!\n", &[]),
        ("let a = -x\n", &[]),
        ("let a = x ?? y\n", &[]),
        ("let a = x ... y\n", &[]),
        ("let a = x ..< y\n", &[]),
        ("prefix operator \u{2039}\nlet a = \u{2039}x\n", &[]),
        ("postfix operator \u{203a}\nlet a = x\u{203a}\n", &[]),
        ("let a = `default`()\n", &[]),
        ("let a = .init()\n", &[]),
        ("let a = foo(.bar())\n", &["foo"]),
        ("let a = Person.init(name: x)\n", &[]),
        ("let a = (try? f()) ?? g()\n", &["f", "g"]),
        ("let a = f() ?? g() ?? h()\n", &["f", "g", "h"]),
        ("let a = b as? String\n", &[]),
        ("let a = (b as? String) ?? \"x\"\n", &[]),
    ];
    let mut failures = Vec::new();
    for (source, required) in cases {
        let e = ex("a.swift", source);
        if !refusal_notes(&e).is_empty() {
            failures.push(format!("{source:?}: refused {:?}", refusal_notes(&e)));
        }
        if empty_names(&e) != (0, 0) {
            failures.push(format!("{source:?}: an empty name"));
        }
        let called: BTreeSet<&str> = e.calls.iter().map(|c| c.callee_name.as_str()).collect();
        for name in *required {
            if !called.contains(name) {
                failures.push(format!("{source:?}: lost the call `{name}` ({called:?})"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 2b. No checked-in Swift fixture holds a bare site, so the gate must
/// refuse nothing there: a note on a fixture is a legitimate call dropped.
#[test]
fn the_gate_refuses_nothing_in_a_checked_in_swift_fixture() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "swift") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata");
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(
        files.len() >= 30,
        "expected the Swift fixtures, found {}",
        files.len()
    );
    let mut failures = Vec::new();
    for path in &files {
        let source = std::fs::read_to_string(path).expect("read");
        let rel = path.strip_prefix(&root).unwrap().to_string_lossy();
        let e = ex(&rel, &source);
        if !refusal_notes(&e).is_empty() {
            failures.push(format!("{rel}: refused {:?}", refusal_notes(&e)));
        }
    }
    assert!(
        failures.is_empty(),
        "scanned {} Swift fixtures:\n{}",
        files.len(),
        failures.join("\n")
    );
}

// ------------------------------------------------------ 3. diagnostic integrity

/// One bare statement with its own name, so a file of `k` of them has `k` sites.
fn site(i: usize) -> String {
    format!("let v{i} = b{i} as? String ?? \"x\"\n")
}

/// The same site wrapped in its own function, which keeps a file of many of
/// them Clean (three or more adjacent top-level statements do not).
fn fsite(i: usize) -> String {
    format!("func g{i}() {{ let v = b{i} as? String ?? \"x\" }}\n")
}

/// ATTACK 3a. The refusal counter is a thread-local, and `extract_all` runs
/// files on rayon workers that each parse many files. 120 files (more than any
/// pool has threads), a mix of Swift files with 0..=2 sites and other
/// languages, extracted through the real multi-file entry point: every file's
/// note must carry exactly its own count, and no other file may carry one.
#[test]
fn the_refusal_count_is_per_file_under_rayon() {
    let mut sources: Vec<(String, String, usize)> = Vec::new();
    for i in 0..120usize {
        match i % 4 {
            0 | 1 => {
                // Layouts that stay Clean with k sites; three or more adjacent
                // statements do not (see `adjacent_bare_statements_keep_the_file_clean`).
                let k = (i / 2) % 3;
                let body = match k {
                    0 => format!("let top{i} = 1\n"),
                    1 => format!("let v{i} = b as? String ?? \"x\"\n"),
                    _ => format!(
                        "let v{i} = b as? String ?? \"x\"\nlet w{i} = d as! String ?? \"y\"\n"
                    ),
                };
                sources.push((format!("f{i}.swift"), body, k));
            }
            2 => sources.push((
                format!("f{i}.kt"),
                format!("fun g{i}() = b as? String ?: \"x\"\n"),
                0,
            )),
            _ => sources.push((
                format!("f{i}.py"),
                format!("def g{i}():\n    return b or 'x'\n"),
                0,
            )),
        }
    }
    let refs: Vec<FileRef> = sources
        .iter()
        .map(|(path, source, _)| FileRef { path, source })
        .collect();
    let first = extract_all(&refs);
    let second = extract_all(&refs);
    assert_eq!(first.len(), sources.len());
    let mut failures = Vec::new();
    let mut swift_with_sites = 0;
    for ((path, _, expected), e) in sources.iter().zip(&first) {
        assert_eq!(&e.file_path, path, "extract_all reordered its output");
        let notes = refusal_notes(e);
        let want: Vec<String> = if *expected == 0 {
            Vec::new()
        } else {
            swift_with_sites += 1;
            vec![refusal_note(*expected)]
        };
        if notes != want.iter().map(String::as_str).collect::<Vec<_>>() {
            failures.push(format!("{path}: notes {notes:?}, wanted {want:?}"));
        }
        if empty_names(e) != (0, 0) {
            failures.push(format!("{path}: an empty name"));
        }
        if *expected > 0 && !clean(e) {
            failures.push(format!("{path}: not Clean ({:?})", e.parse_outcome));
        }
    }
    // A worker that carried a count over from an earlier file would differ
    // from the same file extracted alone on this thread.
    for ((path, source, _), e) in sources.iter().zip(&first) {
        let alone = serde_json::to_string(&ex(path, source)).expect("serialize");
        if alone != serde_json::to_string(e).expect("serialize") {
            failures.push(format!(
                "{path}: the parallel result differs from a lone run"
            ));
        }
    }
    for (a, b) in first.iter().zip(&second) {
        let (ja, jb) = (
            serde_json::to_string(a).expect("serialize"),
            serde_json::to_string(b).expect("serialize"),
        );
        if ja != jb {
            failures.push(format!("{}: two runs differ", a.file_path));
        }
    }
    assert!(swift_with_sites > 0, "the batch held no site");
    assert!(
        failures.is_empty(),
        "{} files, {swift_with_sites} with sites:\n{}",
        first.len(),
        failures.join("\n")
    );
}

/// ATTACK 3a'. The ruling is that the file stays Clean. Three or more
/// adjacent top-level bare statements make the grammar recover differently and
/// the file is Partial on main and on the lane alike; the diagnostic then
/// counts only the sites the walk reached (2 of 3), because the rest sit inside
/// the error region. Lone and non-adjacent sites stay Clean (probed above).
#[test]
#[ignore = "pre-existing on main: three adjacent top-level bare statements turn the file Partial (grammar recovery)"]
fn adjacent_bare_statements_keep_the_file_clean() {
    let mut failures = Vec::new();
    for n in [2usize, 3, 4, 7] {
        let source: String = (0..n).map(site).collect();
        let e = ex("a.swift", &source);
        if !clean(&e) {
            failures.push(format!("{n} adjacent sites: {:?}", e.parse_outcome));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3b. The note counts what the walk refused. Sites the grammar puts
/// inside an error region are never walked, so a Partial file may count fewer
/// than its sites alone do, but never more (no double visit by a second walk),
/// and a Clean file must count exactly the sum of its sites alone.
#[test]
fn the_refusal_count_never_exceeds_the_sites_and_is_exact_when_clean() {
    let mut failures = Vec::new();
    let mut clean_files = 0;
    for n in [1usize, 2, 3, 7, 25, 100] {
        for (label, wrap) in [("top-level", site as fn(usize) -> String), ("func", fsite)] {
            let alone: usize = (0..n)
                .map(|i| refusal_notes(&ex("a.swift", &wrap(i))).len())
                .sum();
            let source: String = (0..n).map(wrap).collect();
            let e = ex("a.swift", &source);
            let notes = refusal_notes(&e);
            let counted = match notes.as_slice() {
                [] => 0,
                [one] => (1..=alone.max(n) + 1)
                    .find(|k| *one == refusal_note(*k))
                    .unwrap_or(usize::MAX),
                many => {
                    failures.push(format!("{n} {label} sites: {} note lines", many.len()));
                    continue;
                }
            };
            if counted > alone {
                failures.push(format!(
                    "{n} {label} sites: counted {counted}, sites alone sum to {alone}"
                ));
            }
            if clean(&e) {
                clean_files += 1;
                if counted != alone {
                    failures.push(format!(
                        "{n} {label} sites: Clean but counted {counted} of {alone}"
                    ));
                }
            }
        }
    }
    assert!(clean_files > 0, "no file in the sweep was Clean");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3c. A budget refusal replaces the extraction: the refusal must carry
/// no count, a real (Clean or Partial) result under a tight budget must carry
/// one line, and whatever a refused file counted must not reach the next file
/// on the thread.
#[test]
fn a_budget_refusal_leaves_no_note_and_no_residue() {
    let many: String = (0..400).map(site).collect();
    let mut failures = Vec::new();
    for budget in [
        Duration::ZERO,
        Duration::from_micros(200),
        Duration::from_millis(1),
        Duration::from_millis(3),
        Duration::from_millis(10),
        Duration::from_millis(30),
        Duration::from_millis(100),
    ] {
        let e = extract_file_with_budget("big.swift", &many, budget);
        let notes = refusal_notes(&e);
        match &e.parse_outcome {
            ParseOutcome::Clean | ParseOutcome::Partial { .. } => {
                // A real extraction: the note counts what its walk reached,
                // so it is one line and never more than the sites there are.
                let counted = (1..=400)
                    .find(|k| notes == [refusal_note(*k).as_str()])
                    .unwrap_or(0);
                if notes.len() > 1 || (notes.len() == 1 && counted == 0) {
                    failures.push(format!("budget {budget:?}: notes {notes:?}"));
                }
            }
            other => {
                if !notes.is_empty() {
                    failures.push(format!(
                        "budget {budget:?}: a refusal ({other:?}) carries notes {notes:?}"
                    ));
                }
            }
        }
        if empty_names(&e) != (0, 0) {
            failures.push(format!("budget {budget:?}: an empty name"));
        }
        // The next file on this thread starts from nothing.
        let next = ex("next.swift", &site(0));
        if refusal_notes(&next) != [refusal_note(1).as_str()] {
            failures.push(format!(
                "after budget {budget:?}: next file notes {:?}",
                refusal_notes(&next)
            ));
        }
        let none = ex("none.swift", "let a = (b as? String) ?? \"x\"\n");
        if !none.diagnostics.is_empty() {
            failures.push(format!(
                "after budget {budget:?}: a file without a site carries {:?}",
                none.diagnostics
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3d. A Swift file whose result is replaced (`refused_extraction` for
/// a NUL byte, an oversize source, a minified path) must not leave its count
/// for the next file.
#[test]
fn a_refused_swift_file_does_not_taint_the_next_one() {
    let nul = format!("{}\0\n", site(0));
    let e = ex("nul.swift", &nul);
    assert!(refusal_notes(&e).is_empty(), "{:?}", e.diagnostics);
    let next = ex("next.swift", "func f() {}\n");
    assert!(next.diagnostics.is_empty(), "{:?}", next.diagnostics);
    let again = ex("again.swift", &site(1));
    assert_eq!(refusal_notes(&again), [refusal_note(1).as_str()]);
}

/// ATTACK 3e. Embedded hosts run an inner extraction under a nested
/// `BudgetGuard::arm`, which zeroes the counter. No template host embeds Swift
/// (the registry lists only ts/js/css/html), so a `lang="swift"` script must
/// be left alone and must not corrupt the outer file's facts or the next
/// Swift file's note.
#[test]
fn a_swift_script_in_a_template_host_leaks_nothing() {
    let mut failures = Vec::new();
    let script = "<script lang=\"swift\">\nlet a = b as? String ?? \"x\"\n</script>\n";
    for (path, source) in [
        ("a.vue", format!("{script}<template><div/></template>\n")),
        ("a.svelte", format!("{script}<div/>\n")),
        ("a.astro", format!("---\n---\n{script}<div/>\n")),
        ("a.liquid", format!("{script}<div></div>\n")),
    ] {
        let e = ex(path, &source);
        if empty_names(&e) != (0, 0) {
            failures.push(format!("{path}: an empty name"));
        }
        if refusal_notes(&e).len() > 1 {
            failures.push(format!("{path}: {:?}", refusal_notes(&e)));
        }
        let next = ex("next.swift", &site(0));
        if refusal_notes(&next) != [refusal_note(1).as_str()] {
            failures.push(format!(
                "after {path}: next Swift file notes {:?}",
                refusal_notes(&next)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3f. A Kotlin file, which re-parses itself under a nested guard in its
/// loss recovery, between two Swift files: nothing crosses.
#[test]
fn a_kotlin_recovery_between_two_swift_files_leaks_nothing() {
    let kotlin =
        "class A { fun f() = if (c) { a() } else { b() } }\nval x = y as? String ?: \"d\"\n";
    let first = ex("a.swift", &site(0));
    let k = ex("k.kt", kotlin);
    let second = ex("b.swift", &(site(1) + &site(2)));
    assert_eq!(refusal_notes(&first), [refusal_note(1).as_str()]);
    assert!(refusal_notes(&k).is_empty(), "{:?}", k.diagnostics);
    assert_eq!(refusal_notes(&second), [refusal_note(2).as_str()]);
}

// -------------------------------------------------------- 5. cache, determinism

/// ATTACK 5a. Two runs of one file are byte-identical, diagnostic included.
#[test]
fn two_runs_of_a_bare_site_file_are_byte_identical() {
    let source = format!(
        "{}{}struct S {{ var p = q as? String ?? \"x\" }}\n",
        site(0),
        site(1)
    );
    let a = serde_json::to_string(&ex("a.swift", &source)).expect("serialize");
    let b = serde_json::to_string(&ex("a.swift", &source)).expect("serialize");
    assert_eq!(a, b);
}

/// ATTACK 5b. What the extraction cache and the durable store keep is the
/// extraction minus its diagnostics. Re-read from that payload, the refused
/// spelling must still equal its twin's re-read, with no empty name.
#[test]
fn a_durable_payload_of_the_bare_site_equals_its_twins() {
    for (bare, twin) in [
        (
            "let a = b as? String ?? \"x\"\n",
            "let a = (b as? String) ?? \"x\"\n",
        ),
        (
            "func f(r: [String: Any]) -> String {\n    return r[\"e\"] as! String ?? \"x\"\n}\n",
            "func f(r: [String: Any]) -> String {\n    return (r[\"e\"] as! String) ?? \"x\"\n}\n",
        ),
    ] {
        let reread = |source: &str| -> Extraction {
            let durable = ex("a.swift", source).for_durable_store();
            let json = serde_json::to_string(&durable).expect("serialize");
            serde_json::from_str(&json).expect("deserialize")
        };
        let (b, t) = (reread(bare), reread(twin));
        assert_eq!(empty_names(&b), (0, 0), "{bare:?}");
        assert_eq!(table_diffs(&b, &t), Vec::<String>::new(), "{bare:?}");
    }
}

/// ATTACK 5c (characterisation of an accepted ceiling, not a defect of the
/// lane). `for_durable_store` clears diagnostics, so a file re-read from the
/// cache or the store carries no trace that a call was refused. The brief
/// accepts this (model.rs:2288); the test pins it so a change is noticed.
#[test]
fn a_durable_reread_carries_no_refusal_note_by_design() {
    let fresh = ex("a.swift", &site(0));
    assert_eq!(refusal_notes(&fresh), [refusal_note(1).as_str()]);
    assert!(fresh.for_durable_store().diagnostics.is_empty());
}

// ------------------------------------------- 4. other languages, empty names

/// Hostile inputs per language: zero-width recovery tokens, operator
/// overloading, macros, immediately-invoked and curried calls, truncated
/// calls. Each is extracted and no call or reference may carry an empty name.
const HOSTILE: &[(&str, &str)] = &[
    ("a.kt", "val a = b as? String ?: \"x\"\n"),
    ("a.kt", "val a = b as? String ?: \n"),
    ("a.kt", "fun f() = x ?: return\n"),
    (
        "a.kt",
        "class A { operator fun invoke() = 1 }\nval x = A()()\n",
    ),
    (
        "a.kt",
        "infix fun Int.foo(x: Int) = 1\nval r = 1 foo 2\nval z = a to b\n",
    ),
    ("a.kt", "fun f() { g()() }\nval a = f(,)\nval b = (1)()\n"),
    ("a.kt", "val a = if (c) f() else\n"),
    (
        "a.java",
        "class A { void f() { this(); super(); new A() {}.g(); ((Runnable)() -> {}).run(); } }\n",
    ),
    (
        "a.java",
        "class A { void f() { x.<T>g(); Runnable r = A::new; } }\n",
    ),
    ("a.java", "class A { A() { this(; } }\n"),
    (
        "a.c",
        "#define F(x) x\nvoid g() { F(); (*fp)(); (void)0; }\n",
    ),
    ("a.c", "int main() { return (*f)(); }\nint x = f(;\n"),
    ("a.cpp", "template<class T> void f() { T()(); T{}(); }\n"),
    (
        "a.cpp",
        "struct S { int operator()(int) const; };\nvoid g(S s) { s(1); s.operator()(2); a.operator+(b); }\n",
    ),
    ("a.cpp", "void h() { ::(); operator()(); }\n"),
    (
        "a.cpp",
        "#define M(a,b) a##b\nvoid f() { M(,)(); M(x,)(); }\n",
    ),
    (
        "a.rs",
        "fn main() { foo!(); (|| 1)(); x.0(); <T as Tr>::f(); r#try(); a.b.c()(); }\n",
    ),
    (
        "a.rs",
        "macro_rules! m { () => {} }\nfn f() { m!{} ()(); x.await(); f::<>(); }\n",
    ),
    (
        "a.go",
        "package p\nfunc f() { g()(); (func(){})(); x.(T)(); a[0](); f[int](); }\n",
    ),
    (
        "a.go",
        "package p\nfunc f() { go func(){}(); defer (func(){})(); ()() }\n",
    ),
    (
        "a.py",
        "f()()\n(lambda: 1)()\na[0]()\nx @ y\n@dec()\ndef g(): pass\nprint(*a)()\n",
    ),
    (
        "a.py",
        "class A:\n    def __call__(self): pass\nA()()\nx = (  )()\n",
    ),
    (
        "a.js",
        "a?.()\n(function(){})()\nnew (foo())()\nimport(\"x\")\ntag`x`\nfoo?.[0]()\n(a, b)()\n(()=>1)()\n",
    ),
    (
        "a.js",
        "class A extends B { constructor() { super(); } }\nf(...)\nnew ()\n",
    ),
    (
        "a.ts",
        "a!()\nfoo<T>()\n(a as any)()\nx satisfies Y\nfoo<>()\nnew Foo<>()\n",
    ),
    (
        "a.tsx",
        "const a = <Foo<T> {...p} />;\nconst b = <></>;\nf(<div/>)()\n",
    ),
    (
        "a.php",
        "<?php\n$f();\n(new A)();\nA::{$x}();\n$a->{$b}();\n$o?->m();\n",
    ),
    (
        "a.rb",
        "foo.()\n->{}.()\nfoo&.bar\nyield()\nsuper()\nfoo::()\n",
    ),
    (
        "a.cs",
        "class A { void F() { x?.M(); new(); default(); (a)(); nameof(x); F<T>(); } }\n",
    ),
    (
        "a.scala",
        "object A { a b c\n f(x)(y)\n new A {}\n x.foo _ }\n",
    ),
    ("a.dart", "void f() { a?.b(); f()(); super(); this(); }\n"),
    ("a.lua", "f()()\n(\"x\"):rep()\n(f)()\n"),
    ("a.sh", "$(cmd)\n${x}\n$((1+1))\n`x`\n"),
    ("a.sql", "select f()() from t;\nselect () from t;\n"),
    ("a.nix", "f g h\n(a: a) 1\n{}.x\n"),
    (
        "a.erl",
        "-module(a).\nf() -> (fun() -> 1 end)(), F = x, F().\n",
    ),
    (
        "a.sol",
        "contract C { function f() public { a()(); this.g(); } }\n",
    ),
    ("a.r", "f()()\n(function() 1)()\n"),
    ("a.pas", "program p;\nbegin f(); end.\n"),
    (
        "a.m",
        "@implementation A\n- (void)f { [a b]; [[A alloc] init]; [a]; [] ; }\n@end\n",
    ),
    ("a.hcl", "resource \"a\" \"b\" { x = f()() }\n"),
    ("a.cu", "__global__ void k() { f<<<1, 1>>>(); }\n"),
    ("a.metal", "kernel void k() { f()(); }\n"),
    ("a.swift", "let a = b as? String ?? \"x\"\n"),
    ("a.swift", "let a = x +\n"),
    ("a.swift", "let a = x \u{200b} y\n"),
];

/// ATTACK 4a. The hostile table above, one extraction each, with coverage
/// stated: how many entries ran and in how many distinct languages.
#[test]
fn no_language_publishes_an_empty_name_on_a_hostile_input() {
    let mut failures = Vec::new();
    let mut languages = BTreeSet::new();
    for (path, source) in HOSTILE {
        let e = extract_file(path, source);
        languages.insert(e.language.clone());
        let (c, r) = empty_names(&e);
        if c != 0 || r != 0 {
            failures.push(format!(
                "{} {path} {source:?}: {c} empty call(s), {r} empty reference(s)",
                e.language
            ));
        }
    }
    assert!(
        languages.len() >= 20,
        "only {} languages were exercised: {languages:?}",
        languages.len()
    );
    assert!(
        failures.is_empty(),
        "{} inputs over {} languages:\n{}",
        HOSTILE.len(),
        languages.len(),
        failures.join("\n")
    );
}

/// ATTACK 4b. Deterministic damage to every checked-in fixture of at most 4 KB
/// (one character deleted at 12 points, 6 truncations, 8 deleted words): the
/// grammar's error recovery is what inserts zero-width tokens, and each
/// damaged file must still name every call and reference it publishes.
#[test]
fn no_language_publishes_an_empty_name_on_a_damaged_fixture() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut failures = Vec::new();
    let (mut variants, mut sources, mut languages) = (0usize, 0usize, BTreeSet::new());
    for path in files {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        if src.is_empty() || src.len() > 4000 {
            continue;
        }
        sources += 1;
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let at: Vec<usize> = src.char_indices().map(|(i, _)| i).collect();
        let n = at.len();
        let mut damaged: Vec<(String, String)> = Vec::new();
        for k in 1..=12 {
            let from = at[(n * k / 13).min(n - 1)];
            let width = src[from..].chars().next().map_or(0, char::len_utf8);
            let mut s = src.clone();
            s.replace_range(from..from + width, "");
            damaged.push((format!("delete@{from}"), s));
        }
        for k in 1..=6 {
            let cut = at[(n * k / 7).min(n - 1)];
            damaged.push((format!("truncate@{cut}"), src[..cut].to_string()));
        }
        let bytes = src.as_bytes();
        for k in 1..=8 {
            let mut from = at[(n * k / 9).min(n - 1)];
            while from < bytes.len() && !bytes[from].is_ascii_alphanumeric() {
                from += 1;
            }
            let mut to = from;
            while to < bytes.len() && (bytes[to].is_ascii_alphanumeric() || bytes[to] == b'_') {
                to += 1;
            }
            if to > from {
                let mut s = src.clone();
                s.replace_range(from..to, "");
                damaged.push((format!("word@{from}"), s));
            }
        }
        for (kind, text) in damaged {
            variants += 1;
            let e = ex(&rel, &text);
            languages.insert(e.language.clone());
            let (c, r) = empty_names(&e);
            if c != 0 || r != 0 {
                failures.push(format!(
                    "{} {rel} [{kind}]: {c} empty call(s), {r} empty reference(s)",
                    e.language
                ));
            }
        }
    }
    assert!(
        variants > 1000 && languages.len() >= 25,
        "coverage too thin: {variants} variants of {sources} files in {} languages",
        languages.len()
    );
    assert!(
        failures.is_empty(),
        "{variants} damaged variants of {sources} fixtures, {} languages:\n{}",
        languages.len(),
        failures.join("\n")
    );
}

/// ATTACK 4c. Found by the damaged-source sweep over 1,935 real files from the
/// user's own trees (23,980 variants, one hit): deleting the `use` keyword from
/// `use a::b::{self};` leaves a path with a brace list, which the Rust grammar
/// reads as a struct expression with no spellable type, and the extractor
/// publishes a `Constructor` reference named `""` (span `a::b::`). The file is
/// Partial. Not the lane's: it touched only the Swift gate.
#[test]
#[ignore = "pre-existing on main: Rust ` a::b::{c};` (a use path missing its keyword) publishes a Constructor reference named \"\""]
fn a_rust_path_with_a_brace_list_publishes_no_empty_constructor_name() {
    let mut failures = Vec::new();
    for source in [
        " iceoryx2_pal_posix::posix::{self};\n",
        " a::b::{self};\n",
        " a::b::{c};\n",
    ] {
        let e = ex("a.rs", source);
        let (calls, refs) = empty_names(&e);
        if calls != 0 || refs != 0 {
            failures.push(format!(
                "{source:?}: {calls} empty call(s), {refs} empty reference(s) ({:?})",
                e.parse_outcome
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
