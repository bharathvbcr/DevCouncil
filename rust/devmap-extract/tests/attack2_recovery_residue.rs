//! Round-2 adversarial probes for `lane/recovery-residue` @ 44060e05 (card
//! ft-bce92fa0, lane D, Fable ruling 37). Each test states the CORRECT
//! behaviour; a failing test is a BREAK. A test marked
//! `#[ignore = "pre-existing on main: ..."]` fails on main dc397d99 as well.
//!
//! Contract under test: one note per refused bare-cast member node, raised in
//! `references.rs` only, whatever the member's spelling (shadowed, backticked,
//! reserved), whatever the context the walk reaches it through; the moved arm
//! must never refuse a member a later rule correctly handled.

use std::time::Duration;

use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_extract::{extract_all, extract_file_with_budget, FileRef};

const BUDGET: Duration = Duration::from_secs(600);

fn ex(path: &str, source: &str) -> Extraction {
    extract_file_with_budget(path, source, BUDGET)
}

fn member_notes(e: &Extraction) -> Vec<&str> {
    e.diagnostics
        .iter()
        .map(String::as_str)
        .filter(|d| d.contains("off a bare cast"))
        .collect()
}

fn note_count(e: &Extraction) -> usize {
    let notes = member_notes(e);
    assert!(notes.len() <= 1, "more than one note line: {notes:?}");
    notes.first().map_or(0, |n| {
        n.trim_start_matches("swift: refused ")
            .split(' ')
            .next()
            .and_then(|c| c.parse().ok())
            .expect("a count")
    })
}

fn describe(e: &Extraction) -> String {
    format!(
        "calls {:?} refs {:?} diag {:?} outcome {:?}",
        e.calls
            .iter()
            .map(|c| (&c.callee_name, &c.receiver_expr))
            .collect::<Vec<_>>(),
        e.references
            .iter()
            .map(|r| (&r.name, &r.receiver_expr))
            .collect::<Vec<_>>(),
        e.diagnostics,
        e.parse_outcome
    )
}

/// Cast or coalesce text at the top level of a receiver.
fn mangled(text: Option<&str>) -> bool {
    let Some(text) = text else {
        return false;
    };
    let mut depth = 0usize;
    let mut top = String::new();
    for ch in text.chars() {
        match ch {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => top.push(ch),
            _ => {}
        }
    }
    top.contains("as?") || top.contains("as!") || top.contains("??")
}

fn leaks(e: &Extraction) -> Vec<String> {
    let mut out = Vec::new();
    for c in &e.calls {
        let operator = !c
            .callee_name
            .starts_with(|ch: char| ch.is_alphanumeric() || ch == '_');
        if !operator && mangled(c.receiver_expr.as_deref()) {
            out.push(format!("call {} on {:?}", c.callee_name, c.receiver_expr));
        }
    }
    for r in &e.references {
        if mangled(r.receiver_expr.as_deref()) {
            out.push(format!("ref {} on {:?}", r.name, r.receiver_expr));
        }
    }
    out
}

fn has_call(e: &Extraction, name: &str) -> bool {
    e.calls.iter().any(|c| c.callee_name == name)
}

fn has_ref(e: &Extraction, name: &str) -> bool {
    e.references.iter().any(|r| r.name == name)
}

// ---------------------------------------------------------------- ATTACK 1/2

