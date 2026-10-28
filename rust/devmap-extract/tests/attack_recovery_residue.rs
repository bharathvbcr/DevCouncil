//! Adversarial probes for `lane/recovery-residue` @ 5004f49c (main dc397d99,
//! card ft-bce92fa0, lane D). Each test states the CORRECT behaviour; a failing
//! test is a BREAK. A test marked `#[ignore = "pre-existing on main: ..."]`
//! fails on main dc397d99 as well, so the lane did not introduce it.
//!
//! Two ignored harness tests (`harness_*`) are not assertions: they dump the
//! facts of a probe set and of an arbitrary file list as JSON lines, so the same
//! module can be compiled into a copy of main and the two dumps diffed.
//!
//! Contract under test: a refusal of a member the grammar hung off a bare cast
//! must never drop a REAL member (a wrong edge is worse than an abstention, but
//! a lost non-self edge parks the lane); every mangled receiver is refused;
//! the note count equals the refusals; `rust_type_name` answering `None` drops
//! no name that used to be a real one.

use std::io::Write;
use std::time::Duration;

use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_extract::{extract_all, extract_file_with_budget, FileRef};

const BUDGET: Duration = Duration::from_secs(600);

fn ex(path: &str, source: &str) -> Extraction {
    extract_file_with_budget(path, source, BUDGET)
}

fn clean(e: &Extraction) -> bool {
    matches!(e.parse_outcome, ParseOutcome::Clean)
}

fn member_note(n: usize) -> String {
    format!("swift: refused {n} member reference(s) the grammar read off a bare cast")
}

fn member_notes(e: &Extraction) -> Vec<&str> {
    e.diagnostics
        .iter()
        .map(String::as_str)
        .filter(|d| d.contains("off a bare cast"))
        .collect()
}

/// Cast or coalesce text at the top level of a receiver. A bracketed group is
/// a real operand (a closure body, an argument list, a parenthesised cast) and
/// is not read: `xs.map { $0 as? T }` is a real receiver, `b as? T ??` is not.
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

/// Every call and reference whose receiver is cast or coalesce text.
fn leaks(e: &Extraction) -> Vec<String> {
    let mut out = Vec::new();
    for c in &e.calls {
        // An operator call (`b as? T == c`) has the real left operand as its
        // receiver, which is cast text only because the operand is a cast.
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

// ----------------------------------------------------------------- harness

const STRIP_KEYS: &[&str] = &[
    "body_signature",
    "declaration_hash",
    "span",
    "start_byte",
    "end_byte",
    "body_start",
    "body_end",
    "line",
    "header",
    "body",
    "extent",
    "raw_import",
    "recovered_decl_ranges",
    "decl_gaps",
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
    let mut imports: Vec<String> = e.imports.iter().map(|i| format!("{i:?}")).collect();
    imports.sort();
    let mut diags = e.diagnostics.clone();
    diags.sort();
    // Every other table, span-free: implicit calls, bindings, receivers, ….
    let mut rest = serde_json::to_value(e).expect("serialize");
    strip(&mut rest);
    if let serde_json::Value::Object(map) = &mut rest {
        for key in [
            "calls",
            "references",
            "symbols",
            "imports",
            "diagnostics",
            "content_hash",
            "source_code",
            "parse_outcome",
        ] {
            map.remove(key);
        }
    }
    serde_json::json!({
        "rest": rest,
        "outcome": format!("{:?}", e.parse_outcome),
        "calls": calls,
        "refs": refs,
        "syms": syms,
        "imports": imports,
        "diag": diags,
    })
}

fn write_lines(lines: &[(String, serde_json::Value)]) {
    let path = std::env::var("RR_OUT").expect("RR_OUT");
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).expect("create"));
    for (label, value) in lines {
        writeln!(out, "{}\t{}", label.replace(['\t', '\n'], " "), value).expect("write");
    }
}

#[test]
#[ignore = "harness: dumps RR_OUT for a diff against another arm"]
fn harness_dump_probes() {
    let lines: Vec<(String, serde_json::Value)> = probes()
        .into_iter()
        .map(|(label, path, source)| {
            let e = ex(&path, &source);
            let mut v = compact(&e);
            v["source"] = serde_json::Value::String(source);
            (label, v)
        })
        .collect();
    write_lines(&lines);
}

#[test]
#[ignore = "harness: dumps RR_OUT for the file list RR_LIST"]
fn harness_dump_files() {
    let list = std::fs::read_to_string(std::env::var("RR_LIST").expect("RR_LIST")).expect("list");
    let paths: Vec<&str> = list.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut lines = Vec::with_capacity(paths.len());
    for path in paths {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let e = ex(path, &source);
        lines.push((path.to_string(), compact(&e)));
    }
    write_lines(&lines);
}

// ------------------------------------------------------------------ probes

const CTX: &[(&str, &str)] = &[
    ("top", "let a = {E}\n"),
    ("func", "func f() {\n    let a = {E}\n    _ = a\n}\n"),
    ("closure", "let g = { x in\n    {E}\n}\n"),
    ("arg", "let a = f({E})\n"),
    ("ret", "func f() -> Any? {\n    return {E}\n}\n"),
    ("interp", "let a = \"\\({E})\"\n"),
    (
        "method",
        "class C {\n    func m() {\n        _ = {E}\n    }\n}\n",
    ),
];

