//! A Ruby attribute is a method: `attr_reader :tap` declares `tap`, a bare send
//! of it binds to that declaration, and an attribute inherited from a
//! superclass or mixed in from a module is found through the ancestor chain —
//! never at `UniqueGlobal` to a namesake in another class (ft-6c5d9e9e; the
//! Homebrew shape: `CaskInvalidError#to_s` reading `reason`, declared by
//! `AbstractCaskErrorWithToken`, bound to `Denylist.reason`).

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, SymbolKind};
use devmap_resolve::model::{ResolutionKind, ResolutionResult};
use devmap_resolve::Resolver;

const SOURCE: &str = "class Cask\n  def self.tap\n    1\n  end\n\n  def self.reason\n    2\n  end\nend\n\n\
class AbstractError\n  attr_reader :reason\nend\n\n\
class InvalidError < AbstractError\n  def to_s\n    reason.to_s\n  end\nend\n\n\
class TapError\n  attr_reader :tap\n\n  def to_s\n    tap.to_s\n  end\nend\n\n\
module Reader\n  attr_reader :name\nend\n\n\
class Mixed\n  include Reader\n\n  def show\n    name\n  end\nend\n\n\
class Named\n  attr_accessor :label\n\n  def label\n    \"x\"\n  end\n\n  def use\n    label\n  end\nend\n";

fn resolve() -> (Vec<devmap_extract::model::Extraction>, ResolutionResult) {
    let extractions = vec![extract_file("a.rb", SOURCE)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    (extractions, result)
}

fn call(result: &ResolutionResult, from: &str) -> Vec<(String, ResolutionKind)> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == from)
        .filter_map(|edge| Some((edge.target_symbol.clone(), edge.resolution.as_ref()?.kind())))
        .collect()
}

#[test]
fn an_attribute_macro_declares_a_method_per_literal_name() {
    let (extractions, _) = resolve();
    let methods: Vec<&str> = extractions[0]
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Method)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    for declared in [
        "a.rb::AbstractError.reason",
        "a.rb::TapError.tap",
        "a.rb::Reader.name",
        "a.rb::Named.label=",
    ] {
        assert!(
            methods.contains(&declared),
            "{declared} missing from {methods:?}"
        );
    }
    assert_eq!(
        methods.iter().filter(|name| **name == "a.rb::Named.label").count(),
        1,
        "an explicit `def` and an `attr_accessor` of one name are one symbol, or the join key breaks"
    );
}

#[test]
fn a_bare_send_of_the_classes_own_attribute_binds_to_it_not_a_namesake() {
    let (_, result) = resolve();
    assert_eq!(
        call(&result, "a.rb::TapError.to_s"),
        vec![("a.rb::TapError.tap".to_string(), ResolutionKind::SameFile)],
        "`tap` is this class's attribute, not `Cask.tap`"
    );
}

#[test]
fn an_inherited_or_mixed_in_attribute_resolves_through_the_ancestor_chain() {
    let (_, result) = resolve();
    assert_eq!(
        call(&result, "a.rb::InvalidError.to_s"),
        vec![(
            "a.rb::AbstractError.reason".to_string(),
            ResolutionKind::ReceiverType
        )],
        "the superclass declares `reason`; `Cask.reason` is a namesake"
    );
    assert_eq!(
        call(&result, "a.rb::Mixed.show"),
        vec![(
            "a.rb::Reader.name".to_string(),
            ResolutionKind::ReceiverType
        )],
        "`include Reader` puts its methods in the ancestor chain"
    );
}
