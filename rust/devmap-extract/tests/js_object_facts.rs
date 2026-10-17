//! How a JS/TS module-scope object is built, as the extractor records it:
//! members in source order, spreads, `Object.assign` sources, aliases, and an
//! `Unknown` step wherever the source cannot be read.

use devmap_extract::extract_file;
use devmap_extract::model::{JsObjectFact, JsObjectPart};

fn facts(path: &str, source: &str) -> Vec<JsObjectFact> {
    extract_file(path, source).js_objects
}

fn declared<'a>(all: &'a [JsObjectFact], name: &str) -> &'a JsObjectFact {
    all.iter()
        .find(|fact| fact.declaration && fact.name == name)
        .unwrap_or_else(|| panic!("no declaration of {name} in {all:?}"))
}

fn spread(name: &str) -> JsObjectPart {
    JsObjectPart::Spread(name.to_string())
}

#[test]
fn a_literal_records_its_steps_in_source_order() {
    let all = facts(
        "a.ts",
        "export const w = { ...core, info() {}, run: () => 1, clear: clearLogs, shorthand, n: make(), ...late };\n",
    );
    assert_eq!(
        declared(&all, "w").parts,
        vec![
            spread("core"),
            JsObjectPart::Inline("info".into()),
            JsObjectPart::Inline("run".into()),
            JsObjectPart::Forward {
                key: "clear".into(),
                value: "clearLogs".into()
            },
            JsObjectPart::Forward {
                key: "shorthand".into(),
                value: "shorthand".into()
            },
            JsObjectPart::Opaque("n".into()),
            spread("late"),
        ]
    );
    assert_eq!(declared(&all, "w").alias_of, None);
}

#[test]
fn an_alias_and_an_object_assign_record_their_source() {
    let all = facts(
        "a.ts",
        "const a = core;\nconst b = ns.core;\nconst c = Object.assign({}, core, { x() {} });\nconst d = Object.assign(core, other);\n",
    );
    assert_eq!(declared(&all, "a").parts, vec![spread("core")]);
    assert_eq!(declared(&all, "a").alias_of.as_deref(), Some("core"));
    assert_eq!(declared(&all, "b").parts, vec![spread("ns.core")]);
    assert_eq!(
        declared(&all, "c").parts,
        vec![spread("core"), JsObjectPart::Opaque("x".into())],
        "a function inside a nested literal is no symbol of the const"
    );
    assert_eq!(declared(&all, "c").alias_of, None);
    assert_eq!(
        declared(&all, "d").parts,
        vec![spread("core"), spread("other")]
    );
    assert_eq!(declared(&all, "d").alias_of.as_deref(), Some("core"));
    // `Object.assign(core, other)` also writes into `core`.
    assert!(
        all.iter()
            .any(|f| !f.declaration && f.name == "core" && f.parts == vec![spread("other")])
    );
}

#[test]
fn what_cannot_be_read_is_unknown_not_skipped() {
    let all = facts(
        "a.ts",
        "const w = { ...make(), [k]: 1, ...(a ? b : c) };\nconst v = Object.assign({}, ...list);\n",
    );
    assert_eq!(
        declared(&all, "w").parts,
        vec![
            JsObjectPart::Unknown,
            JsObjectPart::Unknown,
            JsObjectPart::Unknown
        ]
    );
    assert_eq!(declared(&all, "v").parts, vec![JsObjectPart::Unknown]);
}

#[test]
fn a_mutation_in_a_callable_is_marked() {
    let all = facts(
        "a.ts",
        "const w = {};\nObject.assign(w, core);\nfunction patch() { Object.assign(w, other); }\n",
    );
    let mutations: Vec<_> = all.iter().filter(|f| !f.declaration).collect();
    assert_eq!(mutations.len(), 2);
    assert!(!mutations[0].in_callable && mutations[1].in_callable);
}

#[test]
fn only_a_module_scope_const_is_recorded() {
    let all = facts(
        "a.ts",
        "let a = { ...core };\nvar b = core;\nfunction f() { const c = { ...core }; return c; }\nconst { d } = core;\n",
    );
    assert!(all.is_empty(), "got {all:?}");
    assert!(facts("a.py", "w = {**core}\n").is_empty());
}
