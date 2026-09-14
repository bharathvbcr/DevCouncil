//! A typed field types the receiver that names it.
//!
//! The consuming half of `langfields`. `declared_types` has always been read by
//! [`field_type_on`] for `h.engine.tick()`, but a language with an implicit
//! `self` writes the field bare — `lease.revalidate()` inside a method means
//! `self.lease` — and that spelling reached the file-wide map, which knows
//! nothing about which type a name belongs to. `member_declared_type` asks the
//! enclosing type first.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

fn calls_from(files: &[(&str, &str)], source: &str) -> Vec<String> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges
        .into_iter()
        .filter(|edge| edge.source_symbol == source && format!("{:?}", edge.edge_kind) == "Calls")
        .map(|edge| edge.target_symbol)
        .collect()
}

/// A bare field name inside a method is `self.field`, and dispatches on it.
#[test]
fn an_implicit_self_field_dispatches_on_its_declared_type() {
    for (path, source) in [
        (
            "a.swift",
            "final class Req {\n\
             \x20   func zzrun() {}\n\
             }\n\
             final class Holder {\n\
             \x20   let lease: Req\n\
             \x20   func go() {\n\
             \x20       lease.zzrun()\n\
             \x20   }\n\
             }\n",
        ),
        (
            "a.kt",
            "class Req {\n\
             \x20   fun zzrun() {}\n\
             }\n\
             class Holder {\n\
             \x20   val lease: Req = x\n\
             \x20   fun go() {\n\
             \x20       lease.zzrun()\n\
             \x20   }\n\
             }\n",
        ),
    ] {
        let targets = calls_from(&[(path, source)], &format!("{path}::Holder.go"));
        assert!(
            targets.iter().any(|target| target.ends_with("Req.zzrun")),
            "{path}: a bare field name is `self.field`, got {targets:?}"
        );
    }
}

/// An explicit `self.field` reaches the same answer.
#[test]
fn an_explicit_self_field_dispatches_the_same_way() {
    let source = "final class Req {\n\
         \x20   func zzrun() {}\n\
         }\n\
         final class Holder {\n\
         \x20   let lease: Req\n\
         \x20   func go() {\n\
         \x20       self.lease.zzrun()\n\
         \x20   }\n\
         }\n";
    let targets = calls_from(&[("a.swift", source)], "a.swift::Holder.go");
    assert!(
        targets.iter().any(|target| target.ends_with("Req.zzrun")),
        "got {targets:?}"
    );
}

/// A namesake field on a *sibling* type in the same file answers nothing.
///
/// The precision `member_declared_type` buys: `lease` inside `Holder.go` is
/// `Holder`'s field, and `Other`'s field of the same name is a different value.
/// Before this rung the only key available was file-wide, which cannot tell
/// them apart and would answer with whichever won the merge.
#[test]
fn a_sibling_types_namesake_field_does_not_answer() {
    let source = "final class Req {\n\
         \x20   func zzrun() {}\n\
         }\n\
         final class Wrong {\n\
         \x20   func zzrun() {}\n\
         }\n\
         final class Other {\n\
         \x20   let lease: Wrong\n\
         }\n\
         final class Holder {\n\
         \x20   let lease: Req\n\
         \x20   func go() {\n\
         \x20       lease.zzrun()\n\
         \x20   }\n\
         }\n";
    let targets = calls_from(&[("a.swift", source)], "a.swift::Holder.go");
    assert!(
        !targets.iter().any(|target| target.ends_with("Wrong.zzrun")),
        "`lease` in Holder is Holder's field, not Other's, got {targets:?}"
    );
}

/// A local shadows the field it shares a name with.
///
/// The ordering `receiver_type_for` already had, and which the new rung must
/// not disturb: a scope's own bindings are asked before the enclosing type's
/// members, in every language that lets both exist.
#[test]
fn a_local_shadows_the_field_of_the_same_name() {
    let source = "final class Req {\n\
         \x20   func zzrun() {}\n\
         }\n\
         final class Local {\n\
         \x20   func zzrun() {}\n\
         }\n\
         final class Holder {\n\
         \x20   let lease: Req\n\
         \x20   func go() {\n\
         \x20       let lease: Local = Local()\n\
         \x20       lease.zzrun()\n\
         \x20   }\n\
         }\n";
    let targets = calls_from(&[("a.swift", source)], "a.swift::Holder.go");
    assert!(
        !targets.iter().any(|target| target.ends_with("Req.zzrun")),
        "the method's own `lease` is a Local, not the field, got {targets:?}"
    );
}

/// A field typed by a collection types nothing, so the call abstains.
#[test]
fn a_collection_field_dispatches_on_nothing() {
    let source = "final class Req {\n\
         \x20   func zzrun() {}\n\
         }\n\
         final class Holder {\n\
         \x20   let leases: [Req]\n\
         \x20   func go() {\n\
         \x20       leases.zzrun()\n\
         \x20   }\n\
         }\n";
    let targets = calls_from(&[("a.swift", source)], "a.swift::Holder.go");
    assert!(
        targets.is_empty(),
        "`leases` is an Array, and Array is not Req, got {targets:?}"
    );
}

/// Two declarations of one name in a file disagree, and the resolver abstains.
///
/// Characterises a trade-off rather than a bug. `bind_receiver` withdraws a
/// file-wide key two declarations disagree about, and reading field types in
/// every grammar means more declarations reach that map — so a call previously
/// typed by an unopposed *local* can become `UninferredReceiver` once a
/// same-named member exists.
///
/// Measured on a 306-file Swift corpus: this cost exactly one edge
/// (`ActiveSaveRequest.resolveCancellation`) and no dead-code finding, against
/// 157 edges gained and a 13-symbol regression closed. Suppressing the
/// file-wide write for members was built and measured as the alternative and
/// cost 266 edges — the abstention is the cheaper honesty.
#[test]
fn two_declarations_of_one_name_abstain_rather_than_guess() {
    let source = "final class Req {\n\
         \x20   func zzrun() {}\n\
         }\n\
         struct Plan {\n\
         \x20   var handle: Unrelated { Unrelated() }\n\
         }\n\
         final class Ledger {\n\
         \x20   func go() {\n\
         \x20       let handle: Req = Req()\n\
         \x20       handle.zzrun()\n\
         \x20   }\n\
         }\n";
    let targets = calls_from(&[("a.swift", source)], "a.swift::Ledger.go");
    // The scope's own typed binding is authoritative and still wins here; the
    // abstention only bites where no scoped evidence exists at all.
    assert!(
        targets.iter().any(|target| target.ends_with("Req.zzrun")),
        "a local with a written type outranks any file-wide disagreement, got {targets:?}"
    );
}
