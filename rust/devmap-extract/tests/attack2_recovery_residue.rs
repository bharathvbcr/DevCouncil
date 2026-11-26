//! Round-2 adversarial probes for `lane/recovery-residue` @ 44060e05 (card
//! ft-bce92fa0, lane D, Fable ruling 37). Each test states the CORRECT
//! behaviour; a failing test is a BREAK. A test marked
//! `#[ignore = "pre-existing on main: ..."]` fails on main dc397d99 as well.
//!
//! Contract under test: one note per refused bare-cast member node, raised in
//! `references.rs` only, whatever the member's spelling (shadowed, backticked,
//! reserved), whatever the context the walk reaches it through; the moved arm
//! must never refuse a member a later rule correctly handled.
//!
//! tree-sitter-swift 0.7.4: `b as? T ?? X` parses as
//! `nil_coalescing_expression(value: as_expression, if_nil: X)`, so the
//! cast-coalesce probes below (`COUNTED`, `COUNTED_MORE`, `CONTEXTS`) refuse
//! nothing and raise no note; `X` is extracted as it would be on its own, and
//! must equal the same source with the cast parenthesised (`(b as? T) ?? X`).
//! What is still refused is a cast to a QUALIFIED type, which the grammar reads
//! as a member of the cast (`(b as? Foo.Bar)` is `navigation(as_expression
//! "b as? Foo", Bar)`); the note-isolation and metamorphic checks use that
//! trigger.

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

