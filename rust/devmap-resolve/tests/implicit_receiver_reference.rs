//! X47 — an implicit receiver dispatches the same way in reference position as
//! it does in call position.
//!
//! X42 taught the **call** ladder that `self.m()` names a member of the type
//! the call is written inside. The **reference** ladder got no such rung, and
//! `resolve_member_reference` documents its two as "typed receiver" and
//! "imported receiver" — neither of which `self` is. So `func=self.helper`,
//! `callbacks.append(self.on_done)` and every other place a method is passed
//! *as a value* resolved to nothing.
//!
//! That is the shape X42 was written about, in the direction that costs most:
//! a method reached only through a callback registration has no caller in the
//! graph, and a symbol with no callers is what the dead-code pass reports.
//!
//! Measured on this repository at 8b2020f: of the 2,668 `generation_unresolved`
//! rows whose receiver is `self`, `cls` or `this`, **2,642 are `Name`
//! references and 26 are calls** — the call half was fixed by X42 and the
//! reference half, 99 % of the rows, was not.
//!
//! **What this is not.** The brief that led here asked whether attribute reads
//! on `self` should be *sites* at all. They already are not counted as defects:
//! they carry `UnresolvedClass::UninferredReceiver`, which is documented as a
//! structural limit of a syntax-directed extractor — "the receiver exists but
//! could not be typed" — and is printed as expected, not as something to act
//! on. `self.name_of_a_plain_field` stays there, and must: dropping the row
//! would remove the one record that the ladder ran and could not attribute.
//! What was missing was not a filter but a rung.

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, Extraction};
use devmap_resolve::model::{ResolutionKind, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions)
}

fn edges_from<'a>(
    result: &'a ResolutionResult,
    source_symbol: &str,
    target_name: &str,
) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_symbol == source_symbol
                && edge
                    .target_symbol
                    .rsplit(['.', ':'])
                    .next()
                    .is_some_and(|tail| tail == target_name)
        })
        .collect()
}

fn kind_of(edge: &ResolvedEdge) -> ResolutionKind {
    edge.resolution
        .as_deref()
        .map(devmap_resolve::model::Resolution::kind)
        .expect("an edge the resolver built carries its own evidence")
}

/// `self.on_done` handed to a registrar. The method is used, and only here.
const CALLBACK: &str = "\
class Service:
    def on_done(self, result):
        return result

    def register(self, bus):
        bus.subscribe(self.on_done)
";

#[test]
fn a_method_passed_as_a_value_through_self_names_that_method() {
    let result = resolve(&[("svc.py", CALLBACK)]);

    let edges = edges_from(&result, "svc.py::Service.register", "on_done");
    assert_eq!(
        edges.len(),
        1,
        "`self.on_done` is a member of `Service`, which is the type `register` \
         is written inside — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        kind_of(edges[0]),
        ResolutionKind::ReceiverType,
        "`self` inside `class Service` *is* a `Service`; that is the evidence, \
         and it is the same evidence the call ladder uses"
    );
    assert_eq!(edges[0].confidence, Confidence::DETERMINISTIC);
}