/// Legitimate Swift: every member here is real and must keep publishing.
const LEGIT: &[&str] = &[
    "(b as? T)?.foo()",
    "(b as! T).foo()",
    "(b as T).foo()",
    "(b as T).foo",
    "(b as? T)!.foo()",
    "(b as? T)?.foo.bar()",
    "(b as? [T])?.first",
    "(b as? T.Type)?.make()",
    "(b as? any P)?.foo()",
    "(b as? Foo.Bar)?.baz()",
    "(b as? Foo.Bar.Baz)?.qux()",
    "(b as? Foo<T>.Bar)?.baz()",
    "b.foo() as? T",
    "b.foo().bar() as! T",
    "b.c.d as? T",
    "(b.foo() as? T)?.bar()",
    "xs.compactMap { $0 as? T }.map { $0.foo() }",
    "xs.compactMap { $0 as? T }.first",
    "xs.map { $0 as! T }.foo()",
    "xs.map { ($0 as? T)?.foo() }",
    "(xs as? [T])?.map { $0.foo() }",
    "h(b as? T).foo()",
    "h(b as? T)?.foo",
    "h(b as! T, c.d()).foo()",
    "h(x: b as? T).foo.bar",
    "h(b as? T) { $0.foo() }",
    "try? h(b as? T).foo()",
    "(try? h(b) as? T)?.foo()",
    "(await h() as? T)?.foo()",
    "b as? T ?? c.foo()",
    "(b as? T) ?? c.foo()",
    "(b as? T) ?? Color.red",
    "(b as? T) ?? .foo()",
    "b.foo as? T ?? c",
    "[b as? T, c.foo()]",
    "[k: b as? T, j: c.foo()]",
    "(b as? T, c.foo())",
    "(b as? T, c: d.foo())",
    "b is T ? c.foo() : d.bar()",
    "(b as? T) != nil ? c.foo() : d.bar()",
    "b as? T == nil ? c.foo() : d.bar()",
    "b as? T != nil && c.foo()",
    "b as? T != nil && c.foo().bar",
    "b as? T != nil || c.foo.bar()",
    "b as? T == c.foo()",
    "b as? T == c.d",
    "b as? T == Color.red",
    "b as? T == .red",
    "!(b as? T).isEmpty",
    "-(b as? Int)!.magnitude",
    "b as? T ? c.foo() : d.bar()",
    "b as? Foo.Type == c.foo()",
    "b as? Foo<Bar> == c.d",
    "b as! T + c.foo()",
    "b as! Int + c.foo().bar",
    "b as! Int * c.d.e",
    "\\Foo.bar",
    "\\Foo.bar.baz",
    "xs.map(\\.foo)",
    "(b as? T)?[0].foo()",
    "(b as? [String: Any])?[\"k\"]",
    "(b as? () -> Void)?()",
    "#selector(Foo.bar)",
    "#keyPath(Foo.bar)",
    "Foo.Bar.make(b as? T)",
    "Foo.Bar.make(b as? T).baz()",
    "Foo.make(b as? T)?.baz.qux",
    "Foo(b as? T).baz()",
    "Foo(b as? T).baz.qux",
    "Foo<T>(b as? T).baz()",
    "super.foo(b as? T)",
    "self.foo(b as? T).bar()",
    "self.foo(b as? T)?.bar",
    "b?.foo(c as? T).bar()",
    "b!.foo(c as! T)!.bar",
    "foo(b as? T)\n    .bar()\n    .baz()",
    "b\n    .foo()\n    .bar() as? T",
    "(b as? T)\n    .foo()",
    "b as? T\n",
];

/// Spellings the grammar misparses: the member hangs off a bare cast and must
/// be refused, never given cast text as a receiver.
const MANGLED: &[&str] = &[
    "b as? T ?? .foo()",
    "b as! T ?? .foo()",
    "b as? T ?? .foo().bar()",
    "b as? T ?? .foo().bar().baz",
    "b as? T ?? .foo.bar.baz",
    "b as? T ?? .foo()?.bar()",
    "b as? T ?? .foo()[0]",
    "b as? T ?? .foo[0].bar()",
    "b as? T ?? .foo(1)(2)",
    "b as? T ?? .foo { $0.bar() }",
    "b as? T ?? .foo { }.bar()",
    "b as? T ?? .foo(x: 1) { }",
    "b as? T ?? Color.red",
    "b as? T ?? Color.red.hash",
    "b as? T ?? Color.make().hash",
    "b as? T ?? Color.make(c.d()).hash",
    "b as? T ?? Color.make(d as? U ?? .x)",
    "try b as? T ?? .foo()",
    "try? b as? T ?? .foo()",
    "(try? b as? T ?? .foo())",
    "b as? T ?? .foo() as? U ?? .bar()",
    "b as? T ?? .foo() ?? .bar()",
    "b as? T ?? c.foo()",
    "b as? T ?? c.d.foo()",
    "b as? T ?? c.foo().bar()",
    "b as? T ?? c.foo!",
    "b as? T ?? c!.foo()",
    "b as? T ?? c?.foo()",
    "b as? T ?? self.foo()",
    "b as? T ?? h(c.foo())",
    "b as? T ?? [c.foo()]",
    "b as? T ?? (c.foo())",
    "b as? T ?? (.foo())",
    "b as? T ?? { c.foo() }()",
    "b as? [T] ?? .foo()",
    "b as? [T]? ?? .foo()",
    "b as? T.Type ?? .foo()",
    "b as? any P ?? .foo()",
    "b as? Foo.Bar ?? .foo()",
    "b as? Foo.Bar ?? Foo.Bar.zero",
    "b as? Foo<T> ?? .foo()",
    "b as? Foo<T>.Bar ?? .foo()",
    "b.c as? T ?? .foo()",
    "b?.c as? T ?? .foo()",
    "b.foo() as? T ?? .bar()",
    "h(b) as? T ?? .foo()",
    "(b as? Foo.Bar)",
    "(b as? Foo.Bar.Baz)",
    "(b as? Foo.Bar)?.baz",
    "(b as! Foo.Bar)",
    "(b as? Foo.Bar.make())",
    "(b as? Foo.Bar.make().baz)",
    "(b as? Foo.Bar<T>)",
    "(b as? Foo.Bar?)",
    "(b as? Foo.Bar!)",
    "(b as? Foo.Bar & Baz.Qux)",
    "(b as? Foo.Bar, c as? Baz.Qux)",
    "[b as? Foo.Bar]",
    "h(b as? Foo.Bar)",
    "h(x: b as? Foo.Bar)",
    "h(b as! Foo.Bar).baz()",
    "b as? Foo.Bar",
    "b as! Foo.Bar",
    "b as? Foo.Bar.Baz",
    "b as? Foo.Bar == c",
    "b as? Foo.Bar != nil",
    "b as? Foo.Bar != nil ? c : d",
    "b as? Foo.Bar ?? c",
    "b as? Foo.Bar ?? nil",
    "b as? Foo.Bar is Baz",
    "b as? Foo.Bar as? Baz.Qux",
    "b as? Foo.Bar\n    ?? .foo()",
    "b as? T ??\n    .foo()",
    "b as? T // c\n    ?? .foo()",
    "b as? T ?? /* c */ .foo()",
    "b as? T ?? .foo(/* c */)",
    "b as? T ? .foo() : .bar()",
    "b as? T ? c.foo() : d.bar()",
    "b as? T ?? ((.foo()))",
    "(b as? T ?? .foo())",
    "(b as? T ?? .foo()).bar()",
    "(b as? T ?? .foo()).bar",
    "(b as? T ?? .foo())?.bar()",
    "h(b as? T ?? .foo())",
    "h(b as? T ?? .foo()).bar()",
    "h(b as? T ?? .foo()) { $0.bar() }",
    "[b as? T ?? .foo()]",
    "[b as? T ?? .foo(), c as? U ?? .bar()]",
    "(b as? T ?? .foo(), 1)",
    "b as? T ?? .foo() == c.bar()",
    "b as? T ?? .foo() + c.bar()",
    "b as? T ?? .foo() ? c.bar() : d.baz()",
    "xs.map { $0 as? T ?? .foo() }",
    "xs.map { $0 as? T ?? .foo() }.first",
    "xs.map { $0 as? T ?? .foo() }.map { $0.bar() }",
    "xs.compactMap { $0 as? Foo.Bar }.map { $0.baz() }",
    "xs.compactMap { $0 as? Foo.Bar }.first?.baz()",
    "(xs as? [Foo.Bar])?.first",
    "(xs as? Foo.Bar<T>.Baz)",
    "b as? T.Protocol ?? .foo()",
    "b as? (T, U) ?? .foo()",
    "b as? (T) -> U ?? .foo()",
    "b as? @escaping () -> Void ?? .foo()",
];

