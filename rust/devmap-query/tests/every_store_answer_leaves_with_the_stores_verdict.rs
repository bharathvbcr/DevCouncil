//! Guard: no public `StoreQueryEngine` answer leaves without the store's verdict.
//!
//! `StoreQueryEngine::exit` stamps the store's freshness onto every `Response`
//! an answer holds. Two things keep that true when a method is added later:
//!
//! 1. The public methods that return a `Response`, or a report built of them,
//!    are enumerated from `engine.rs` itself. Each one must leave through
//!    `self.exit(` or be a one-line delegate to another such method, and each
//!    must be reached by the sweep that drives the engine.
//! 2. That sweep runs on a store whose working tree has changed since the
//!    build, so the store's verdict is a real "stale", and no `Response` it
//!    returns, nested or refused, may say "not checked".

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use devmap_query::StoreQueryEngine;

use super::refusal_freshness_attack::{expected, fixture, sweep};

const ENGINE: &str = include_str!("../src/engine.rs");

/// Return types that hold a `Response` (or are one).
const ANSWER_TYPES: &[&str] = &[
    "Response<",
    "LayeredImpact",
    "Vec<Neighbors>",
    "ExploreReport",
    "AffectedTestsReport",
    "CloneReport",
    "SkeletonReport",
    "EvidencePack",
    "PreviewReport",
];

/// `pub fn name(...) -> anyhow::Result<T> { body }` of the store engine's impl,
/// as `(name, return type text, body text)`.
fn public_answers() -> Vec<(String, String, String)> {
    let start = ENGINE
        .find("impl<'a> StoreQueryEngine<'a> {")
        .expect("the store engine's impl block");
    let text = &ENGINE[start..];
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(rel) = text[at..].find("\n    pub fn ") {
        let sig = at + rel + "\n    pub fn ".len();
        // The impl block ends at the first line that closes at column zero.
        if text[..sig].contains("\n}\n") {
            break;
        }
        let name: String = text[sig..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let arrow = text[sig..].find(") -> ").map(|i| sig + i + 5);
        let open = arrow.and_then(|a| text[a..].find("{\n").map(|i| a + i));
        let (Some(arrow), Some(open)) = (arrow, open) else {
            at = sig;
            continue;
        };
        let returns = text[arrow..open].trim().to_string();
        let mut depth = 0usize;
        let mut end = open;
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        if ANSWER_TYPES.iter().any(|t| returns.contains(t)) {
            found.push((name, returns, text[open + 1..end].to_string()));
        }
        at = end;
    }
    found
}

#[test]
fn every_public_answer_method_leaves_through_the_one_exit() {
    let answers = public_answers();
    assert!(
        answers.len() >= 20,
        "the scan found only {}: {:?}",
        answers.len(),
        answers.iter().map(|a| &a.0).collect::<Vec<_>>()
    );
    let names: BTreeSet<&str> = answers.iter().map(|a| a.0.as_str()).collect();
    for (name, returns, body) in &answers {
        let trimmed = body.trim();
        let delegates = names.iter().any(|other| {
            *other != name
                && trimmed.starts_with(&format!("self.{other}("))
                && trimmed.matches(';').count() == 0
        });
        assert!(
            body.contains("self.exit(") || delegates,
            "`{name}` returns {returns} but neither leaves through `self.exit(` nor delegates \
             to a public answer method that does"
        );
    }
}

/// Method name of a sweep label: `impact("a")` -> `impact`, `clones.groups` ->
/// `clones`, `neighbors_at_rung([..]).x.callers` -> `neighbors_at_rung`.
fn method_of(label: &str) -> &str {
    label
        .split(|c| c == '(' || c == '.')
        .next()
        .unwrap_or(label)
}

#[test]
fn the_sweep_reaches_every_public_answer_method() {
    let answers = public_answers();
    let fx = fixture("guard-reach", None);
    let engine = StoreQueryEngine::new(&fx.store);
    let (seen, errored) = sweep(&engine);
    let mut driven: BTreeSet<String> = seen
        .iter()
        .map(|(label, _)| method_of(label).to_string())
        .chain(errored.iter().map(|e| method_of(e).to_string()))
        .collect();
    // A delegate is reached when its target is driven through it: the sweep
    // calls `ask`, which is `ask_scoped`, which is the wrapped body.
    let delegation: BTreeMap<&str, Vec<&str>> = answers
        .iter()
        .map(|(name, _, body)| {
            let targets = answers
                .iter()
                .filter(|(other, _, _)| other != name && body.contains(&format!("self.{other}(")))
                .map(|(other, _, _)| other.as_str())
                .collect();
            (name.as_str(), targets)
        })
        .collect();
    loop {
        let before = driven.len();
        for (name, targets) in &delegation {
            if driven.contains(*name) {
                driven.extend(targets.iter().map(|t| t.to_string()));
            }
        }
        if driven.len() == before {
            break;
        }
    }
    let missed: Vec<&str> = answers
        .iter()
        .map(|a| a.0.as_str())
        .filter(|name| !driven.contains(*name))
        .collect();
    assert!(
        missed.is_empty(),
        "public answer methods the sweep never drives (add them to `sweep` in \
         refusal_freshness_attack.rs): {missed:?}"
    );
}

#[test]
fn no_part_of_any_answer_says_not_checked_beside_a_stale_verdict() {
    let fx = fixture("guard-stale", None);
    fx.store.status("t").unwrap();
    fs::write(fx.root.join("chain.py"), "def a():\n    return 2\n").unwrap();
    fs::write(
        fx.root.join("brand_new_file.py"),
        "def z():\n    return 0\n",
    )
    .unwrap();
    let status = fx.store.status("t").unwrap();
    assert_eq!(status.source_freshness, Some(false), "{status:?}");
    let want = expected(&fx.store);
    assert_eq!(want.fresh, Some(false), "{want:?}");
    let (seen, _) = sweep(&StoreQueryEngine::new(&fx.store));
    assert!(seen.len() > 60, "sweep too thin: {}", seen.len());
    let bad: Vec<String> = seen
        .iter()
        .filter(|(_, got)| {
            got.fresh != Some(false)
                || got
                    .reason
                    .as_deref()
                    .unwrap_or_default()
                    .contains("not checked")
        })
        .map(|(label, got)| format!("{label}: {got:?}"))
        .collect();
    assert!(
        bad.is_empty(),
        "{} parts lost the verdict:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
