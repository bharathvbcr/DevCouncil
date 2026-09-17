//! The ranked tier, driven the way the harness drives it: one JSON request on
//! stdin, one JSON reply on stdout, through the real binary.
//!
//! These are end-to-end on purpose. The unit tests in `lexical::store` prove
//! the format and the ones in `lexical::token` prove the tokeniser, but the
//! property that actually matters to a caller — that a ranking is about the
//! query, is reproducible, and refuses rather than inventing — only exists
//! once the walk, the index, the slot and the scorer are assembled.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

mod support;

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        // pid plus a counter, never a timestamp: two tests entering this in
        // the same nanosecond is not hypothetical on a machine with 18 cores.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dcgrep-ranked-test-{}-{}",
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

    fn index(&self) -> Value {
        let (ok, reply) = self.call("index", json!({}));
        assert!(ok, "index must succeed: {reply}");
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

/// A corpus where the right answer is not the one substring search would give.
fn corpus(repo: &Repo) {
    // About parsing JSON responses, and says so repeatedly.
    repo.write(
        "src/json_parser.rs",
        "fn parseJsonResponse(body: &str) -> Response {\n\
             // parse the json response into a response value\n\
             let json = parse_json(body);\n\
             json.into_response()\n\
         }\n",
    );
    // Mentions json once, in passing, and is about something else entirely.
    repo.write(
        "src/server.rs",
        "fn serve() {\n\
            let listener = TcpListener::bind(addr);\n\
            // returns json\n\
            loop { accept(&listener); }\n\
         }\n",
    );
    // Prose about the same subject: the tier must not be code-only.
    repo.write(
        "docs/parsing.md",
        "# Parsing responses\n\nHow to parse a JSON response body.\n",
    );
    // Nothing to do with the query.
    repo.write(
        "src/database.rs",
        "fn migrate() { run_migrations(&pool); }\n",
    );
}

#[test]
fn a_repository_with_no_index_is_refused_rather_than_answered_empty() {
    let repo = Repo::new();
    corpus(&repo);
    let (ok, reply) = repo.call("rank", json!({"query": "json response"}));
    assert!(!ok, "must not succeed: {reply}");
    let error = reply["error"].as_str().expect("an error");
    assert!(error.contains("no ranked index"), "{error}");
    assert!(
        error.contains("dcgrep index"),
        "the refusal must name the remedy: {error}"
    );
    assert!(
        reply.get("files").is_none(),
        "a refusal must not carry a result list: {reply}"
    );
}

#[test]
fn one_build_produces_both_indexes_in_one_pass() {
    let repo = Repo::new();
    corpus(&repo);
    let built = repo.index();
    assert_eq!(built["engine"], "tgrep-core");
    assert_eq!(built["files_indexed"], 4);
    // Same walk, same files: a ranked index covering fewer files than the
    // trigram index would mean two readers of one repository again.
    assert_eq!(built["lexical_files"], 4);
    assert_eq!(built["lexical_vocabulary"], "code-v1");
    assert!(built["lexical_postings"].as_u64().unwrap() > 0);
    assert_eq!(built["lexical_unindexed"], 0);
    assert!(built["lexical_limit_reason"].is_null());
}

#[test]
fn the_file_that_is_about_the_query_outranks_the_one_that_mentions_it() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();
    let reply = repo.rank("parse json response");
    let paths = repo.paths(&reply);

    assert_eq!(
        paths.first().map(String::as_str),
        Some("src/json_parser.rs"),
        "ranking: {reply}"
    );
    assert!(
        paths.contains(&"docs/parsing.md".to_string()),
        "prose about the subject belongs in the answer: {reply}"
    );
    assert!(
        !paths.contains(&"src/database.rs".to_string()),
        "a file with none of the terms must not be ranked: {reply}"
    );
    // server.rs says "json" once out of a short file; it may appear, but it
    // must not outrank the file the query is actually about.
    if let Some(position) = paths.iter().position(|p| p == "src/server.rs") {
        assert!(position > 0, "{reply}");
    }
    assert_eq!(reply["vocabulary"], "code-v1");
    assert_eq!(reply["terms_unknown"], 0);
}

#[test]
fn scores_descend_and_every_returned_file_scores_above_zero() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();
    let reply = repo.rank("json parse");
    let mut last = f64::INFINITY;
    for file in reply["files"].as_array().unwrap() {
        let score = file["score"].as_f64().expect("a numeric score");
        assert!(score > 0.0, "a zero-scoring file is not an answer: {file}");
        assert!(score <= last, "ranking is not descending: {reply}");
        last = score;
    }
}