/// (source, member nodes the lane must refuse and note). Counts are by
/// construction: each `X as? T ?? .m` / `(X as? T).` misparse is one refused
/// member; a parenthesised cast, a real receiver and a leading-dot member of a
/// contextual type are not.
const COUNTED: &[(&str, usize)] = &[
    // a bare-cast member that is also an argument label
    ("func f() {\n    _ = b as? T ?? .foo(foo: 1)\n}\n", 1),
    ("func f() {\n    _ = b as? T ?? .foo(foo: foo)\n}\n", 1),
    ("func f(foo: Int) {\n    _ = b as? T ?? .foo(foo: foo)\n}\n", 1),
    ("func f() {\n    _ = b as? T ?? .foo(foo: 1, bar: 2)\n}\n", 1),
    // a leading-dot member in argument position beside a bare cast
    ("func f() {\n    h(b as? T ?? .minute)\n}\n", 1),
    ("func f() {\n    h(.minute, b as? T ?? .minute)\n}\n", 1),
    ("func f() {\n    h(b as? T ?? .minute, .minute)\n}\n", 1),
    ("func f() {\n    h(a: .minute, b: b as? T ?? .minute)\n}\n", 1),
    ("func f() {\n    h(.minute(1), b as? T ?? .minute(2))\n}\n", 1),
    ("func f() -> Shape {\n    .circle(1)\n}\nfunc g() {\n    _ = b as? T ?? .circle(2)\n}\n", 1),
    ("func f() -> Shape {\n    return b as? T ?? .circle(2)\n}\n", 1),
    ("var v: Shape = b as? T ?? .circle(2)\n", 1),
    ("var v: Shape = .circle(1)\nvar w = b as? T ?? .circle(2)\n", 1),
    // nested and repeated casts
    // The grammar hangs `.y` off the OUTER bare cast too (`T ?? (...)` is the
    // cast's right-hand text), so both members are refused: two, not one.
    ("func f() {\n    _ = (b as? T ?? (c as? U ?? .x()).y())\n}\n", 2),
    ("func f() {\n    _ = b as? T ?? (c as? U ?? .x()).y()\n}\n", 2),
    ("func f() {\n    _ = (c as? U ?? .x()).y()\n}\n", 1),
    ("func f() {\n    _ = (b as? (c as? T).U)\n}\n", 0),
    ("func f() {\n    _ = (b as? T).u ?? (c as? V).w\n}\n", 0),
    // two bare casts in one expression, each with its own member
    ("func f() {\n    _ = [b as? T ?? .x(), c as? U ?? .y()]\n}\n", 2),
    ("func f() {\n    g(b as? T ?? .x(), c as? U ?? .y())\n}\n", 2),
    ("func f() {\n    _ = (b as? T ?? .x(), c as? U ?? .y())\n}\n", 2),
    ("func f() {\n    let a = b as? T ?? .x()\n    let c = d as? U ?? .y()\n}\n", 2),
    ("func f() {\n    _ = b as? T ?? .x() + c as? U ?? .y()\n}\n", 2),
    // the same member name used twice on different casts
    ("func f() {\n    _ = [b as? T ?? .foo(), c as? U ?? .foo()]\n}\n", 2),
    ("func f() {\n    let foo = 1\n    _ = [b as? T ?? .foo(), c as? U ?? .foo()]\n}\n", 2),
    // a chain longer than one hop: each member of the chain is its own node
    ("func f() {\n    _ = b as? T ?? .a().b().c()\n}\n", 3),
    ("func f() {\n    _ = b as? T ?? .a.b.c\n}\n", 3),
    ("func f() {\n    let a = 1\n    _ = b as? T ?? .a.b.c\n}\n", 3),
    ("func f() {\n    let b = 1\n    _ = x as? T ?? .a.b.c\n}\n", 3),
    // a member that is both a call callee and read as a value
    ("func f() {\n    _ = b as? T ?? .foo(.foo)\n}\n", 1),
    ("func f() {\n    _ = b as? T ?? .foo(c as? U ?? .foo())\n}\n", 2),
    ("func f() {\n    _ = b as? T ?? .foo({ $0 as? U ?? .foo() })\n}\n", 2),
    ("func f() {\n    _ = b as? T ?? .foo { $0 as? U ?? .foo() }\n}\n", 2),
    // subscript off the cast member: the member is a read, not a call
    ("func f() {\n    _ = b as? T ?? .foo[0]\n}\n", 1),
    ("func f() {\n    _ = b as? T ?? .foo[0].bar()\n}\n", 2),
    // trailing closures
    ("func f() {\n    _ = b as? T ?? .foo { }\n}\n", 1),
    ("func f() {\n    _ = b as? T ?? .foo { } .bar { }\n}\n", 2),
    // macro expansion: the argument of a freestanding macro is an expression
    ("func f() {\n    _ = #stringify(b as? T ?? .foo())\n}\n", 1),
    ("func f() {\n    #warning(\"x\")\n    _ = #stringify(b as? T ?? .foo(), c as? U ?? .foo())\n}\n", 2),
    // an optional-chained member after a bare cast member
    ("func f() {\n    _ = b as? T ?? .a?.b()\n}\n", 2),
];

