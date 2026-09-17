//! The learned sparse tier, end to end, through the real binary.
//!
//! No model runs here and none needs to. A learned index is two things — the
//! document weights an encoder produced offline, and the token/weight tables a
//! query is scored against — and this file supplies both by hand. That is the
//! whole point of the design: if a fixture written in a test can stand in for
//! `opensearch-neural-sparse-encoding-doc-v2-mini`, then the search path holds
//! no model, no Python and no inference, which is what makes `dcgrep` still a
//! single static binary once someone turns this on.
//!
//! What these prove is the part a parser test cannot: that the imported
//! weights actually decide the ranking, that the walk still owns which files
//! exist, and that every way of getting the two halves out of step is refused
//! loudly rather than answered with a plausible-looking empty list.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

mod support;

/// The fixture vocabulary. Ids are positions, exactly as WordPiece ids are.
///
/// `##json` is a continuation piece: BERT WordPiece only joins a fragment to
/// the one before it when the vocabulary carries the `##` spelling, so
/// `parseJson` is `parse` + `##json` and would be `[UNK]` without it. The
/// first draft of this fixture left it out, and the parity gate refused the
/// build rather than publishing an index whose queries resolved to nothing —
/// which is the entire reason the gate is there.
const VOCAB: &[&str] = &[
    "[UNK]", "parse", "json", "server", "route", "handler", "##json", "gamma",
];