/// The same source with every simple `X as? T ?? ` (operand a plain identifier,
/// type a plain or bracketed name) written `(X as? T) ?? `. Swift gives `as`
/// higher precedence than `??`, so this is the same expression; it is the
/// "parenthesised twin" the cast-coalesce probes must equal. Returns the twin
/// and how many casts it wrapped.
fn parenthesised_twin(source: &str) -> (String, usize) {
    let is_operand = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let is_type = |c: char| c.is_alphanumeric() || c == '_' || c == '[' || c == ']';
    let mut out = String::new();
    let mut wrapped = 0;
    let mut rest = source;
    while let Some(ch) = rest.chars().next() {
        let cast = [" as? ", " as! "].into_iter().find(|c| rest.starts_with(c));
        if let Some(cast) = cast {
            let operand_len = out.chars().rev().take_while(|c| is_operand(*c)).count();
            let after = &rest[cast.len()..];
            let ty_len = after.chars().take_while(|c| is_type(*c)).count();
            if operand_len > 0 && ty_len > 0 && after[ty_len..].starts_with(" ?? ") {
                let operand: String = out
                    .chars()
                    .skip(out.chars().count() - operand_len)
                    .collect();
                out.truncate(out.len() - operand.len());
                out.push('(');
                out.push_str(&operand);
                out.push_str(cast);
                out.push_str(&after[..ty_len]);
                out.push(')');
                rest = &after[ty_len..];
                wrapped += 1;
                continue;
            }
        }
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    (out, wrapped)
}

/// The calls and references a resolver reads, receivers compared without
/// parentheses (a twin differs from its bare spelling only by the parentheses
/// around the cast, which a receiver's source text carries) and without
/// `assigned_to` (a separate, ignored follow-up: ft-c0b8696f).
fn facts(e: &Extraction) -> (Vec<String>, Vec<String>) {
    let bare = |r: &Option<String>| {
        r.as_deref()
            .map_or("-".to_string(), |r| r.replace(['(', ')', ' '], ""))
    };
    let mut calls: Vec<String> = e
        .calls
        .iter()
        .map(|c| {
            format!(
                "{}|{}|{}",
                c.caller_symbol.as_deref().unwrap_or(""),
                c.callee_name,
                bare(&c.receiver_expr)
            )
        })
        .collect();
    let mut refs: Vec<String> = e
        .references
        .iter()
        .map(|r| {
            format!(
                "{:?}|{}|{}|{}",
                r.kind,
                r.name,
                bare(&r.receiver_expr),
                r.enclosing_symbol.as_deref().unwrap_or("-")
            )
        })
        .collect();
    calls.sort();
    refs.sort();
    (calls, refs)
}

/// The 0.7.4 contract for one cast-coalesce probe: it stays Clean, raises no
/// note, publishes no empty name and no cast text as a receiver, and equals its
/// parenthesised twin. Returns whether a twin cast was made.
fn judge_cast_coalesce(source: &str, failures: &mut Vec<String>) -> bool {
    let e = ex("a.swift", source);
    if !e.diagnostics.is_empty() {
        failures.push(format!(
            "{source:?}: nothing is refused, yet {}",
            describe(&e)
        ));
    }
    for leak in leaks(&e) {
        failures.push(format!("{source:?}: leaked {leak}"));
    }
    if e.calls.iter().any(|c| c.callee_name.trim().is_empty())
        || e.references.iter().any(|r| r.name.trim().is_empty())
    {
        failures.push(format!("{source:?}: an empty name"));
    }
    let (twin, wrapped) = parenthesised_twin(source);
    if wrapped > 0 {
        let t = ex("a.swift", &twin);
        if facts(&e) != facts(&t) {
            failures.push(format!(
                "{source:?}: differs from its twin {twin:?}\n   bare {:?}\n   twin {:?}",
                facts(&e),
                facts(&t)
            ));
        }
    }
    wrapped > 0
}

// ---------------------------------------------------------------- ATTACK 1/2

/// Cast-coalesce shapes the lane used to refuse and count (one note per refused
/// member). Under tree-sitter-swift 0.7.4 none of them is refused: each is
/// judged by `judge_cast_coalesce` (no note, no leak, equal to its twin).
const COUNTED: &[&str] = &[
    // a bare-cast member that is also an argument label
    "func f() {\n    _ = b as? T ?? .foo(foo: 1)\n}\n",
    "func f() {\n    _ = b as? T ?? .foo(foo: foo)\n}\n",
    "func f(foo: Int) {\n    _ = b as? T ?? .foo(foo: foo)\n}\n",
    "func f() {\n    _ = b as? T ?? .foo(foo: 1, bar: 2)\n}\n",
    // a leading-dot member in argument position beside a bare cast
    "func f() {\n    h(b as? T ?? .minute)\n}\n",
    "func f() {\n    h(.minute, b as? T ?? .minute)\n}\n",
    "func f() {\n    h(b as? T ?? .minute, .minute)\n}\n",
    "func f() {\n    h(a: .minute, b: b as? T ?? .minute)\n}\n",
    "func f() {\n    h(.minute(1), b as? T ?? .minute(2))\n}\n",
    "func f() -> Shape {\n    .circle(1)\n}\nfunc g() {\n    _ = b as? T ?? .circle(2)\n}\n",
    "func f() -> Shape {\n    return b as? T ?? .circle(2)\n}\n",
    "var v: Shape = b as? T ?? .circle(2)\n",
    "var v: Shape = .circle(1)\nvar w = b as? T ?? .circle(2)\n",
    // nested and repeated casts
    // 0.7.3 hung `.y` off the OUTER bare cast too; 0.7.4 publishes `y` on the
    // parenthesised `(c as? U ?? .x())`.
    "func f() {\n    _ = (b as? T ?? (c as? U ?? .x()).y())\n}\n",
    "func f() {\n    _ = b as? T ?? (c as? U ?? .x()).y()\n}\n",
    "func f() {\n    _ = (c as? U ?? .x()).y()\n}\n",
    "func f() {\n    _ = (b as? (c as? T).U)\n}\n",
    "func f() {\n    _ = (b as? T).u ?? (c as? V).w\n}\n",
    // two bare casts in one expression, each with its own member
    "func f() {\n    _ = [b as? T ?? .x(), c as? U ?? .y()]\n}\n",
    "func f() {\n    g(b as? T ?? .x(), c as? U ?? .y())\n}\n",
    "func f() {\n    _ = (b as? T ?? .x(), c as? U ?? .y())\n}\n",
    "func f() {\n    let a = b as? T ?? .x()\n    let c = d as? U ?? .y()\n}\n",
    "func f() {\n    _ = b as? T ?? .x() + c as? U ?? .y()\n}\n",
    // the same member name used twice on different casts
    "func f() {\n    _ = [b as? T ?? .foo(), c as? U ?? .foo()]\n}\n",
    "func f() {\n    let foo = 1\n    _ = [b as? T ?? .foo(), c as? U ?? .foo()]\n}\n",
    // a chain longer than one hop: each member of the chain is its own node
    "func f() {\n    _ = b as? T ?? .a().b().c()\n}\n",
    "func f() {\n    _ = b as? T ?? .a.b.c\n}\n",
    "func f() {\n    let a = 1\n    _ = b as? T ?? .a.b.c\n}\n",
    "func f() {\n    let b = 1\n    _ = x as? T ?? .a.b.c\n}\n",
    // a member that is both a call callee and read as a value
    "func f() {\n    _ = b as? T ?? .foo(.foo)\n}\n",
    "func f() {\n    _ = b as? T ?? .foo(c as? U ?? .foo())\n}\n",
    "func f() {\n    _ = b as? T ?? .foo({ $0 as? U ?? .foo() })\n}\n",
    "func f() {\n    _ = b as? T ?? .foo { $0 as? U ?? .foo() }\n}\n",
    // subscript off the cast member: the member is a read, not a call
    "func f() {\n    _ = b as? T ?? .foo[0]\n}\n",
    "func f() {\n    _ = b as? T ?? .foo[0].bar()\n}\n",
    // trailing closures
    "func f() {\n    _ = b as? T ?? .foo { }\n}\n",
    "func f() {\n    _ = b as? T ?? .foo { } .bar { }\n}\n",
    // macro expansion: the argument of a freestanding macro is an expression
    "func f() {\n    _ = #stringify(b as? T ?? .foo())\n}\n",
    "func f() {\n    #warning(\"x\")\n    _ = #stringify(b as? T ?? .foo(), c as? U ?? .foo())\n}\n",
    // an optional-chained member after a bare cast member
    "func f() {\n    _ = b as? T ?? .a?.b()\n}\n",
];

/// ATTACK 1/2, rewritten for tree-sitter-swift 0.7.4. Every cast-coalesce shape
/// in `COUNTED` is parsed correctly: no note, no cast text as a receiver, and
/// the facts equal its parenthesised twin. The sweep must have made a twin for
/// most of the table, or it compared nothing.
#[test]
fn every_refused_member_is_one_note_in_the_shapes_the_move_could_disturb() {
    let mut failures = Vec::new();
    let twinned = COUNTED
        .iter()
        .filter(|source| judge_cast_coalesce(source, &mut failures))
        .count();
    assert!(
        twinned >= COUNTED.len() - 5,
        "only {twinned} of {} probes had a cast to parenthesise",
        COUNTED.len()
    );
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

/// The live refusal trigger: a cast to a qualified type, which the grammar reads
/// as a member of the cast (`navigation(as_expression "b as? Foo", member)`).
fn live_expr(member: &str) -> String {
    format!("(b as? Foo.{member})")
}

/// Contexts where 0.7.4 parses `(b as? Foo.member)` correctly as a qualified type
/// (a prefix `!` or `&` operand), so nothing is refused there: the count is the
/// same for every spelling (zero), but not the number of slots.
const LIVE_PARSES_CORRECTLY: &[&str] = &[
    "func f() {\n    _ = !{E}\n}\n",
    "func f() {\n    _ = &{E}\n}\n",
];

/// Spellings that stay a plain member name after `Foo.` (the others, such as
/// `init`, `self`, `Type`, `is` and `as`, are keywords or metatypes there and
/// parse differently, so they cannot be a refused member).
fn plain_member(member: &str) -> bool {
    !matches!(member, "init" | "self" | "Type" | "is" | "as" | "$0x")
}

/// ATTACK 2. Metamorphic: the note count of a source must not depend on how its
/// member is spelled or what binds that spelling, because the arm runs before
/// every spelling rule.
///
/// tree-sitter-swift 0.7.4, two halves. (1) The cast-coalesce shape `b as? T ??
/// .{member}()` in every context is parsed correctly for every spelling: no
/// note, no leak, and the count (zero) equals the plain-name baseline. (2) The
/// live trigger `(b as? Foo.{member})` (a context holding exactly one `{E}`
/// refuses exactly one member) is noted once per slot for every plain
/// spelling, equal to the plain-name baseline `zzqq`.
#[test]
fn the_note_count_is_independent_of_the_member_spelling_in_every_context() {
    let mut failures = Vec::new();
    let mut seen = 0;
    let mut live_seen = 0;
    for ctx in CONTEXTS {
        let slots = ctx.matches("{E}").count();
        let fill = |e: &str| {
            ctx.replace("{E_OTHER}", "1")
                .replace("{E_NONE}", "1")
                .replace("{E}", e)
        };
        for member in SPELLINGS {
            let source = fill(&expr(member));
            let e = ex("a.swift", &source);
            seen += 1;
            if note_count(&e) != 0 || !e.diagnostics.is_empty() {
                failures.push(format!(
                    "member {member:?} in {ctx:?}: nothing is refused, yet {}",
                    describe(&e)
                ));
            }
            for leak in leaks(&e) {
                failures.push(format!("member {member:?} in {ctx:?}: leaked {leak}"));
            }
            if !plain_member(member) {
                continue;
            }
            let source = fill(&live_expr(member));
            let e = ex("a.swift", &source);
            live_seen += 1;
            // A source the grammar could not read has nothing to count.
            if !matches!(e.parse_outcome, ParseOutcome::Clean) && ctx.contains("broken") {
                continue;
            }
            let got = note_count(&e);
            let want = note_count(&ex("a.swift", &fill(&live_expr("zzqq"))));
            let exempt = LIVE_PARSES_CORRECTLY.contains(ctx);
            if got != want
                || (exempt && want != 0)
                || (!exempt && want != slots && !ctx.contains("broken"))
            {
                failures.push(format!(
                    "live member {member:?} in {ctx:?}: got {got}, plain-name baseline {want}, slots {slots}\n   {}",
                    describe(&e)
                ));
            }
            for leak in leaks(&e) {
                failures.push(format!("live member {member:?} in {ctx:?}: leaked {leak}"));
            }
        }
    }
    assert!(seen > 1000 && live_seen > 1000, "{seen} / {live_seen}");
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
/// a file with notes into a file with none, across many threads. The noted file
/// is triggered by the bare-cast member that tree-sitter-swift 0.7.4 still
/// mis-parses (`(b as? Foo.foo)`, with a local `foo` hiding the name); the
/// cast-coalesce shape it used before is parsed correctly and refuses nothing.
#[test]
fn the_note_does_not_leak_between_files_under_rayon() {
    let noted = "func f() {\n    let foo = 1\n    _ = (b as? Foo.foo)\n    _ = foo\n}\n";
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
/// reach one through a path the arm does not see. Cast-coalesce probes: see
/// `COUNTED` for what 0.7.4 does with them.
const COUNTED_MORE: &[&str] = &[
    "let s = \"\\(b as? T ?? .foo(\"x \\(c as? U ?? .bar())\"))\"\n",
    "let s = \"\"\"\n  \\(b as? T ?? .foo())\n  \\(c as? U ?? .bar())\n  \"\"\"\n",
    "func f() async {\n    async let x = b as? T ?? .foo()\n    _ = await x\n}\n",
    "func f() {\n    repeat { } while (b as? T ?? .foo()) == nil\n}\n",
    "func f() {\n    outer: for v in xs { _ = b as? T ?? .foo(); continue outer }\n}\n",
    "class C {\n    static let shared = b as? T ?? .foo()\n}\n",
    "class C {\n    #if DEBUG\n    var v = b as? T ?? .foo()\n    #endif\n}\n",
    "struct S {\n    let x = b as? T ?? .foo()\n    let y = c as? U ?? .foo()\n}\n",
    "enum E {\n    case a(Int = 0)\n    static var d = b as? T ?? .foo()\n}\n",
    "typealias A = Int\nfunc f() { _ = b as? T ?? .foo() }\n",
    "func f() {\n    let g = { { { b as? T ?? .foo() }() }() }()\n}\n",
    "func f() {\n    g { h { i { _ = b as? T ?? .foo() } } }\n}\n",
    "func f() {\n    let a = [1].map { x in b as? T ?? .foo() }.filter { y in c as? U ?? .foo() }\n}\n",
    "func f() {\n    let a = xs.map { $0 as? T ?? .foo() }\n}\n",
    "func f() {\n    let a = xs.map { ($0 as? T)?.foo() ?? .bar() }\n}\n",
    "func f() {\n    let a = b as? T ?? .foo() as? U ?? .bar()\n}\n",
    "func f() {\n    let a = (b as? T ?? .foo()) as? U ?? .bar()\n}\n",
    "func f() {\n    let a = b as? T ?? (.foo())\n}\n",
    "func f() {\n    let a = b as? T ?? (x.foo())\n}\n",
    "func f() {\n    let a: T = b as? T ?? T.foo()\n}\n",
    "func f() {\n    let a: [T] = b as? [T] ?? [].foo()\n}\n",
];

/// Rewritten for tree-sitter-swift 0.7.4, like the table above: nested,
/// interpolated and unusual contexts hold cast-coalesce probes that used to be
/// refused and are now parsed correctly (no note, no leak, equal to the twin).
#[test]
fn every_refused_member_is_one_note_in_nested_and_unusual_contexts() {
    let mut failures = Vec::new();
    let twinned = COUNTED_MORE
        .iter()
        .filter(|source| judge_cast_coalesce(source, &mut failures))
        .count();
    assert!(
        twinned >= COUNTED_MORE.len() - 6,
        "only {twinned} of {} probes had a cast to parenthesise",
        COUNTED_MORE.len()
    );
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