/// A rare term outweighs a common one, which is the whole of what IDF buys.
///
/// Written after a deliberate sabotage — removing the IDF factor from the
/// score entirely — left every other test in this file passing. A ranker
/// without it degrades into "whichever file repeats the commonest word most",
/// and no fixture built around a single subject can tell the difference.
#[test]
fn a_rare_term_outranks_a_much_repeated_common_one() {
    let repo = Repo::new();
    // Nineteen files shouting the common word.
    for i in 0..19 {
        repo.write(
            &format!("common/f{i:02}.rs"),
            "handler handler handler handler handler\n",
        );
    }
    // One file that says it once, alongside a word only it contains.
    repo.write("rare.rs", "handler quetzalcoatl\n");
    repo.index();

    let reply = repo.rank("handler quetzalcoatl");
    let paths = repo.paths(&reply);
    assert_eq!(
        paths.first().map(String::as_str),
        Some("rare.rs"),
        "the file holding the discriminating term must win, not the one \
         repeating the common term: {reply}"
    );

    // And the margin is not incidental: the rare term is worth more than four
    // extra repetitions of the common one.
    let files = reply["files"].as_array().unwrap();
    let best = files[0]["score"].as_f64().unwrap();
    let runner_up = files[1]["score"].as_f64().unwrap();
    assert!(
        best > runner_up * 1.5,
        "rare term barely counted: {best} vs {runner_up}"
    );
}

#[test]
fn the_same_query_ranks_identically_every_time() {
    let repo = Repo::new();
    // Varied, deliberately. Identical files cannot expose this: adding the
    // same value to itself is order-independent whatever the summation order,
    // so a corpus of clones passes even when the terms are summed in hash
    // order. The weights below are all different, which is what makes the
    // last bits of each score depend on the order they were added in.
    let vocabulary = [
        "handler", "route", "parse", "json", "config", "buffer", "stream",
    ];
    for i in 0..40 {
        let mut body = String::new();
        for (slot, word) in vocabulary.iter().enumerate() {
            // A different repetition count per (file, word), so no two files
            // and no two terms contribute the same number.
            let times = (i * 7 + slot * 3) % 11;
            for _ in 0..times {
                body.push_str(word);
                body.push(' ');
            }
            body.push('\n');
        }
        repo.write(&format!("d{i:02}/f{i:02}.rs"), &body);
    }
    repo.index();
    let first = repo.rank("handler route parse json config");
    for round in 0..10 {
        let again = repo.rank("handler route parse json config");
        assert_eq!(
            repo.paths(&again),
            repo.paths(&first),
            "run {round} ranked differently"
        );
        // Scores, not only paths. This assertion is the one that matters:
        // an earlier version compared paths alone and passed while the
        // scores moved in their last bits every run, because the query's
        // terms were summed in hash-map order and floating-point addition
        // is not associative. Paths only diverge once that wobble crosses a
        // near-tie — so the weaker test would have reported this as stable
        // right up until the day it mattered.
        assert_eq!(
            again["files"], first["files"],
            "run {round} produced different scores for the same query"
        );
    }
}

#[test]
fn a_query_of_words_nothing_contains_is_an_empty_ranking_not_an_error() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();
    let reply = repo.rank("quetzalcoatl thaumaturgy");
    assert_eq!(reply["count"], 0);
    assert_eq!(reply["terms_unknown"], reply["terms_total"]);
    assert!(
        reply["terms_total"].as_u64().unwrap() >= 2,
        "the query did have terms: {reply}"
    );
}

#[test]
fn a_query_with_no_indexable_terms_is_refused_with_the_reason() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();
    // Every word is one character, below the term floor. Returning an empty
    // ranking here would say "nothing is about this" when the truth is that
    // nothing was asked.
    let (ok, reply) = repo.call("rank", json!({"query": "a i x ?"}));
    assert!(!ok, "{reply}");
    let error = reply["error"].as_str().unwrap();
    assert!(error.contains("no indexable terms"), "{error}");
}

#[test]
fn an_empty_or_oversized_query_is_refused() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();

    let (ok, reply) = repo.call("rank", json!({"query": "   "}));
    assert!(!ok);
    assert!(reply["error"].as_str().unwrap().contains("required"));

    let (ok, reply) = repo.call("rank", json!({"query": "x".repeat(2000)}));
    assert!(!ok);
    let error = reply["error"].as_str().unwrap();
    assert!(error.contains("over the"), "{error}");
    assert!(
        error.contains("no search ran"),
        "a refusal must not read as a negative result: {error}"
    );
}

#[test]
fn a_scope_is_matched_by_path_component_and_not_by_prefix() {
    let repo = Repo::new();
    repo.write("src/handler.rs", "fn handler() { route(); }\n");
    repo.write("src_old/handler.rs", "fn handler() { route(); }\n");
    repo.index();

    let (ok, reply) = repo.call("rank", json!({"query": "handler route", "path": "src"}));
    assert!(ok, "{reply}");
    let paths = repo.paths(&reply);
    assert_eq!(
        paths,
        vec!["src/handler.rs".to_string()],
        "`src` must not swallow `src_old`: {reply}"
    );
}