/// What each token contributes to a query. Index-parallel to `VOCAB`.
const QUERY_WEIGHTS: &[f32] = &[0.0, 2.0, 3.0, 1.0, 1.0, 1.0, 2.5, 1.0];

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dcgrep-learned-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fresh fixture");
        Self(root)
    }

    fn write(&self, path: &str, contents: &str) {
        let full = self.0.join(path);
        fs::create_dir_all(full.parent().unwrap()).expect("fixture parent");
        fs::write(full, contents).expect("fixture file");
    }

    fn call(&self, command: &str, mut request: Value) -> (bool, Value) {
        request["root"] = json!(self.0);
        let (ok, stdout) =
            support::run(command, request.to_string().as_bytes(), support::CALL_BOUND);
        let reply: Value = serde_json::from_slice(&stdout).expect("one JSON reply");
        (ok, reply)
    }

    /// Writes an encoding file and builds the index from it.
    ///
    /// Beside the repository, not inside it — where a real producer writes it,
    /// and where it cannot be walked into the index it is describing.
    fn index_with(&self, encoding: &str) -> (bool, Value) {
        let path = self.0.with_extension("jsonl");
        fs::write(&path, encoding).expect("encoding fixture");
        let reply = self.call("index", json!({ "sparse": path }));
        let _ = fs::remove_file(&path);
        reply
    }

    fn rank(&self, query: &str) -> Value {
        let (ok, reply) = self.call("rank", json!({ "query": query }));
        assert!(ok, "rank must succeed: {reply}");
        reply
    }

    fn paths(&self, reply: &Value) -> Vec<String> {
        reply["files"]
            .as_array()
            .expect("files array")
            .iter()
            .map(|file| file["path"].as_str().expect("path").to_string())
            .collect()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The header line, with parity samples this build genuinely reproduces.
fn header() -> Value {
    json!({
        "schema": 1,
        "model": "fixture-encoder",
        "vocabulary": "wordpiece-30522",
        "vocab": VOCAB,
        "query_weights": QUERY_WEIGHTS,
        // Four shapes, each pinning a different tokeniser decision: a whole
        // word, a word that is also a continuation piece, a compound that
        // must split across both, and a word in neither — which is `[UNK]`
        // rather than nothing, because a dropped token and an unknown one
        // rank differently.
        "parity": [
            {"text": "parse", "ids": [1]},
            {"text": "json", "ids": [2]},
            {"text": "parseJson", "ids": [1, 6]},
            {"text": "nothingatall", "ids": [0]},
        ],
    })
}

/// One document line.
fn document(path: &str, total_terms: u32, terms: &[(u32, f32)]) -> Value {
    json!({
        "path": path,
        "total_terms": total_terms,
        "terms": terms.iter().map(|(id, w)| json!([id, w])).collect::<Vec<_>>(),
    })
}

fn encoding(header: Value, documents: &[Value]) -> String {
    let mut out = header.to_string();
    for document in documents {
        out.push('\n');
        out.push_str(&document.to_string());
    }
    out.push('\n');
    out
}

/// Three files whose *text* would rank one way and whose encoder weights rank
/// another. A lexical index that ignored the import would order them by the
/// words on disk, which is exactly the failure this corpus makes visible.
fn corpus(repo: &Repo) {
    repo.write("src/alpha.rs", "fn alpha() { let x = 1; }\n");
    repo.write("src/beta.rs", "fn beta() { let y = 2; }\n");
    repo.write("src/gamma.rs", "fn gamma() { let z = 3; }\n");
}

#[test]
fn a_learned_index_publishes_and_names_its_model_and_vocabulary() {
    let repo = Repo::new();
    corpus(&repo);
    let (ok, reply) = repo.index_with(&encoding(
        header(),
        &[
            document("src/alpha.rs", 10, &[(1, 0.9), (2, 0.2)]),
            document("src/beta.rs", 10, &[(2, 0.8)]),
            document("src/gamma.rs", 10, &[(3, 0.7)]),
        ],
    ));
    assert!(ok, "index must succeed: {reply}");
    assert_eq!(reply["lexical_vocabulary"], "wordpiece-30522");
    assert_eq!(reply["lexical_model"], "fixture-encoder");
    assert_eq!(reply["lexical_files"], 3);
    // Nothing in the encoding went unused, so the field is absent rather than
    // present and zero.
    assert!(reply.get("lexical_unmatched").is_none(), "{reply}");

    let ranked = repo.rank("parse");
    assert_eq!(ranked["vocabulary"], "wordpiece-30522");
    assert_eq!(repo.paths(&ranked), vec!["src/alpha.rs"]);
}

#[test]
fn the_imported_weights_decide_the_order_not_the_text_on_disk() {
    let repo = Repo::new();
    corpus(&repo);
    // Identical files. Only the encoder distinguishes them, so any ordering
    // that comes out is the encoder's and could not have come from the text.
    for name in ["src/alpha.rs", "src/beta.rs", "src/gamma.rs"] {
        repo.write(name, "fn same() {}\n");
    }
    let (ok, reply) = repo.index_with(&encoding(
        header(),
        &[
            document("src/alpha.rs", 10, &[(2, 0.10)]),
            document("src/beta.rs", 10, &[(2, 0.90)]),
            document("src/gamma.rs", 10, &[(2, 0.50)]),
        ],
    ));
    assert!(ok, "index must succeed: {reply}");

    let ranked = repo.rank("json");
    assert_eq!(
        repo.paths(&ranked),
        vec!["src/beta.rs", "src/gamma.rs", "src/alpha.rs"],
        "the ranking must follow the encoder's weights: {ranked}"
    );
}

#[test]
fn a_query_token_the_model_weighs_at_nothing_contributes_nothing() {
    let repo = Repo::new();
    corpus(&repo);
    let (ok, reply) = repo.index_with(&encoding(
        header(),
        &[
            // alpha carries only `[UNK]`, whose query weight is 0.0.
            document("src/alpha.rs", 10, &[(0, 5.0)]),
            document("src/beta.rs", 10, &[(3, 0.1)]),
        ],
    ));
    assert!(ok, "index must succeed: {reply}");

    // `nothingatall` is out of vocabulary, so it tokenises to `[UNK]` — which
    // alpha carries with a large document weight. A ranking that multiplied
    // document weight by nothing would still put alpha first if the zero query
    // weight were being ignored.
    let ranked = repo.rank("nothingatall");
    assert!(
        repo.paths(&ranked).is_empty(),
        "a zero-weighted token must not rank anything: {ranked}"
    );
    assert_eq!(ranked["count"], 0);

    // And the tier is not simply broken: a weighted token still ranks.
    assert_eq!(repo.paths(&repo.rank("server")), vec!["src/beta.rs"]);
}

#[test]
fn a_path_the_walk_never_saw_is_dropped_and_counted_rather_than_indexed() {
    let repo = Repo::new();
    corpus(&repo);
    let (ok, reply) = repo.index_with(&encoding(
        header(),
        &[
            document("src/alpha.rs", 10, &[(2, 0.5)]),
            // Deleted since the encoder ran.
            document("src/deleted.rs", 10, &[(2, 0.9)]),
            // Never in this repository at all.
            document("../../../etc/passwd", 10, &[(2, 0.9)]),
            document("/etc/shadow", 10, &[(2, 0.9)]),
        ],
    ));
    assert!(ok, "index must succeed: {reply}");
    assert_eq!(reply["lexical_files"], 1, "only the walked file: {reply}");
    assert_eq!(reply["lexical_unmatched"], 3, "{reply}");

    // The highest-weighted documents in that encoding were the ones outside
    // the tree. None of them may appear.
    let ranked = repo.rank("json");
    assert_eq!(repo.paths(&ranked), vec!["src/alpha.rs"]);
}

#[test]
fn a_file_the_encoder_never_saw_is_searchable_and_simply_unranked() {
    let repo = Repo::new();
    corpus(&repo);
    let (ok, reply) = repo.index_with(&encoding(
        header(),
        &[document("src/alpha.rs", 10, &[(2, 0.5)])],
    ));
    assert!(ok, "index must succeed: {reply}");
    // beta and gamma were walked and trigram-indexed, but the encoding did not
    // describe them, so the ranked tier leaves them out and says how many.
    assert_eq!(reply["lexical_files"], 1, "{reply}");
    assert_eq!(reply["lexical_unindexed"], 2, "{reply}");
    assert_eq!(reply["files_indexed"], 3, "{reply}");

    // Exact search still finds them; only the ranking is partial.
    let (ok, found) = repo.call("search", json!({ "pattern": "gamma" }));
    assert!(ok, "{found}");
    assert_eq!(found["count"], 1, "{found}");
}

#[test]
fn a_tokeniser_that_disagrees_with_the_model_refuses_the_build() {
    let repo = Repo::new();
    corpus(&repo);
    let mut head = header();
    // The producer claims `parse` is token 3. This build resolves it to 1.
    head["parity"] = json!([{"text": "parse", "ids": [3]}]);
    let (ok, reply) = repo.index_with(&encoding(
        head,
        &[document("src/alpha.rs", 10, &[(1, 0.9)])],
    ));
    assert!(!ok, "a tokeniser disagreement must refuse: {reply}");
    let error = reply["error"].as_str().expect("an error");
    assert!(error.contains("disagrees with the model"), "{error}");
    assert!(error.contains("not published"), "{error}");

    // And nothing was published: the ranked tier still refuses for want of an
    // index rather than answering from a half-written one.
    let (ok, ranked) = repo.call("rank", json!({ "query": "parse" }));
    assert!(!ok, "{ranked}");
    assert!(
        ranked["error"]
            .as_str()
            .is_some_and(|e| e.contains("no ranked index")),
        "{ranked}"
    );
}

#[test]
fn an_encoding_without_parity_samples_refuses_the_build() {
    let repo = Repo::new();
    corpus(&repo);
    let mut head = header();
    head["parity"] = json!([]);
    let (ok, reply) = repo.index_with(&encoding(
        head,
        &[document("src/alpha.rs", 10, &[(1, 0.9)])],
    ));
    assert!(!ok, "an unprovable tokeniser must refuse: {reply}");
    assert!(
        reply["error"]
            .as_str()
            .is_some_and(|e| e.contains("parity samples")),
        "{reply}"
    );
}

#[test]
fn a_ranking_over_a_learned_index_is_reproducible() {
    let repo = Repo::new();
    corpus(&repo);
    // Weights that differ per file and per term, so a sum taken in a different
    // order lands on a different float. Equal weights would make this test
    // pass no matter what the scorer did with ordering.
    let (ok, reply) = repo.index_with(&encoding(
        header(),
        &[
            document("src/alpha.rs", 17, &[(1, 0.31), (2, 0.47), (3, 0.13)]),
            document("src/beta.rs", 23, &[(1, 0.29), (2, 0.41), (4, 0.19)]),
            document("src/gamma.rs", 11, &[(2, 0.37), (3, 0.23), (5, 0.07)]),
        ],
    ));
    assert!(ok, "{reply}");

    let first = repo.rank("parse json server route handler");
    for _ in 0..12 {
        let again = repo.rank("parse json server route handler");
        assert_eq!(again, first, "two identical queries answered differently");
    }
}

#[test]
fn rebuilding_without_the_encoding_returns_the_repository_to_bm25() {
    let repo = Repo::new();
    corpus(&repo);
    let (ok, reply) = repo.index_with(&encoding(
        header(),
        &[document("src/alpha.rs", 10, &[(2, 0.5)])],
    ));
    assert!(ok, "{reply}");
    assert_eq!(repo.rank("json")["vocabulary"], "wordpiece-30522");

    // No `sparse`, so the build computes its own weights — and the published
    // slot must speak only the new vocabulary. A query scored half in one term
    // space and half in the other is the failure the stored vocabulary exists
    // to make impossible.
    let (ok, reply) = repo.call("index", json!({}));
    assert!(ok, "{reply}");
    assert_eq!(reply["lexical_vocabulary"], "code-v1");
    assert!(reply.get("lexical_model").is_none(), "{reply}");

    let ranked = repo.rank("gamma");
    assert_eq!(ranked["vocabulary"], "code-v1");
    assert_eq!(repo.paths(&ranked), vec!["src/gamma.rs"], "{ranked}");
}

#[test]
fn health_advertises_both_ranked_vocabularies() {
    let (_, stdout) = support::run("health", b"", support::CALL_BOUND);
    let reply: Value = serde_json::from_slice(&stdout).expect("JSON");
    let vocabularies = reply["ranked_vocabularies"]
        .as_array()
        .expect("ranked_vocabularies");
    assert!(vocabularies.iter().any(|v| v == "code-v1"), "{reply}");
    assert!(
        vocabularies.iter().any(|v| v == "wordpiece-30522"),
        "health must not deny a capability this build has: {reply}"
    );
    let engines = reply["ranked_engines"].as_array().expect("ranked_engines");
    assert_eq!(engines.len(), vocabularies.len(), "{reply}");
}