/// The inherited case. A subclass passing `self.helper` names the base's
/// declaration, which is where the method actually lives.
#[test]
fn an_inherited_method_passed_through_self_names_the_base() {
    const HERITAGE: &str = "\
class Base:
    def helper(self, row):
        return row


class Child(Base):
    def wire(self, bus):
        bus.subscribe(self.helper)
";
    let result = resolve(&[("h.py", HERITAGE)]);

    let edges = edges_from(&result, "h.py::Child.wire", "helper");
    assert_eq!(
        edges.len(),
        1,
        "`self.helper` in a `Child(Base)` reaches `Base.helper` — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert_eq!(kind_of(edges[0]), ResolutionKind::ReceiverType);
}

/// The refusal that keeps this a rung and not a widening: an implicit receiver
/// names a *member*, so a module-level function of the same name is not a
/// candidate — the same rule X42 applied to calls, and for the same reason. A
/// free function that gains a caller it does not have is a free function the
/// dead-code pass stops reporting.
#[test]
fn a_module_level_function_is_not_a_member_of_the_enclosing_type() {
    const FREE_FUNCTION: &str = "\
def on_done(result):
    return result


class Service:
    def register(self, bus):
        bus.subscribe(self.on_done)
";
    let result = resolve(&[("svc.py", FREE_FUNCTION)]);

    let edges = edges_from(&result, "svc.py::Service.register", "on_done");
    assert!(
        edges.is_empty(),
        "`self.on_done` cannot reach a module-level `on_done`; binding it \
         hands that function a caller it does not have — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}

/// A plain field read stays exactly where it was: recorded, and in the tier
/// that means "the receiver exists and could not be typed" rather than the tier
/// that means "probable defect". The row is the evidence that the ladder ran.
#[test]
fn a_plain_attribute_read_is_still_recorded_and_still_not_a_defect() {
    const FIELD: &str = "\
class Service:
    def __init__(self):
        self.count = 0

    def report(self):
        return self.count
";
    let result = resolve(&[("svc.py", FIELD)]);

    let row = result
        .unresolved
        .iter()
        .find(|row| row.source_symbol == "svc.py::Service.report" && row.callee_name == "count")
        .expect("an attribute read the ladder could not attribute is still recorded");
    assert_eq!(
        row.class.label(),
        "uninferred_receiver",
        "a field is not a member the ladder can name, and that is a structural \
         limit rather than a defect"
    );
}

/// A scope that says what `self` is keeps the answer it wrote down. The
/// ordering X42 settled for calls holds here too: written evidence in this very
/// scope outranks the enclosing type's default.
#[test]
fn a_scope_that_rebinds_the_receiver_keeps_what_it_wrote() {
    const REBOUND: &str = "\
class Other:
    def on_done(self, result):
        return result


class Service:
    def on_done(self, result):
        return result

    def register(self, bus):
        this = Other()
        bus.subscribe(this.on_done)
";
    let result = resolve(&[("svc.py", REBOUND)]);

    let edges = edges_from(&result, "svc.py::Service.register", "on_done");
    assert_eq!(
        edges.len(),
        1,
        "`this = Other()` states what `this` is — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert!(
        edges[0].target_symbol.contains("Other"),
        "the binding this scope wrote outranks the enclosing type: {:?}",
        edges[0].target_symbol
    );
}

/// The abstention. Two supertypes at one level declaring the name is an
/// ambiguity the language's MRO resolves and this resolver does not.
#[test]
fn a_diamond_reaching_two_declarations_abstains() {
    const DIAMOND: &str = "\
class Left:
    def helper(self, row):
        return row


class Right:
    def helper(self, row):
        return row


class Child(Left, Right):
    def wire(self, bus):
        bus.subscribe(self.helper)
";
    let result = resolve(&[("d.py", DIAMOND)]);

    let edges = edges_from(&result, "d.py::Child.wire", "helper");
    assert!(
        edges.is_empty(),
        "two supertypes declare `helper`; picking one is picking by input \
         order — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}

/// The same shape in the **call** ladder. X42 put the implicit-receiver rung
/// ahead of rung 2c, so `self.m()` reaches the enclosing type's `m` where one
/// exists — but where the type declares no `m` at all, 2c still admits a
/// `self.` receiver and matches the bare name against every symbol the file
/// declares. A module-level function is not a member of any class, so this is
/// the fabricated-caller defect surviving in the one case X42 did not reach.
#[test]
fn a_module_level_function_is_not_reachable_by_a_self_call_either() {
    const FREE_FUNCTION_CALL: &str = "\
def on_done(result):
    return result


class Service:
    def register(self):
        return self.on_done(1)
";
    let result = resolve(&[("svc.py", FREE_FUNCTION_CALL)]);

    let edges = edges_from(&result, "svc.py::Service.register", "on_done");
    assert!(
        edges.is_empty(),
        "`self.on_done()` cannot reach a module-level `on_done`; the free \
         function gets a caller it does not have, and is shielded from the \
         dead-code pass by it — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}