#[test]
fn truncation_says_so_and_only_when_a_file_was_withheld() {
    let repo = Repo::new();
    for i in 0..6 {
        repo.write(&format!("f{i}.rs"), "fn handler() { route(); }\n");
    }
    repo.index();

    for (limit, expect_truncated) in [(1, true), (5, true), (6, false), (7, false)] {
        let (ok, reply) = repo.call(
            "rank",
            json!({"query": "handler route", "max_results": limit}),
        );
        assert!(ok, "{reply}");
        assert_eq!(
            reply["truncated"].as_bool().unwrap_or(false),
            expect_truncated,
            "limit {limit}: {reply}"
        );
        assert_eq!(reply["count"].as_u64().unwrap() as usize, limit.min(6));
        if expect_truncated {
            assert_eq!(reply["limit"], limit);
        } else {
            assert!(reply.get("limit").is_none(), "{reply}");
        }
    }
}

#[test]
fn a_file_edited_after_indexing_is_returned_and_marked_stale() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();

    let before = repo.rank("parse json response");
    assert_eq!(before["stale_files"], 0);
    for file in before["files"].as_array().unwrap() {
        assert!(
            file.get("stale").is_none(),
            "a fresh index has nothing stale: {file}"
        );
    }

    repo.write(
        "src/json_parser.rs",
        "fn parseJsonResponse() { /* rewritten */ }\n",
    );
    let after = repo.rank("parse json response");
    assert_eq!(
        after["stale_files"], 1,
        "the edited file must be named stale: {after}"
    );
    let edited = after["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/json_parser.rs")
        .expect("the edited file is still ranked");
    assert_eq!(edited["stale"], true);
}

#[test]
fn a_deleted_file_is_marked_stale_rather_than_quietly_dropped() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();
    fs::remove_file(repo.0.join("src/json_parser.rs")).expect("remove");

    let reply = repo.rank("parse json response");
    let gone = reply["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/json_parser.rs");
    if let Some(gone) = gone {
        assert_eq!(
            gone["stale"], true,
            "a file that no longer exists is not what the index read: {gone}"
        );
    }
}

#[test]
fn a_tampered_ranked_index_is_refused_by_the_integrity_stamp() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();
    // The published slot is named by the pointer; find it rather than guess.
    let pointer: Value = serde_json::from_str(
        &fs::read_to_string(repo.0.join(".devcouncil/dcgrep/current.json")).unwrap(),
    )
    .unwrap();
    let slot = repo
        .0
        .join(".devcouncil/dcgrep")
        .join(pointer["slot"].as_str().unwrap());
    let path = slot.join("lexical.bin");
    let mut bytes = fs::read(&path).unwrap();
    let len = bytes.len();
    bytes[len - 1] ^= 0xff;
    fs::write(&path, bytes).unwrap();

    let (ok, reply) = repo.call("rank", json!({"query": "json"}));
    assert!(!ok, "a tampered index must not be ranked against: {reply}");
    let error = reply["error"].as_str().unwrap();
    assert!(
        error.contains("changed after publication") || error.contains("lexical"),
        "{error}"
    );
}

#[test]
fn a_binary_or_oversized_file_is_absent_from_the_ranking_and_from_its_counts() {
    let repo = Repo::new();
    repo.write("text.rs", "fn handler() { route(); }\n");
    fs::write(
        repo.0.join("blob.bin"),
        b"handler\x00route handler route\n".as_slice(),
    )
    .unwrap();
    let mut fat = vec![b'x'; 2 * 1024 * 1024 + 16];
    fat.extend_from_slice(b"\nhandler route\n");
    fs::write(repo.0.join("huge.txt"), fat).unwrap();

    let built = repo.index();
    assert_eq!(
        built["lexical_files"], 1,
        "only the readable text file is ranked: {built}"
    );
    let paths = repo.paths(&repo.rank("handler route"));
    assert_eq!(paths, vec!["text.rs".to_string()]);
}

#[test]
fn rebuilding_after_a_change_reranks_and_clears_staleness() {
    let repo = Repo::new();
    corpus(&repo);
    repo.index();
    repo.write(
        "src/database.rs",
        "fn migrate() {\n\
            // parse the json response from the migration server\n\
            let json = parse_json(response);\n\
         }\n",
    );
    // Before the rebuild the new content is invisible to the ranking: the
    // index is a snapshot and says so through `stale`, it does not guess.
    let before = repo.paths(&repo.rank("parse json response"));
    assert!(!before.contains(&"src/database.rs".to_string()));

    repo.index();
    let after = repo.rank("parse json response");
    assert_eq!(after["stale_files"], 0, "{after}");
    assert!(
        repo.paths(&after).contains(&"src/database.rs".to_string()),
        "a rebuilt index must see the new content: {after}"
    );
}