/// Member chains through a postfix `!`: the grammar wraps the refused call in a
/// `postfix_expression`, which `swift_on_bare_cast` does not walk through, so
/// the next member is published with the cast text as its receiver.
const POSTFIX: &[&str] = &[
    "b as? T ?? .foo()!.bar()",
    "b as? T ?? .foo()!.bar",
    "b as? T ?? .foo![0].bar",
    "b as? T ?? Color.make()!.hash",
    "b as? T ?? .foo()!!.bar()",
    "b as! T ?? .foo()!.bar().baz",
    "try b as? T ?? .foo()!.bar()",
    "(b as? Foo.Bar)!.baz",
    "(b as? Foo.Bar)!.baz()",
];

/// Statement-level forms (switch/if/for/catch patterns) with real members.
const STATEMENTS: &[&str] = &[
    "switch b {\ncase let x as T:\n    x.foo()\ncase let y as Foo.Bar:\n    y.baz()\ndefault: break\n}\n",
    "switch b {\ncase let x as? T:\n    x.foo()\ndefault: break\n}\n",
    "switch b {\ncase is T: b.foo()\ncase is Foo.Bar: b.baz()\ndefault: break\n}\n",
    "switch b {\ncase let x as T where x.foo():\n    x.bar()\ndefault: break\n}\n",
    "if case let v as Foo = b { v.bar() }\n",
    "if case let v as Foo.Bar = b { v.baz() }\n",
    "if let v = b as? Foo { v.bar() }\n",
    "if let v = b as? Foo.Bar { v.baz() }\n",
    "if let v = b as? Foo, v.bar() { v.baz() }\n",
    "if let v = b as? Foo, let w = v.bar as? Baz { w.qux() }\n",
    "if let v = b as? Foo.Bar, v.baz() { }\n",
    "guard let v = b as? Foo else { return }\nv.bar()\n",
    "guard let v = b as? Foo.Bar else { return }\nv.baz()\n",
    "guard let v = b as? Foo, v.bar() else { return }\n",
    "for case let x as T in xs { x.foo() }\n",
    "for case let x as Foo.Bar in xs { x.baz() }\n",
    "for x in xs as! [T] { x.foo() }\n",
    "for x in (xs as? [T]) ?? [] { x.foo() }\n",
    "for x in xs as? [T] ?? [] { x.foo() }\n",
    "while let v = b as? Foo { v.bar() }\n",
    "do { try f() } catch let e as MyError { e.code }\n",
    "do { try f() } catch let e as MyError { print(e.code) }\n",
    "do { try f() } catch let e as Foo.Bar { print(e.code) }\n",
    "do { try f() } catch MyError.bad { g.foo() }\n",
    "do { try f() } catch is MyError { g.foo() }\n",
    "do { try f() } catch { (error as? MyError)?.recover() }\n",
    "do { try f() } catch { (error as NSError).code }\n",
    "do { try f() } catch { if let e = error as? Foo.Bar { e.baz() } }\n",
    "let x = b as? T\nx?.foo()\n",
    "let x = b as? T\n.foo\n",
    "let x = b as? T\n    .foo()\n",
    "let x = b as T\n.foo()\n",
    "var x = b as! T\nx.foo()\n",
    "let x = b as? T; let y = c.foo()\n",
    "let x = b as? T; c.foo()\n",
    "let x: T? = b as? T\nc.foo()\n",
    "let x = b as? T\nc.foo()\nd.bar()\n",
    "let x = b as? T\n.init()\n",
    "func f(_ b: Any) -> T? {\n    b as? T\n}\n",
    "func f(_ b: Any) -> T? {\n    b as? T\n    c.foo()\n}\n",
    "func f(_ b: Any) {\n    let r = b as? T\n    r?.foo()\n    c.bar()\n}\n",
    "struct S {\n    var v: T? { b as? T }\n    func m() { v?.foo() }\n}\n",
    "struct S: View {\n    var body: some View {\n        VStack {\n            Text(b as? String ?? \"\")\n            c.foo()\n        }\n    }\n}\n",
    "@ViewBuilder var body: some View {\n    if let v = b as? Foo { v.bar() }\n    c.baz()\n}\n",
    "let k = \\Foo.bar\nlet j = b as? T\n",
    "let k: KeyPath<Foo, Bar> = \\.bar\nlet j = b as? T ?? c.foo()\n",
    "let s = #selector(Foo.bar)\nlet j = b as? T\n",
    "extension Foo where Self: Bar { func m() { (self as? Baz)?.qux() } }\n",
    "extension Foo { func m() { if let x = self as? Bar { x.baz() } } }\n",
    "func f<T>(_ x: T) { if let y = x as? Foo { y.bar() } }\n",
    "func f<T>(_ x: T) { (x as? Foo)?.bar() }\n",
    "func f<T>(_ x: T) { (x as? any Foo)?.bar() }\n",
    "func f(_ x: Any) { (x as? Foo.Type)?.make() }\n",
    "func f(_ x: Any) { (x as! Foo.Type).make() }\n",
    "func f(_ x: Any) { let t = x as! Foo.Type\n t.make() }\n",
    "func f(_ x: Any) { (x as? some P)?.bar() }\n",
    "func f(_ x: Any) -> Bool { x is Foo.Bar }\n",
    "func f(_ x: Any) -> Bool { x as? Foo.Bar != nil }\n",
    "func f(_ x: Any) -> Bool { return x as? Foo.Bar != nil && g.bar() }\n",
    "func f(_ x: Any) -> Bool { return (x as? Foo.Bar) != nil && g.bar() }\n",
];

