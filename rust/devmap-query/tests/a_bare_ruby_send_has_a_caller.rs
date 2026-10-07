//! `impact` on a Ruby method called as a bare send finds its caller.
//!
//! Measured 2026-10-06 on this exact corpus: `impact CtlRb.bare_helper`
//! answered `total: 0, resolution: Available, walk_incomplete: None` — a
//! confident zero for a method with a caller — because `def entry;
//! bare_helper; end` parses as a plain `identifier` and the Ruby call
//! extractor claimed only `call` nodes. Ruby declares the CALLS capability, so
//! the call-blind caveat could not fire either. The parenthesised form was
//! answered correctly, which is what made the zero look trustworthy.
//!
//! The local read of a same-named variable is the opposite control: Ruby reads
//! the local, so `shadowed` has no caller and must still answer zero.

use devmap_extract::extract_file;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const CONTROL: &str = "class CtlRb
  def entry
    bare_helper
  end

  def paren_entry
    paren_helper()
  end

  def entry_local
    shadowed = 1
    shadowed
  end

  def bare_helper
    1
  end

  def paren_helper
    2
  end

  def shadowed
    3
  end
end
";

/// The Homebrew shape: `to_s` reads its own `attr_reader :reason`, and the
/// corpus's only `def reason` is in an unrelated class. Before the bare send
/// was refused for attributes, `impact Denylist.reason` named `Err.to_s` as a
/// caller at 0.9 confidence.
const ATTRIBUTE: &str = "class Err
  attr_reader :reason
  def to_s
    reason
  end
end
";
const UNRELATED: &str = "class Denylist
  def reason
    1
  end
end
";

fn store() -> Store {
    store_of(&[("ctl.rb", CONTROL)])
}

fn store_of(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn callers(store: &Store, target: &str) -> (u32, Vec<String>) {
    let answer = StoreQueryEngine::new(store)
        .impact(Request {
            query: target.to_string(),
            token_budget: 8_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .unwrap();
    assert!(
        answer.walk_incomplete.is_none(),
        "{target}: a one-file corpus walks completely, got {:?}",
        answer.walk_incomplete
    );
    let mut sources: Vec<String> = answer
        .items
        .iter()
        .map(|edge| edge.source_symbol.clone())
        .collect();
    sources.sort();
    (answer.total, sources)
}

#[test]
fn a_bare_send_is_a_caller_of_the_method_it_names() {
    let store = store();
    assert_eq!(
        callers(&store, "CtlRb.bare_helper"),
        (1, vec!["ctl.rb::CtlRb.entry".to_string()])
    );
}

#[test]
fn the_parenthesised_control_is_unchanged() {
    let store = store();
    assert_eq!(
        callers(&store, "CtlRb.paren_helper"),
        (1, vec!["ctl.rb::CtlRb.paren_entry".to_string()])
    );
}

#[test]
fn an_attribute_read_is_not_a_caller_of_a_namesake_elsewhere() {
    let store = store_of(&[("err.rb", ATTRIBUTE), ("denylist.rb", UNRELATED)]);
    assert_eq!(callers(&store, "Denylist.reason"), (0, Vec::new()));
}

#[test]
fn a_local_read_of_the_same_name_is_not_a_caller() {
    let store = store();
    assert_eq!(callers(&store, "CtlRb.shadowed"), (0, Vec::new()));
}
