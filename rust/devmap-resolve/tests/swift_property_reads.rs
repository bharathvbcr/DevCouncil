//! A Swift property read binds through its receiver or through `self`, and
//! never through its bare name (ft-6a46dddd).
//!
//! Before extraction schema v97 a stored or computed property was no symbol, so
//! every read was a `no_namesake` ledger row. Making them symbols opens the
//! opposite risk — the bare-name rungs (SameFile by name, the module-wide name
//! index, UniqueGlobal) binding `x.isPaused` to whichever property shares the
//! spelling — so each test below pins one half of that boundary.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// Every use edge (a read or a call) a symbol ending in `caller` makes, as
/// `(target, kind)`.
fn uses_from(result: &ResolutionResult, caller: &str) -> Vec<(String, ResolutionKind)> {
    result
        .edges
        .iter()
        .filter(|edge: &&ResolvedEdge| {
            matches!(edge.edge_kind, EdgeKind::References | EdgeKind::Calls)
                && edge.source_symbol.ends_with(caller)
        })
        .map(|edge| {
            (
                edge.target_symbol.clone(),
                edge.resolution
                    .as_deref()
                    .map(Resolution::kind)
                    .expect("evidence"),
            )
        })
        .collect()
}

fn reaches(uses: &[(String, ResolutionKind)], target: &str) -> bool {
    uses.iter().any(|(symbol, _)| symbol.ends_with(target))
}

const STATE: &str = "struct Playback {\n    var isPaused: Bool\n    var count: Int\n    func bump() -> Int { count + 1 }\n    func viaSelf() -> Int { self.count }\n}\nstruct Item { var isCompleted: Bool }\n";

#[test]
fn a_typed_receiver_reads_the_property_of_its_type() {
    let result = resolve(&[
        ("Sources/M/State.swift", STATE),
        (
            "Sources/M/Use.swift",
            "func flag(_ p: Playback) -> Bool { p.isPaused }\nfunc local() -> Bool {\n    let p = Playback(isPaused: true, count: 1)\n    return p.isPaused\n}\n",
        ),
    ]);
    for caller in ["flag", "local"] {
        let uses = uses_from(&result, caller);
        assert!(reaches(&uses, "Playback.isPaused"), "{caller}: {uses:?}");
        assert!(
            uses.iter()
                .filter(|(symbol, _)| symbol.ends_with("Playback.isPaused"))
                .all(|(_, kind)| *kind == ResolutionKind::ReceiverType),
            "{caller}: a typed receiver is ReceiverType evidence, {uses:?}"
        );
    }
}

#[test]
fn a_closure_parameter_over_a_typed_array_reads_the_element_property() {
    let result = resolve(&[
        ("Sources/M/State.swift", STATE),
        (
            "Sources/M/Use.swift",
            "func any(_ items: [Item]) -> Bool { items.contains { $0.isCompleted } }\n",
        ),
    ]);
    let uses = uses_from(&result, "any");
    assert!(reaches(&uses, "Item.isCompleted"), "{uses:?}");
}

#[test]
fn self_and_an_implicit_self_read_the_enclosing_types_property() {
    let result = resolve(&[("Sources/M/State.swift", STATE)]);
    for caller in ["Playback.bump", "Playback.viaSelf"] {
        let uses = uses_from(&result, caller);
        assert!(reaches(&uses, "Playback.count"), "{caller}: {uses:?}");
    }
}

#[test]
fn a_property_read_on_an_untyped_receiver_never_binds_by_its_name() {
    let result = resolve(&[
        ("Sources/M/State.swift", STATE),
        (
            "Sources/M/Use.swift",
            "func mystery() -> Bool {\n    let u = make()\n    return u.isPaused\n}\nfunc param(_ x: Elsewhere) -> Bool { x.isPaused }\n",
        ),
    ]);
    for caller in ["mystery", "param"] {
        let uses = uses_from(&result, caller);
        assert!(
            !reaches(&uses, "Playback.isPaused"),
            "{caller}: the one property named isPaused is not evidence about u or x: {uses:?}"
        );
    }
}

#[test]
fn a_bare_name_in_another_file_or_a_bare_call_never_reaches_a_property() {
    let result = resolve(&[
        ("Sources/M/State.swift", STATE),
        (
            "Sources/M/Use.swift",
            "func readBare() -> Int { count }\nfunc callBare() -> Int { count() }\n",
        ),
    ]);
    for caller in ["readBare", "callBare"] {
        let uses = uses_from(&result, caller);
        assert!(
            !reaches(&uses, "Playback.count"),
            "{caller}: neither the module-wide name index nor UniqueGlobal may bind a property: {uses:?}"
        );
    }
}

#[test]
fn a_same_named_property_of_another_type_is_not_bound() {
    let result = resolve(&[(
        "Sources/M/Ids.swift",
        "struct A { let id: Int }\nstruct B { let id: Int }\nfunc readA(_ a: A) -> Int { a.id }\nfunc readB(_ b: B) -> Int { b.id }\n",
    )]);
    let a = uses_from(&result, "readA");
    assert!(reaches(&a, "A.id") && !reaches(&a, "B.id"), "{a:?}");
    let b = uses_from(&result, "readB");
    assert!(reaches(&b, "B.id") && !reaches(&b, "A.id"), "{b:?}");
}

#[test]
fn an_argument_label_and_a_leading_dot_member_are_not_property_reads() {
    let result = resolve(&[(
        "Sources/M/Labels.swift",
        "enum Axis { case size }\nstruct Tally {\n    var total: Int\n    var size: Int\n    func make() -> Tally { Tally(total: 1, size: 2) }\n    func pick() -> Axis { .size }\n}\n",
    )]);
    for caller in ["Tally.make", "Tally.pick"] {
        let uses = uses_from(&result, caller);
        assert!(
            !reaches(&uses, "Tally.total") && !reaches(&uses, "Tally.size"),
            "{caller}: {uses:?}"
        );
    }
}