/// Damaged and valid Rust around every caller of `rust_type_name`.
const RUST_PROBES: &[&str] = &[
    "impl a:: {}\n",
    "impl a::{} {}\n",
    "impl a:: { fn m(&self) {} }\nfn f(x: S) { x.m(); }\n",
    "impl a::{} { fn m(&self) {} fn n() {} }\n",
    "impl Tr for a:: { fn m(&self) {} }\n",
    "impl a:: for S { fn m(&self) {} }\nfn f(s: S) { s.m(); }\n",
    "impl std::ops::Deref for a:: { type Target = X; fn deref(&self) -> &X { todo() } }\n",
    "impl std::ops::Deref for G { type Target = a::; fn deref(&self) -> &X { todo() } }\n",
    "impl std::ops::Deref for G { type Target = Inner; fn deref(&self) -> &Inner { todo() } }\n",
    "fn f(x: a::) { x.m(); }\n",
    "fn f(x: &a::) { x.m(); }\n",
    "fn f(x: a::b::) { x.m(); }\n",
    "fn f(x: &mut a::b::, y: S) { x.m(); y.n(); }\n",
    "fn f(x: Box<a::>) { x.m(); }\n",
    "fn f(x: &dyn a::) { x.m(); }\n",
    "fn f(x: impl a::) { x.m(); }\n",
    "fn f<T: a::>(x: T) { x.m(); }\n",
    "fn f(x: S) { x.m(); }\n",
    "fn f(x: a::S) { x.m(); }\n",
    "fn f(x: a::S<T>) { x.m(); }\n",
    "fn f(x: &'a a::S<'a, T>) { x.m(); }\n",
    "fn f(x: Vec<a::S>) { x.m(); }\n",
    "fn f(x: Rc<a::>) { x.m(); }\n",
    "fn f() { let x = a::b::{c}; x.m(); }\n",
    "fn f() { let x = a::{c}; x.m(); }\n",
    "fn f() { let x = a::b::{}; x.m(); }\n",
    "fn f() { let x = a:: {}; x.m(); }\n",
    "fn f() { let x = ::{c}; x.m(); }\n",
    "fn f() { let x = a::b::{c, d}; x.m(); }\n",
    "fn f() { let _g = a::b::{c}; }\n",
    "fn f() { let g = a::b::{c}; g.m(); drop(g); }\n",
    "fn f() { let w = Rc::new(a::b::{c}); w.m(); }\n",
    "fn f() { let w = Arc::new(Mutex::new(a::b::{c})); w.m(); }\n",
    "fn f() { let w = Rc::new(S { x: 1 }); w.m(); }\n",
    "fn f() { let w = Rc::new(a::S { x: 1 }); w.m(); }\n",
    "fn f() { let s = S { x: 1 }; s.m(); }\n",
    "fn f() { let s = a::S { x: 1 }; s.m(); }\n",
    "fn f() { let s = crate::a::S::<T> { x: 1 }; s.m(); }\n",
    "fn f() { let s = S::<T> { x: 1 }; s.m(); }\n",
    "fn f() { let s = Self { x: 1 }; s.m(); }\n",
    "fn f() { let s = a::b:: { x: 1 }; s.m(); }\n",
    "fn f() { let m: a:: = x.into(); m.n(); }\n",
    "fn f() { let m: a::S = x.into(); m.n(); }\n",
    "fn f() { let m: S = x.into(); m.n(); }\n",
    "fn f() { let m: Vec<a::> = x.into(); }\n",
    "fn f() -> Result<(), a::> { g()?; Ok(()) }\n",
    "fn f() -> Result<(), a::b::> { g()?; Ok(()) }\n",
    "fn f() -> Result<(), E> { g()?; Ok(()) }\n",
    "fn f() -> Result<(), a::E> { g()?; Ok(()) }\n",
    "fn f() -> Result<(), Box<dyn a::>> { g()?; Ok(()) }\n",
    "fn f() -> Result<(), &'static a::> { g()?; Ok(()) }\n",
    "use a::b::{self};\nfn f() { b::g(); }\n",
    "use a::b::{elf};\nfn f() { b::g(); }\n",
    "use a::b::{s};\nfn f() { s::g(); }\n",
    "use a::b::{ };\n",
    "use a::b::{ , };\n",
    "use a::b::{self as};\n",
    "use a::b::{self as c};\nfn f() { c::g(); }\n",
    "use a::{b::{self}, c};\nfn f() { b::g(); c(); }\n",
    "use a::b::{c, d::{e, f}};\nfn f() { c(); e(); f(); }\n",
    "use a::{b::,c};\n",
    "use a::b::;\n",
    "use ::{a};\n",
    "use a::*;\nfn f() { g(); }\n",
    "m!(a::b::{c});\n",
    "fn f() { m!(a::b::{c}); x.m(); }\n",
    "fn f() { vec![a::{c}]; x.m(); }\n",
    "fn f() { println!(\"{}\", a::b::{c}); x.m(); }\n",
    "fn f() { m! { a:: } x.m(); }\n",
    "macro_rules! m { ($t:ty) => { let x: $t = a::{c}; x.m(); } }\n",
    "#[derive(a::)]\nstruct S;\nfn f(s: S) { s.m(); }\n",
    "#[a::b::{c}]\nfn f() { x.m(); }\n",
    "#[cfg(a::)]\nfn f() { x.m(); }\n",
    "#[a::]\nimpl S { fn m(&self) {} }\n",
    "#[derive(Debug)]\nstruct S<T: a::> { x: T }\n",
    "struct S { x: a:: }\nfn f(s: S) { s.x.m(); }\n",
    "trait T: a:: { fn m(&self); }\n",
    "trait T { type A: a::; fn m(&self); }\n",
    "enum E { A(a::), B }\nfn f(e: E) { e.m(); }\n",
    "fn f() { let x = <a::>::new(); x.m(); }\n",
    "fn f() { let x: <a:: as T>::Out = g(); x.m(); }\n",
    "fn f() { let x = a::<>::new(); x.m(); }\n",
    "fn f() { let x = S::<>::new(); x.m(); }\n",
    "fn f() { let x = <S>::new(); x.m(); }\n",
    "fn f() { let x = a::b::{c}::d(); x.m(); }\n",
    "fn f() { match x { a::b::{c} => y.m(), _ => z.n() } }\n",
    "fn f() { if let a::b::{c} = x { y.m(); } }\n",
    "fn f() { let a::b::{c} = x; y.m(); }\n",
    "fn f() { loop { let g = a::b::{c}; g.m(); break; } }\n",
    "struct G; impl Drop for G { fn drop(&mut self) {} }\nfn f() { let g = a::b::{c}; }\n",
    "fn drop() {}\nfn f() { let g = a::b::{c}; }\n",
    "fn from() {}\nfn f() { let m: a:: = x.into(); }\n",
    "fn f() { let x = a::b::d(); x.m(); }\n",
    "fn f() { let x = b::d(); x.m(); }\n",
    "fn f() { let x = d(); x.m(); }\n",
    "fn f() { let x = a::b::{c}::d(); x.m(); }\n",
    "fn f() { let x = a::b::{c}.d(); x.m(); }\n",
    "fn f() { let x = a::b::{c}::D::new(); x.m(); }\n",
    "fn f() { let x = a::b::{c}?; x.m(); }\n",
    "fn f() { let x = a::b::{c}.unwrap(); x.m(); }\n",
    "fn f() { let x = Some(a::b::{c}); x.m(); }\n",
    "fn f() { let x = (a::b::{c}); x.m(); }\n",
    "fn f() { let x = &a::b::{c}; x.m(); }\n",
    "fn f() { let x = Box::new(a::b::{c}); x.m(); }\n",
    "fn f() { let x = Box::new(a::b::{c}).m(); }\n",
];

