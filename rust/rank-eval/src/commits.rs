//! Commit-subject query generator. Automatic and reproducible: the same
//! repository, revision, corpus and corpus text give the same JSONL.
//!
//! A commit becomes a query when
//!
//! * it is not a merge and its conventional type is fix, feat, perf or refactor;
//! * the code files it touched, after dropping tests, fixtures, docs, lockfiles
//!   and generated output, number one to three, and every one of them still
//!   exists at the revision and is in the corpus;
//! * its subject, with `type(scope):` removed, has at least three words;
//! * that text occurs nowhere in the corpus (a verbatim hit would turn the test
//!   into string matching); and
//! * no earlier commit already produced the same text.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use crate::corpus::{has_code_suffix, is_excluded_ground_truth};

pub const TYPES: [&str; 4] = ["fix", "feat", "perf", "refactor"];
pub const MIN_WORDS: usize = 3;

/// Strip `type(scope)!:` and return the text when the type is wanted.
pub fn strip_subject(subject: &str) -> Option<String> {
    let (head, rest) = subject.split_once(':')?;
    let head = head.trim_end_matches('!');
    let kind = head.split('(').next()?.trim().to_ascii_lowercase();
    if !TYPES.contains(&kind.as_str()) {
        return None;
    }
    if head.contains('(') && !head.ends_with(')') {
        return None;
    }
    let text = rest.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[derive(Debug, PartialEq)]
pub struct Candidate {
    pub sha: String,
    pub subject: String,
    pub files: Vec<String>,
}

/// Parse `git log --name-only --format=%x1e%H%x1f%s` output.
pub fn parse_log(text: &str) -> Vec<Candidate> {
    text.split('\u{1e}')
        .filter(|rec| !rec.trim().is_empty())
        .filter_map(|rec| {
            let (head, files) = rec.split_once('\n').unwrap_or((rec, ""));
            let (sha, subject) = head.split_once('\u{1f}')?;
            Some(Candidate {
                sha: sha.trim().to_string(),
                subject: subject.trim().to_string(),
                files: files
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}

/// Why a commit did not become a query. Counted and reported, never silent.
#[derive(Default, Debug)]
pub struct Drops {
    pub not_wanted_type: usize,
    pub no_code_files: usize,
    pub too_many_files: usize,
    pub file_missing_at_rev: usize,
    pub file_not_in_corpus: usize,
    pub too_few_words: usize,
    pub verbatim_in_corpus: usize,
    pub duplicate: usize,
}

pub struct Query {
    pub id: String,
    pub query: String,
    pub relevant: Vec<String>,
    pub source: String,
}

pub fn select(
    commits: &[Candidate],
    exists_at_rev: &HashSet<String>,
    corpus: &HashSet<String>,
    corpus_text_contains: &dyn Fn(&str) -> bool,
) -> (Vec<Query>, Drops) {
    let mut drops = Drops::default();
    let mut seen_text: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for c in commits {
        let Some(text) = strip_subject(&c.subject) else {
            drops.not_wanted_type += 1;
            continue;
        };
        let mut code: Vec<&String> = c
            .files
            .iter()
            .filter(|f| has_code_suffix(f) && !is_excluded_ground_truth(f))
            .collect();
        code.sort();
        code.dedup();
        if code.is_empty() {
            drops.no_code_files += 1;
            continue;
        }
        if code.len() > 3 {
            drops.too_many_files += 1;
            continue;
        }
        if code.iter().any(|f| !exists_at_rev.contains(*f)) {
            drops.file_missing_at_rev += 1;
            continue;
        }
        if code.iter().any(|f| !corpus.contains(*f)) {
            drops.file_not_in_corpus += 1;
            continue;
        }
        if text.split_whitespace().count() < MIN_WORDS {
            drops.too_few_words += 1;
            continue;
        }
        if corpus_text_contains(&text) {
            drops.verbatim_in_corpus += 1;
            continue;
        }
        if !seen_text.insert(text.to_ascii_lowercase()) {
            drops.duplicate += 1;
            continue;
        }
        out.push(Query {
            id: format!("c{:04}", out.len() + 1),
            query: text,
            relevant: code.into_iter().cloned().collect(),
            source: format!("commit:{}", &c.sha[..12.min(c.sha.len())]),
        });
    }
    (out, drops)
}

pub fn git_log(repo: &Path, rev: &str) -> Result<Vec<Candidate>, String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "log",
            "--no-merges",
            "-M",
            "--name-only",
            "--format=%x1e%H%x1f%s",
            rev,
        ])
        .output()
        .map_err(|e| format!("git log: {e}"))?;
    if !out.status.success() {
        return Err(format!("git log failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let text = String::from_utf8(out.stdout).map_err(|_| "git log output is not utf-8")?;
    Ok(parse_log(&text))
}

pub fn tree_files(repo: &Path, rev: &str) -> Result<HashSet<String>, String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["ls-tree", "-r", "--name-only", rev])
        .output()
        .map_err(|e| format!("git ls-tree: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-tree failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_and_scope_are_stripped() {
        assert_eq!(strip_subject("fix(devmap-store): handle torn pages"), Some("handle torn pages".into()));
        assert_eq!(strip_subject("feat!: drop the old flag"), Some("drop the old flag".into()));
        assert_eq!(strip_subject("Perf(dc-grep)!: skip empty files"), Some("skip empty files".into()));
        assert_eq!(strip_subject("refactor: split the walker"), Some("split the walker".into()));
    }

    #[test]
    fn unwanted_types_and_unprefixed_subjects_are_refused() {
        for s in ["docs: x y z", "chore(x): a b c", "test: a b c", "wip: a b c", "style: a b c",
                  "Merge branch 'x'", "fixing things: a b c", "fix(unclosed: a b c", "fix:"] {
            assert_eq!(strip_subject(s), None, "{s}");
        }
    }

    #[test]
    fn log_parsing_keeps_files_with_their_commit() {
        let text = "\u{1e}aaa\u{1f}fix(a): one two three\n\nsrc/a.rs\nsrc/b.rs\n\u{1e}bbb\u{1f}feat: x y z\n\nsrc/c.go\n";
        let got = parse_log(text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].files, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(got[1].sha, "bbb");
    }

    fn cand(sha: &str, subject: &str, files: &[&str]) -> Candidate {
        Candidate { sha: sha.repeat(12), subject: subject.into(), files: files.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn selection_applies_each_rule_and_counts_the_drops() {
        let exists: HashSet<String> = ["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs", "src/e.rs"]
            .iter().map(|s| s.to_string()).collect();
        let corpus: HashSet<String> = exists.iter().filter(|p| *p != "src/e.rs").cloned().collect();
        let verbatim = |t: &str| t == "this text is in the corpus";
        let commits = vec![
            cand("1", "fix(x): tidy the retry loop", &["src/a.rs", "tests/t.rs", "docs/d.md"]),
            cand("2", "docs: words here please", &["src/a.rs"]),
            cand("3", "fix: only tests changed here", &["tests/t.rs"]),
            cand("4", "fix: four files at once now", &["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"]),
            cand("5", "fix: file deleted since then", &["src/gone.rs"]),
            cand("6", "fix: file outside the corpus", &["src/e.rs"]),
            cand("7", "fix: two words", &["src/a.rs"]),
            cand("8", "fix: this text is in the corpus", &["src/a.rs"]),
            cand("9", "feat(y): Tidy the retry loop", &["src/b.rs"]),
        ];
        let (q, d) = select(&commits, &exists, &corpus, &verbatim);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].query, "tidy the retry loop");
        assert_eq!(q[0].relevant, vec!["src/a.rs"]);
        assert_eq!(
            (d.not_wanted_type, d.no_code_files, d.too_many_files, d.file_missing_at_rev,
             d.file_not_in_corpus, d.too_few_words, d.verbatim_in_corpus, d.duplicate),
            (1, 1, 1, 1, 1, 1, 1, 1)
        );
    }
}