/// ATTACK 1/2. The count is exact for every spelled shape, and no cast text
/// leaks as a receiver.
#[test]
fn every_refused_member_is_one_note_in_the_shapes_the_move_could_disturb() {
    let mut failures = Vec::new();
    for (source, want) in COUNTED {
        let e = ex("a.swift", source);
        let got = note_count(&e);
        if got != *want {
            failures.push(format!(
                "{source:?}: want {want} got {got}: {}",
                describe(&e)
            ));
        }
        for leak in leaks(&e) {
            failures.push(format!("{source:?}: leaked {leak}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ------------------------------------------------------------- ATTACK 2 (meta)

/// Contexts that each hold exactly one `{E}`.
const CONTEXTS: &[&str] = &[
    "let a = {E}\n",
    "func f() {\n    _ = {E}\n}\n",
    "func f() {\n    let foo = 1\n    _ = {E}\n    _ = foo\n}\n",
    "func f(foo: Int) {\n    _ = {E}\n}\n",
    "let g = { (foo: Int) in\n    {E}\n}\n",
    "let g = { foo in\n    {E}\n}\n",
    "let g = { [foo] in\n    {E}\n}\n",
    "let g = { $0\n    _ = {E}\n}\n",
    "func f() {\n    g { _ = {E} }\n}\n",
    "func f() {\n    g { foo in _ = {E} }\n}\n",
    "func f() {\n    g(x: {E})\n}\n",
    "func f() {\n    g({E}, y: {E_OTHER})\n}\n",
    "let a = \"\\({E})\"\n",
    "let a = \"x \\({E}) y \\({E_NONE})\"\n",
    "let a = \"\"\"\n  x \\({E})\n  \"\"\"\n",
    "#if os(iOS)\nlet a = {E}\n#else\nlet a = 1\n#endif\n",
    "#if os(iOS)\nlet a = 1\n#else\nlet a = {E}\n#endif\n",
    "#if DEBUG\n#if os(iOS)\nlet a = {E}\n#endif\n#endif\n",
    "#if os(iOS)\nfunc f() { _ = {E} }\n#endif\n",
    "struct S: View {\n    var body: some View {\n        Text(\"x\")\n        Foo({E})\n    }\n}\n",
    "struct S: View {\n    var body: some View {\n        VStack {\n            if cond {\n                Text(\"x\")\n                Foo({E})\n            } else {\n                Bar()\n            }\n        }\n    }\n}\n",
    "@resultBuilder struct RB { static func buildBlock(_ x: Any...) -> Any { x } }\n@RB func f() -> Any {\n    {E}\n    1\n}\n",
    "func f() {\n    defer { _ = {E} }\n}\n",
    "func f() {\n    let g = { () -> Any? in {E} }\n}\n",
    "func f() {\n    @autoclosure let g = {E}\n}\n",
    "func f() {\n    if cond { _ = {E} } else { _ = 1 }\n}\n",
    "func f() {\n    guard let _ = Optional(1) else { _ = {E}; return }\n}\n",
    "func f() {\n    switch x { case 1: _ = {E}\n    default: break }\n}\n",
    "func f() {\n    for foo in xs { _ = {E} }\n}\n",
    "func f() {\n    if let foo = x { _ = {E} }\n}\n",
    "func f() {\n    while let foo = x { _ = {E} }\n}\n",
    "func f() {\n    do { let foo = 1; _ = {E}; _ = foo } catch { }\n}\n",
    "func f() {\n    func inner(foo: Int) { _ = {E} }\n    inner(foo: 1)\n}\n",
    "class C {\n    var foo = 1\n    func m() { _ = {E} }\n}\n",
    "class C {\n    var v: Any? { {E} }\n}\n",
    "class C {\n    var v: Any? {\n        get { {E} }\n        set { _ = foo }\n    }\n}\n",
    "class C {\n    var v = 0 {\n        willSet { _ = {E} }\n        didSet { }\n    }\n}\n",
    "class C {\n    lazy var v = {E}\n}\n",
    "class C {\n    var v = {E}\n    init() {}\n}\n",
    "class C {\n    init(foo: Int) { _ = {E} }\n}\n",
    "class C {\n    deinit { _ = {E} }\n}\n",
    "class C {\n    subscript(foo: Int) -> Any? { {E} }\n}\n",
    "func f(_ x: Any? = {E}) {}\n",
    "extension E {\n    func f(foo: Int) { _ = {E} }\n}\n",
    "enum E { case a\n    func f() { _ = {E} } }\n",
    "actor A { func f() async { _ = await g({E}) } }\n",
    "func f() throws {\n    _ = try g({E})\n}\n",
    "func f() {\n    let a = [\"k\": {E}]\n}\n",
    "func f() {\n    let a = [{E}, {E_NONE}]\n}\n",
    "func f() {\n    let (foo, bar) = (1, 2)\n    _ = {E}\n    _ = (foo, bar)\n}\n",
    "func f() {\n    var foo = 1\n    foo += 1\n    _ = {E}\n}\n",
    "func f() {\n    let foo: Int\n    foo = 1\n    _ = {E}\n}\n",
    "func f<foo>(_ x: foo) {\n    _ = {E}\n}\n",
    "@available(*, deprecated)\nfunc f() { _ = {E} }\n",
    "func f() { _ = {E} }\nfunc g() { _ = {E_NONE} }\n",
    "@Foo({E}) struct S {}\n",
    "struct S {\n    @State var v = {E}\n}\n",
    "enum E: Int { case a = 1 }\nfunc f() { _ = {E} }\n",
    "func f() {\n    if case .some(let v) = {E} { _ = v }\n}\n",
    "func f() {\n    guard case .some(let v) = {E} else { return }\n}\n",
    "func f() {\n    switch x { case let v where v == {E}: break\n    default: break }\n}\n",
    "func f() {\n    switch {E} { case .a: break\n    default: break }\n}\n",
    "func f() {\n    for v in {E} { _ = v }\n}\n",
    "func f() {\n    let (a, b) = ({E}, 1)\n}\n",
    "func f() {\n    let p = #Predicate<Int> { $0 == {E} }\n}\n",
    "func f() {\n    let k = \\Foo.bar\n    _ = {E}\n}\n",
    "struct S<T: P> where T.X == Int {\n    func f() { _ = {E} }\n}\n",
    "func f() {\n    x.y = {E}\n}\n",
    "func f() {\n    x[0] = {E}\n}\n",
    "func f() {\n    _ = foo ? {E} : {E_NONE}\n}\n",
    "func f() {\n    _ = {E_NONE} ?? {E}\n}\n",
    "func f() {\n    _ = try? {E}\n}\n",
    "func f() {\n    _ = await {E}\n}\n",
    "func f() {\n    _ = -({E})\n}\n",
    "func f() {\n    _ = !{E}\n}\n",
    "func f() {\n    _ = &{E}\n}\n",
    "func f() {\n    return;\n    _ = {E}\n}\n",
    "func f() {\n    _ = {E}\n}\nfunc broken( {\n",
    "func broken( {\nfunc f() {\n    _ = {E}\n}\n",
    "let broken = = =\nfunc f() {\n    _ = {E}\n}\n",
    "func f() {\n    _ = {E}\n}\n// trailing\n#endif\n",
    "protocol P { }\nextension P {\n    var v: Any? { {E} }\n}\n",
    "func f() {\n    Task { @MainActor in _ = {E} }\n}\n",
    "func f() {\n    DispatchQueue.main.async { [weak self] in\n        guard let foo = self else { return }\n        _ = {E}\n    }\n}\n",
    "func f() {\n    let h: () -> Void = { _ = {E} }\n    h()\n}\n",
    "func f() {\n    let h = { (a: Int, foo: Int) -> Int in _ = {E}; return a }\n}\n",
];

/// Member spellings: (member text, whether a binding of it is also declared).
/// `{M}` is spliced into `.{M}()`.
const SPELLINGS: &[&str] = &[
    "foo", "_foo", "$0x", "init", "self", "default", "none", "some", "Type", "`class`", "`foo`",
    "is", "as", "T", "print", "self", "Foo", "x", "l", "map",
];

fn expr(member: &str) -> String {
    format!("b as? T ?? .{member}()")
}

/// ATTACK 2. Metamorphic: the note count of a source must not depend on how its
/// member is spelled or what binds that spelling, because the arm now runs
/// before every spelling rule. A context holding exactly one `{E}` refuses
/// exactly one member for every spelling (the raw identifier is a separate
/// Partial case).
#[test]
fn the_note_count_is_independent_of_the_member_spelling_in_every_context() {
    let mut failures = Vec::new();
    let mut seen = 0;
    for ctx in CONTEXTS {
        let slots = ctx.matches("{E}").count();
        for member in SPELLINGS {
            let source = ctx
                .replace("{E_OTHER}", "1")
                .replace("{E_NONE}", "1")
                .replace("{E}", &expr(member));
            let e = ex("a.swift", &source);
            seen += 1;
            // A source the grammar could not read has nothing to count.
            if !matches!(e.parse_outcome, ParseOutcome::Clean) && ctx.contains("broken") {
                continue;
            }
            let got = note_count(&e);
            let base = ex(
                "a.swift",
                &ctx.replace("{E_OTHER}", "1")
                    .replace("{E_NONE}", "1")
                    .replace("{E}", &expr("zzqq")),
            );
            let want = note_count(&base);
            if got != want || (want != slots && !ctx.contains("broken")) {
                failures.push(format!(
                    "member {member:?} in {ctx:?}: got {got}, plain-name baseline {want}, slots {slots}\n   {}",
                    describe(&e)
                ));
            }
            for leak in leaks(&e) {
                failures.push(format!("member {member:?} in {ctx:?}: leaked {leak}"));
            }
        }
    }
    assert!(seen > 1000);
    assert!(
        failures.is_empty(),
        "{} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// ATTACK 2. A refusal in `swift_navigation_call` is never silent: if the call
/// to a member of a bare cast is absent, a note exists.
#[test]
fn a_member_call_off_a_bare_cast_is_never_published_without_a_note() {
    let mut failures = Vec::new();
    for ctx in CONTEXTS {
        let source = ctx
            .replace("{E_OTHER}", "1")
            .replace("{E_NONE}", "1")
            .replace("{E}", "b as? T ?? .zzqq()");
        let e = ex("a.swift", &source);
        if has_call(&e, "zzqq") {
            failures.push(format!("{ctx:?}: call zzqq published: {}", describe(&e)));
        }
        if has_ref(&e, "zzqq") {
            failures.push(format!("{ctx:?}: ref zzqq published: {}", describe(&e)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ------------------------------------------------------------------ ATTACK 1b

/// ATTACK 1. A real reference named like a refused member survives: the moved
/// arm refuses by node, not by name, so a real `foo` beside a refused `.foo`
/// must still publish its facts.
#[test]
fn a_real_reference_with_the_name_of_a_refused_member_still_publishes() {
    let cases: &[(&str, &str)] = &[
        (
            "func foo() {}\nfunc f() {\n    _ = b as? T ?? .foo()\n    foo()\n}\n",
            "foo",
        ),
        (
            "let foo = 1\nfunc f() {\n    _ = b as? T ?? .foo()\n    _ = foo\n}\n",
            "foo",
        ),
        (
            "func foo() {}\nfunc f() {\n    _ = (b as? T)?.foo()\n    _ = b as? T ?? .foo()\n}\n",
            "foo",
        ),
        (
            "func foo() {}\nfunc f() {\n    h(foo: b as? T ?? .foo())\n    run(foo)\n}\n",
            "foo",
        ),
        (
            "func foo() {}\nfunc f() {\n    _ = b as? T ?? .bar(foo)\n}\n",
            "foo",
        ),
        (
            "func foo() {}\nfunc f() {\n    _ = b as? T ?? .bar(foo())\n}\n",
            "foo",
        ),
        (
            "func foo() {}\nfunc f() {\n    _ = (b as? T ?? .bar).baz(foo)\n}\n",
            "foo",
        ),
    ];
    let mut failures = Vec::new();
    for (source, name) in cases {
        let e = ex("a.swift", source);
        if !(has_call(&e, name) || has_ref(&e, name)) {
            failures.push(format!(
                "{source:?}: lost every fact for {name}: {}",
                describe(&e)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 1. The real members beside a cast still publish when the lane's note
/// fires in the same expression.
#[test]
fn real_members_beside_a_refused_member_survive_the_moved_arm() {
    let cases: &[(&str, &str)] = &[
        (
            "func f() {\n    _ = (b as? T)?.real() ?? c as? U ?? .fake()\n}\n",
            "real",
        ),
        (
            "func f() {\n    _ = x.real() ?? b as? U ?? .fake()\n}\n",
            "real",
        ),
        (
            "func f() {\n    _ = b as? U ?? .fake(x.real())\n}\n",
            "real",
        ),
        (
            "func f() {\n    _ = b as? U ?? .fake(label: x.real())\n}\n",
            "real",
        ),
        (
            "func f() {\n    _ = b as? U ?? .fake({ x.real() })\n}\n",
            "real",
        ),
        ("func f() {\n    _ = (b as? U ?? .fake).real()\n}\n", "real"),
        (
            "func f() {\n    _ = (b as? U ?? .fake()).real()\n}\n",
            "real",
        ),
        (
            "func f() {\n    let foo = 1\n    _ = x.real(foo) ?? b as? U ?? .foo()\n}\n",
            "real",
        ),
        (
            "func f() {\n    _ = b as? U ?? .fake()\n    x.real()\n}\n",
            "real",
        ),
        (
            "func f() {\n    g(b as? U ?? .fake(), x.real())\n}\n",
            "real",
        ),
    ];
    let mut failures = Vec::new();
    for (source, name) in cases {
        let e = ex("a.swift", source);
        if !has_call(&e, name) {
            failures.push(format!("{source:?}: lost call {name}: {}", describe(&e)));
        }
        if has_call(&e, "fake") || has_ref(&e, "fake") {
            failures.push(format!("{source:?}: published fake: {}", describe(&e)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ------------------------------------------------------------------ ATTACK 2b

/// ATTACK 2. The count is per file under `extract_all`, and does not carry from
/// a file with notes into a file with none, across many threads.
#[test]
fn the_note_does_not_leak_between_files_under_rayon() {
    let noted = "func f() {\n    let foo = 1\n    _ = b as? T ?? .foo()\n    _ = foo\n}\n";
    let quiet = "func g() { _ = (b as? T)?.foo() }\n";
    let paths: Vec<String> = (0..200).map(|i| format!("f{i}.swift")).collect();
    let files: Vec<FileRef> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| FileRef {
            path,
            source: if i % 2 == 0 { noted } else { quiet },
        })
        .collect();
    let out = extract_all(&files);
    let mut failures = Vec::new();
    for (i, e) in out.iter().enumerate() {
        let want = usize::from(i % 2 == 0);
        let got = note_count(e);
        if got != want {
            failures.push(format!("f{i}: want {want} got {got}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 2. A file with a partial parse elsewhere still counts a refused member
/// in its valid region exactly once (recovery walks must not revisit it).
#[test]
fn a_partial_file_counts_a_member_in_its_valid_region_once() {
    let cases: &[&str] = &[
        "func broken( {\n}\nfunc f() {\n    _ = b as? T ?? .foo()\n}\n",
        "func f() {\n    _ = b as? T ?? .foo()\n}\nstruct S {\n    var x: = \n}\n",
        "func f() {\n    _ = b as? T ?? .foo()\n}\nclass C { func m( }\n",
        "extension X {\n    func f() { _ = b as? T ?? .foo() }\n    func g(\n}\n",
        "func f() {\n    _ = b as? T ?? .foo()\n    let = 3\n}\n",
    ];
    let mut failures = Vec::new();
    for source in cases {
        let e = ex("a.swift", source);
        let got = note_count(&e);
        if got > 1 {
            failures.push(format!("{source:?}: counted {got}: {}", describe(&e)));
        }
        if has_call(&e, "foo") {
            failures.push(format!("{source:?}: published foo: {}", describe(&e)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ------------------------------------------------------------------- harness

/// A compact, span-free, sorted view of the facts a resolver reads.
fn compact(e: &Extraction) -> serde_json::Value {
    let mut calls: Vec<String> = e
        .calls
        .iter()
        .map(|c| {
            format!(
                "{}|{}|{}",
                c.caller_symbol.as_deref().unwrap_or(""),
                c.callee_name,
                c.receiver_expr.as_deref().unwrap_or("-")
            )
        })
        .collect();
    calls.sort();
    let mut refs: Vec<String> = e
        .references
        .iter()
        .map(|r| {
            format!(
                "{:?}|{}|recv={}|asg={}|enc={}",
                r.kind,
                r.name,
                r.receiver_expr.as_deref().unwrap_or("-"),
                r.assigned_to.as_deref().unwrap_or("-"),
                r.enclosing_symbol.as_deref().unwrap_or("-")
            )
        })
        .collect();
    refs.sort();
    let mut syms: Vec<String> = e
        .symbols
        .iter()
        .map(|s| format!("{:?}|{}", s.kind, s.name))
        .collect();
    syms.sort();
    let mut diags = e.diagnostics.clone();
    diags.sort();
    serde_json::json!({
        "outcome": format!("{:?}", e.parse_outcome),
        "calls": calls,
        "refs": refs,
        "syms": syms,
        "diag": diags,
    })
}

/// Dump the facts of every file in `RR_LIST` to `RR_OUT` as `path<TAB>json`.
#[test]
#[ignore = "harness: dumps RR_OUT for the file list RR_LIST, to diff against another arm"]
fn harness_dump_files() {
    use std::io::Write;
    let list = std::fs::read_to_string(std::env::var("RR_LIST").expect("RR_LIST")).expect("list");
    let out_path = std::env::var("RR_OUT").expect("RR_OUT");
    let mut out = std::io::BufWriter::new(std::fs::File::create(out_path).expect("create"));
    for path in list.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let e = ex(path, &source);
        writeln!(out, "{path}\t{}", compact(&e)).expect("write");
    }
}

/// Extract `RR_FILE` `RR_REPS` times and print the minimum wall time, for an
/// instruction-count run under `/usr/bin/time -l`.
#[test]
#[ignore = "harness: times RR_FILE x RR_REPS"]
fn harness_time_file() {
    let path = std::env::var("RR_FILE").expect("RR_FILE");
    let reps: usize = std::env::var("RR_REPS")
        .expect("RR_REPS")
        .parse()
        .expect("reps");
    let source = std::fs::read_to_string(&path).expect("read");
    let mut best = Duration::MAX;
    for _ in 0..reps {
        let start = std::time::Instant::now();
        let e = ex(&path, &source);
        best = best.min(start.elapsed());
        std::hint::black_box(&e);
    }
    eprintln!("RR_BEST_NS {}", best.as_nanos());
}

// ------------------------------------------------------------ ATTACK 2 (more)

/// More contexts and nesting that might make the walk visit one member twice or
/// reach one through a path the arm does not see.
const COUNTED_MORE: &[(&str, usize)] = &[
    (
        "let s = \"\\(b as? T ?? .foo(\"x \\(c as? U ?? .bar())\"))\"\n",
        2,
    ),
    (
        "let s = \"\"\"\n  \\(b as? T ?? .foo())\n  \\(c as? U ?? .bar())\n  \"\"\"\n",
        2,
    ),
    ("func f() async {\n    async let x = b as? T ?? .foo()\n    _ = await x\n}\n", 1),
    ("func f() {\n    repeat { } while (b as? T ?? .foo()) == nil\n}\n", 1),
    ("func f() {\n    outer: for v in xs { _ = b as? T ?? .foo(); continue outer }\n}\n", 1),
    ("class C {\n    static let shared = b as? T ?? .foo()\n}\n", 1),
    ("class C {\n    #if DEBUG\n    var v = b as? T ?? .foo()\n    #endif\n}\n", 1),
    ("struct S {\n    let x = b as? T ?? .foo()\n    let y = c as? U ?? .foo()\n}\n", 2),
    ("enum E {\n    case a(Int = 0)\n    static var d = b as? T ?? .foo()\n}\n", 1),
    ("typealias A = Int\nfunc f() { _ = b as? T ?? .foo() }\n", 1),
    ("func f() {\n    let g = { { { b as? T ?? .foo() }() }() }()\n}\n", 1),
    ("func f() {\n    g { h { i { _ = b as? T ?? .foo() } } }\n}\n", 1),
    ("func f() {\n    let a = [1].map { x in b as? T ?? .foo() }.filter { y in c as? U ?? .foo() }\n}\n", 2),
    ("func f() {\n    let a = xs.map { $0 as? T ?? .foo() }\n}\n", 1),
    ("func f() {\n    let a = xs.map { ($0 as? T)?.foo() ?? .bar() }\n}\n", 0),
    ("func f() {\n    let a = b as? T ?? .foo() as? U ?? .bar()\n}\n", 2),
    ("func f() {\n    let a = (b as? T ?? .foo()) as? U ?? .bar()\n}\n", 2),
    ("func f() {\n    let a = b as? T ?? (.foo())\n}\n", 0),
    ("func f() {\n    let a = b as? T ?? (x.foo())\n}\n", 0),
    ("func f() {\n    let a: T = b as? T ?? T.foo()\n}\n", 1),
    ("func f() {\n    let a: [T] = b as? [T] ?? [].foo()\n}\n", 1),
];

#[test]
fn every_refused_member_is_one_note_in_nested_and_unusual_contexts() {
    let mut failures = Vec::new();
    for (source, want) in COUNTED_MORE {
        let e = ex("a.swift", source);
        let got = note_count(&e);
        if got != *want {
            failures.push(format!(
                "{source:?}: want {want} got {got}: {}",
                describe(&e)
            ));
        }
        for leak in leaks(&e) {
            failures.push(format!("{source:?}: leaked {leak}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 2. The raw-identifier probe is exempt from a note because the grammar
/// errors on it; the exemption must not let a neighbouring member publish cast
/// text as its receiver.
#[test]
fn a_raw_identifier_member_beside_a_bare_cast_leaks_no_cast_receiver() {
    let cases: &[&str] = &[
        "func f() {\n    _ = b as? T ?? .`foo bar`().baz()\n}\n",
        "func f() {\n    _ = b as? T ?? .baz().`foo bar`()\n}\n",
        "func f() {\n    _ = b as? T ?? .`foo bar`\n    _ = c as? U ?? .qux()\n}\n",
        "func f() {\n    _ = (b as? T ?? .`foo bar`()).baz()\n}\n",
        "func f() {\n    let `foo bar` = 1\n    _ = b as? T ?? .qux(`foo bar`)\n}\n",
    ];
    let mut failures = Vec::new();
    for source in cases {
        let e = ex("a.swift", source);
        for leak in leaks(&e) {
            failures.push(format!("{source:?}: leaked {leak}: {}", describe(&e)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