/// The spelling that marks the one SHADOW probe written as a raw identifier.
const RAW_IDENTIFIER: &str = "`foo bar`";

/// A refused call whose member name the references walk refuses for another
/// reason first (a local binding of that name, a reserved spelling): the
/// refusal must still be noted, because the call is gone either way.
const SHADOW: &[&str] = &[
    "func f() {\n    let foo = 1\n    _ = b as? T ?? .foo()\n    _ = foo\n}\n",
    "func f(foo: Int) {\n    _ = b as? T ?? .foo()\n}\n",
    "func f(_ foo: Int) {\n    _ = b as? T ?? .foo(1)\n}\n",
    "func f() {\n    if let foo = x { _ = b as? T ?? .foo() }\n}\n",
    "func f() {\n    for foo in xs { _ = b as? T ?? .foo() }\n}\n",
    "func f() {\n    let g = { (foo: Int) in b as? T ?? .foo() }\n}\n",
    "func f() {\n    _ = b as? T ?? .init()\n}\n",
    "func f() {\n    _ = b as? T ?? .self\n}\n",
    "func f() {\n    _ = b as? T ?? .default()\n}\n",
    "func f() {\n    _ = b as? T ?? .none\n}\n",
    "func f() {\n    _ = b as? T ?? .some(1)\n}\n",
    "func f() {\n    _ = b as? T ?? .Type\n}\n",
    "func f() {\n    _ = b as? T ?? .`class`()\n}\n",
    // A raw identifier (SE-0451, Swift 6.2: spaces inside backticks). It is valid
    // Swift, but tree-sitter-swift does not support it: the member becomes an
    // ERROR region and no call is extracted, so there is nothing to refuse or
    // note. The file is Partial and the site lies inside the error range, which
    // is the only case the test accepts without a note (see `RAW_IDENTIFIER`).
    "func f() {\n    _ = b as? T ?? .`foo bar`()\n}\n",
    "func f() {\n    _ = b as? T ?? ._foo()\n}\n",
    "func f() {\n    _ = b as? T ?? .foo(foo: 1)\n}\n",
    "func f() {\n    _ = b as? T ?? .foo($0)\n}\n",
    "func f() {\n    _ = b as? T ?? x.foo()\n}\nfunc g() {\n    _ = (b as? Foo.Bar)\n}\n",
    "func foo() {}\nfunc f() {\n    _ = b as? T ?? .foo()\n}\n",
    "struct S {\n    var foo = 1\n    func f() { _ = b as? T ?? .foo() }\n}\n",
];

fn probes() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (group, set) in [("legit", LEGIT), ("mangled", MANGLED), ("postfix", POSTFIX)] {
        for (i, expr) in set.iter().enumerate() {
            for (cname, ctx) in CTX {
                let source = ctx.replace("{E}", expr);
                out.push((
                    format!("{group}:{i}:{cname}:{expr}"),
                    "p.swift".to_string(),
                    source,
                ));
            }
        }
    }
    for (i, source) in SHADOW.iter().enumerate() {
        out.push((
            format!("shadow:{i}:{}", source.replace('\n', " ")),
            "p.swift".to_string(),
            (*source).to_string(),
        ));
    }
    for (i, source) in RUST_PROBES.iter().enumerate() {
        out.push((
            format!("rust:{i}:{}", source.lines().next().unwrap_or("")),
            "p.rs".to_string(),
            (*source).to_string(),
        ));
    }
    for (i, source) in STATEMENTS.iter().enumerate() {
        out.push((
            format!("stmt:{i}:{}", source.lines().next().unwrap_or("")),
            "p.swift".to_string(),
            (*source).to_string(),
        ));
    }
    out
}

// ------------------------------------------------------------- assertions

/// ATTACK 2. Missed refusals: every mangled spelling, in every context, must
/// publish no fact whose receiver is cast or coalesce text.
#[test]
fn no_mangled_swift_spelling_publishes_cast_text_as_a_receiver() {
    let mut failures = Vec::new();
    let mut seen = 0;
    for (label, path, source) in probes() {
        if !label.starts_with("mangled:") {
            continue;
        }
        seen += 1;
        let e = ex(&path, &source);
        for leak in leaks(&e) {
            failures.push(format!("{label}: {leak}"));
        }
    }
    assert!(seen > 500);
    assert!(
        failures.is_empty(),
        "{} leak(s) of {seen} probes:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Probes the grammar misparses even though the Swift is legitimate, so the
/// lane refuses one member of each (and notes it): a qualified type read as a
/// member of the cast (1c), `Foo.Type`, a member chain on a bare cast, and a
/// real `c.foo()` after `as? T ??`. Each is checked separately below.
const EXPECTED_REFUSALS: &[&str] = &[
    "(b as? T.Type)?.make()",
    "(b as? Foo.Bar)?.baz()",
    "(b as? Foo.Bar.Baz)?.qux()",
    "(b as? Foo<T>.Bar)?.baz()",
    "b as? T ?? c.foo()",
    "b as? Foo.Type == c.foo()",
    "let x = b as? T\n.foo\n",
    "let x = b as? T\n    .foo()\n",
    "let x = b as T\n.foo()\n",
    "let x = b as? T\n.init()\n",
    "let k: KeyPath<Foo, Bar> = \\.bar\nlet j = b as? T ?? c.foo()\n",
    "(x as? Foo.Type)?.make()",
    "(x as! Foo.Type).make()",
    "x as? Foo.Bar != nil",
    "(x as? Foo.Bar) != nil && g.bar()",
];

/// ATTACK 1. False refusals: a legitimate spelling in any context must carry
/// no note and no leak (the lane's rule fires only on a misparse).
#[test]
fn no_legitimate_swift_spelling_is_refused_or_noted() {
    let mut failures = Vec::new();
    let mut seen = 0;
    for (label, path, source) in probes() {
        if !(label.starts_with("legit:") || label.starts_with("stmt:")) {
            continue;
        }
        if EXPECTED_REFUSALS
            .iter()
            .any(|known| source.contains(known) || label.ends_with(known))
        {
            continue;
        }
        seen += 1;
        let e = ex(&path, &source);
        if !member_notes(&e).is_empty() {
            failures.push(format!("{label}: noted {:?}", member_notes(&e)));
        }
    }
    assert!(seen > 400);
    assert!(
        failures.is_empty(),
        "{} refusal(s) of {seen} probes:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// ATTACK 1, named members. The members a human wrote on a real receiver
/// survive: each pair is (source, member that must still publish).
#[test]
fn the_real_members_beside_a_cast_survive() {
    let mut failures = Vec::new();
    for (source, member) in [
        ("let a = (b as? T)?.foo()\n", "foo"),
        ("let a = (b as! T).foo()\n", "foo"),
        ("let a = (b as T).foo()\n", "foo"),
        ("let a = (b as? T)!.foo()\n", "foo"),
        ("let a = (b as? Foo.Bar)?.baz()\n", "baz"),
        ("let a = h(b as? T).foo()\n", "foo"),
        ("let a = h(b as! T).foo.bar\n", "bar"),
        (
            "let a = xs.compactMap { $0 as? T }.map { $0.foo() }\n",
            "foo",
        ),
        ("let a = xs.compactMap { $0 as? T }.first\n", "first"),
        ("let a = (b as? T.Type)?.make()\n", "make"),
        ("let a = (b as? Foo.Bar.Baz)?.qux()\n", "qux"),
        ("let a = (b as? Foo<T>.Bar)?.baz()\n", "baz"),
        ("let a = b as? Foo.Type == c.foo()\n", "foo"),
        ("let a = b as? T != nil && c.foo()\n", "foo"),
        ("let a = b.foo() as? T\n", "foo"),
        ("let a = b as? T == c.foo()\n", "foo"),
        ("let a = b as? T ? c.foo() : d.bar()\n", "bar"),
        ("let a = [b as? T, c.foo()]\n", "foo"),
        ("let a = (b as? T, c.foo())\n", "foo"),
        ("let a = Foo.make(b as? T).baz()\n", "baz"),
        ("let a = Foo(b as? T).baz()\n", "baz"),
        ("func f() {\n let x = b as? T\n c.foo()\n}\n", "foo"),
        ("for case let x as T in xs { x.foo() }\n", "foo"),
        ("if case let v as Foo = b { v.bar() }\n", "bar"),
        (
            "do { try f() } catch let e as MyError { e.code() }\n",
            "code",
        ),
    ] {
        let e = ex("a.swift", source);
        if !(has_call(&e, member) || has_ref(&e, member)) {
            failures.push(format!(
                "{source:?}: `{member}` not published (clean {}, calls {:?}, refs {:?}, diag {:?})",
                clean(&e),
                e.calls
                    .iter()
                    .map(|c| (&c.callee_name, &c.receiver_expr))
                    .collect::<Vec<_>>(),
                e.references.iter().map(|r| &r.name).collect::<Vec<_>>(),
                e.diagnostics
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3. The note equals the refusals, per file, under rayon: files with
/// a distinct number of refused members, extracted through `extract_all`.
#[test]
fn the_member_note_is_exact_per_file_under_rayon() {
    let mut sources: Vec<(String, String, usize)> = Vec::new();
    for n in 0..64usize {
        let mut source = String::new();
        for i in 0..n {
            source.push_str(&format!("let a{i} = (b as? Foo.Bar)\n"));
            source.push_str(&format!("let c{i} = d{i}.foo()\n"));
        }
        if n == 0 {
            source.push_str("let z = 1\n");
        }
        sources.push((format!("f{n}.swift"), source, n));
    }
    let refs: Vec<FileRef> = sources
        .iter()
        .map(|(path, source, _)| FileRef { path, source })
        .collect();
    let results = extract_all(&refs);
    assert_eq!(results.len(), 64);
    let mut failures = Vec::new();
    for ((path, _, n), e) in sources.iter().zip(&results) {
        let notes = member_notes(e);
        let expected: Vec<String> = if *n == 0 {
            vec![]
        } else {
            vec![member_note(*n)]
        };
        if notes != expected.iter().map(String::as_str).collect::<Vec<_>>() {
            failures.push(format!("{path}: notes {notes:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 1, characterization (HOLD). A REAL member after `as? T ??` is refused
/// with the cast's own members: `c.foo()` is `foo` on `c`, a receiver the site
/// spells, but the grammar reads `T ??` as an optional type and hangs `c.foo`
/// off the cast. The lane refuses it (main published it with the receiver
/// `b as? T ??`, which no resolver rung binds), so no edge moves; what is lost
/// is the evidence that `foo` is called. Pinned so a change either way is seen.
#[test]
fn a_real_member_after_the_coalesce_is_refused_and_noted_not_misbound() {
    for (source, member) in [
        ("let a = b as? T ?? c.foo()\n", "foo"),
        ("let a = b as? T ?? c.d.foo()\n", "foo"),
        ("let a = b as? T ?? Color.make()\n", "make"),
        ("let a = b as? T ?? Color.red\n", "red"),
    ] {
        let e = ex("a.swift", source);
        assert!(
            !has_call(&e, member) && !has_ref(&e, member),
            "{source:?}: `{member}` was published after all: {:?} {:?}",
            e.calls,
            e.references
        );
        assert!(leaks(&e).is_empty(), "{source:?}: {:?}", leaks(&e));
        assert!(!member_notes(&e).is_empty(), "{source:?}: refused unnoted");
    }
}

/// ATTACK 3. A refused call is a noted refusal even when the references walk
/// drops the member's name for another reason first: the note is raised only
/// by that walk's last arm, so a call refused in `swift_navigation_call` whose
/// name a local binding or a reserved spelling hides would vanish unrecorded.
#[test]
fn a_refused_call_is_noted_even_when_its_name_is_hidden_by_another_rule() {
    let mut failures = Vec::new();
    let mut raw_exempt = 0;
    for source in SHADOW {
        let e = ex("a.swift", source);
        for leak in leaks(&e) {
            failures.push(format!("{source:?}: leaked {leak}"));
        }
        // Either the refusal is noted, or (raw identifier only) the grammar put
        // the member site inside an error region and there was nothing to refuse.
        let site = source.find("?? .").map(|at| at + "?? ".len());
        let in_error_range = match (&e.parse_outcome, site) {
            (ParseOutcome::Partial { error_ranges, .. }, Some(site)) => error_ranges
                .iter()
                .any(|r| r.start_byte <= site && site < r.end_byte),
            _ => false,
        };
        let raw = source.contains(RAW_IDENTIFIER);
        // The error-range exemption is taken by the raw identifier alone, so a
        // regression on a valid probe cannot hide behind it.
        if raw {
            raw_exempt += 1;
            if !(member_notes(&e).is_empty() && in_error_range) {
                failures.push(format!(
                    "{source:?}: the raw identifier was expected to be a Partial file with its \
                     site in an error range and no note ({:?})",
                    e.parse_outcome
                ));
            }
        } else if member_notes(&e).is_empty() {
            failures.push(format!(
                "{source:?}: refused unnoted (calls {:?}, diag {:?}, {:?})",
                e.calls
                    .iter()
                    .map(|c| (&c.callee_name, &c.receiver_expr))
                    .collect::<Vec<_>>(),
                e.diagnostics,
                e.parse_outcome
            ));
        }
    }
    assert_eq!(raw_exempt, 1, "exactly one probe is the raw identifier");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 2. A member chain through a postfix `!` off a bare cast keeps its
/// cast text as the receiver: `swift_on_bare_cast` walks `navigation_expression`
/// and `call_expression` only, and `.foo()!` is a `postfix_expression`. The
/// refused `foo` is gone, but `.bar` is published on `b as? T ?? .foo()!`.
#[test]
#[ignore = "follow-up of ft-c0b8696f: Swift walkers (swift_on_bare_cast does not walk a postfix_expression; main published these facts too)"]
fn a_postfix_chain_off_a_bare_cast_publishes_no_cast_text_receiver() {
    let mut failures = Vec::new();
    let mut seen = 0;
    for (label, path, source) in probes() {
        if !label.starts_with("postfix:") {
            continue;
        }
        seen += 1;
        let e = ex(&path, &source);
        for leak in leaks(&e) {
            failures.push(format!("{label}: {leak}"));
        }
    }
    assert!(seen > 50);
    assert!(
        failures.is_empty(),
        "{} leak(s) of {seen} probes:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Contexts that each hold exactly one refused member (`Bar` of
/// `(b as? Foo.Bar)`). The count must be one per context: a context the walk
/// visits twice (a conditional-compilation arm, an interpolation, a result
/// builder, an `@autoclosure`) must not count its member twice.
const ONE_REFUSED_MEMBER: &[&str] = &[
    "let a = (b as? Foo.Bar)\n",
    "let a = \"\\((b as? Foo.Bar))\"\n",
    "let a = \"\"\"\n  x \\((b as? Foo.Bar))\n  \"\"\"\n",
    "func f() {\n    let a = (b as? Foo.Bar)\n}\n",
    "class C {\n    var v = (b as? Foo.Bar)\n    func m() {}\n}\n",
    "class C {\n    lazy var v = (b as? Foo.Bar)\n}\n",
    "class C {\n    var v: Int { (b as? Foo.Bar) }\n}\n",
    "struct S: View {\n    var body: some View {\n        Text(\"x\")\n        Color((b as? Foo.Bar))\n    }\n}\n",
    "#if os(iOS)\nlet a = (b as? Foo.Bar)\n#else\nlet a = 1\n#endif\n",
    "#if os(iOS)\nlet a = 1\n#else\nlet a = (b as? Foo.Bar)\n#endif\n",
    "func f() {\n    defer { _ = (b as? Foo.Bar) }\n}\n",
    "func f() {\n    let g = { () -> Any? in (b as? Foo.Bar) }\n}\n",
    "func f() {\n    g { (b as? Foo.Bar) }\n}\n",
    "func f() {\n    g(x: (b as? Foo.Bar), y: 1)\n}\n",
    "func f() {\n    if (b as? Foo.Bar) != nil { }\n}\n",
    "func f() {\n    switch (b as? Foo.Bar) { default: break }\n}\n",
    "func f() {\n    for x in [(b as? Foo.Bar)] { _ = x }\n}\n",
    "func f(_ x: Any? = (b as? Foo.Bar)) {}\n",
    "func f() {\n    @autoclosure let g = (b as? Foo.Bar)\n}\n",
    "extension E {\n    func f() { _ = (b as? Foo.Bar) }\n}\n",
    "enum E { case a\n    func f() { _ = (b as? Foo.Bar) } }\n",
    "protocol P { }\nextension P { var v: Any? { (b as? Foo.Bar) } }\n",
    "func f() async {\n    _ = await g((b as? Foo.Bar))\n}\n",
    "func f() throws {\n    _ = try g((b as? Foo.Bar))\n}\n",
    "actor A { func f() { _ = (b as? Foo.Bar) } }\n",
    "@MainActor func f() { _ = (b as? Foo.Bar) }\n",
    "func f() {\n    let a = [\"k\": (b as? Foo.Bar)]\n}\n",
    "func f() {\n    let a = (x: (b as? Foo.Bar), y: 1)\n}\n",
];

/// ATTACK 3. One refused member is one counted member, in every context.
#[test]
fn a_refused_member_is_counted_once_in_every_context() {
    let mut failures = Vec::new();
    for source in ONE_REFUSED_MEMBER {
        let e = ex("a.swift", source);
        let notes = member_notes(&e);
        if notes.is_empty() && clean(&e) {
            failures.push(format!("{source:?}: Clean and not refused at all"));
            continue;
        }
        if notes.len() > 1 || notes.iter().any(|n| *n != member_note(1)) {
            failures.push(format!("{source:?}: notes {notes:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3. A budget refusal mid-file leaves no member note and no residue
/// for the next file, at any budget.
#[test]
fn a_budget_refusal_leaves_no_member_note_and_no_residue() {
    let many: String = (0..400)
        .map(|i| format!("let a{i} = (b as? Foo.Bar)\n"))
        .collect();
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
        let notes = member_notes(&e);
        match &e.parse_outcome {
            ParseOutcome::Clean | ParseOutcome::Partial { .. } => {
                let counted = (1..=400).find(|k| notes == [member_note(*k).as_str()]);
                if notes.len() > 1 || (notes.len() == 1 && counted.is_none()) {
                    failures.push(format!("budget {budget:?}: notes {notes:?}"));
                }
            }
            other => {
                if !notes.is_empty() {
                    failures.push(format!("budget {budget:?}: {other:?} carries {notes:?}"));
                }
            }
        }
        let next = ex("next.swift", "let a = (b as? Foo.Bar)\n");
        if member_notes(&next) != [member_note(1).as_str()] {
            failures.push(format!(
                "after budget {budget:?}: next file notes {:?}",
                member_notes(&next)
            ));
        }
        let none = ex("none.swift", "let a = (b as? Foo)?.bar()\n");
        if !none.diagnostics.is_empty() {
            failures.push(format!(
                "after budget {budget:?}: a file without a site carries {:?}",
                none.diagnostics
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3. Both Swift refusal notes in one file are each exact and separate:
/// the unspelled-operator count and the bare-cast count never fold into one
/// line or into each other.
#[test]
fn the_two_swift_refusal_notes_are_each_exact_in_one_file() {
    let source =
        "let a = b as? String ?? \"x\"\nlet c = (d as? Foo.Bar)\nlet e = (f as? Foo.Bar.Baz)\n";
    let e = ex("a.swift", source);
    let unspelled: Vec<&str> = e
        .diagnostics
        .iter()
        .map(String::as_str)
        .filter(|d| d.contains("inserted with no spelling"))
        .collect();
    assert!(unspelled.len() <= 1, "{:?}", e.diagnostics);
    assert_eq!(
        member_notes(&e),
        [member_note(3).as_str()],
        "{:?}",
        e.diagnostics
    );
}

/// ATTACK 5. Determinism: the same source extracted repeatedly, alone and in a
/// shuffled batch under rayon, serializes to the same bytes, notes included.
#[test]
fn a_file_with_refused_members_extracts_to_the_same_bytes_every_time() {
    let sources: Vec<(String, String)> = (0..48usize)
        .map(|i| {
            let mut s = String::new();
            for k in 0..(i % 7 + 1) {
                s.push_str(&format!("let a{k} = b as? Color ?? .foo{k}()\n"));
                s.push_str(&format!("let c{k} = (d as? Foo.Bar.Baz{k})\n"));
            }
            (format!("f{i}.swift"), s)
        })
        .collect();
    let alone: Vec<String> = sources
        .iter()
        .map(|(p, s)| serde_json::to_string(&ex(p, s)).unwrap())
        .collect();
    for round in 0..3 {
        let mut order: Vec<usize> = (0..sources.len()).collect();
        order.rotate_left((round * 13) % sources.len());
        order.reverse();
        let refs: Vec<FileRef> = order
            .iter()
            .map(|&i| FileRef {
                path: &sources[i].0,
                source: &sources[i].1,
            })
            .collect();
        let batch = extract_all(&refs);
        for (slot, &i) in order.iter().enumerate() {
            let got = serde_json::to_string(&batch[slot]).unwrap();
            assert_eq!(
                got, alone[i],
                "round {round}: {} differs between a lone and a batched extraction",
                sources[i].0
            );
        }
    }
}

/// ATTACK 2. `swift_on_bare_cast` is bounded at 16 steps ("a chain longer than
/// that is not refused"). A member chain off a bare cast that is longer still
/// publishes its far members with cast text as the receiver, unless the
/// receiver is dropped for length. Probed at several depths. The bound counts
/// tree nodes, not hops: `.f()` is a `navigation_expression` under a
/// `call_expression`, two steps per hop, so a call chain is cut at about eight.
#[test]
#[ignore = "follow-up of ft-f1d5a16e: budget.rs (the 16-step bound of swift_on_bare_cast counts nodes, not hops; main published every member)"]
fn a_long_chain_off_a_bare_cast_publishes_no_cast_text_receiver() {
    let mut failures = Vec::new();
    for depth in [4usize, 8, 9, 15, 16, 17, 18, 24, 40, 64] {
        for member in [".f()", ".f", ".f()?"] {
            let chain = member.repeat(depth);
            let source = format!("let a = b as? T ??{chain}\n");
            let e = ex("a.swift", &source);
            let bad = leaks(&e);
            if !bad.is_empty() {
                failures.push(format!(
                    "depth {depth} `{member}`: {} leak(s), first {:?}",
                    bad.len(),
                    bad.first()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 2, found in real code (2 of 2,615 Swift files under ~/Code): a
/// nil-coalescing chain with no cast at all, `a ?? rows.map { .. }.max()`,
/// is read as `(a ?? rows.map { .. }).max()`, so `max` is published on the
/// receiver `a ?? rows.map { .. }`. The lane's contract names `??` recovery
/// text, but its predicate only recognises an `as_expression` base.
#[test]
#[ignore = "follow-up of ft-c0b8696f: Swift walkers (a `??` chain with no cast is outside swift_on_bare_cast; main published the same receiver)"]
fn a_coalesce_chain_without_a_cast_publishes_no_coalesce_text_receiver() {
    let mut failures = Vec::new();
    for source in [
        "let m = a ?? rows.map { $0.w }.max()\n",
        "let m = a ?? rows.map { $0.w }.first\n",
        "let t = x?.text ?? y.first { $0.k }?.text\n",
        "let t = a ?? b.c\n",
        "let t = a ?? b.c()\n",
        "let t = a ?? Color.make().hash\n",
    ] {
        let e = ex("a.swift", source);
        for leak in leaks(&e) {
            failures.push(format!("{source:?}: {leak}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 4. Across every damaged and valid Rust probe (the callers of
/// `rust_type_name`: constructors, implicit drop and from, try conversions,
/// typed parameters and bindings, deref impls, wrapped values), no table
/// carries an empty type name, receiver or initializer.
#[test]
fn no_rust_probe_publishes_an_empty_type_name_in_any_table() {
    let mut failures = Vec::new();
    let mut seen = 0;
    for (label, path, source) in probes() {
        if !label.starts_with("rust:") {
            continue;
        }
        seen += 1;
        let json = serde_json::to_string(&ex(&path, &source)).expect("serialize");
        for needle in [
            "\"declared_type\":\"\"",
            "\"receiver_expr\":\"\"",
            "\"initializer\":\"\"",
            "\"initializer\":\"{..}\"",
            "\"callee_name\":\"\"",
            "\"name\":\"\"",
        ] {
            if json.contains(needle) {
                failures.push(format!("{label}: {needle}"));
            }
        }
    }
    assert!(seen > 100, "only {seen} rust probes");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ATTACK 3. The member note is a diagnostic of the fresh extraction only: a
/// durable re-read carries none, as the unspelled-operator note does.
#[test]
fn the_member_note_clears_on_a_durable_reread_like_the_other() {
    let fresh = ex("a.swift", "let a = (b as? Foo.Bar)\n");
    assert_eq!(member_notes(&fresh), [member_note(1).as_str()]);
    assert!(fresh.for_durable_store().diagnostics.is_empty());
}
