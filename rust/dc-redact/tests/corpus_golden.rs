//! The redacted output of the secret corpus, byte for byte.
//!
//! Every row of the three labelled secret corpora in `dc-verify` is run through
//! `redact_secrets`, and the result is compared with `corpus_golden.tsv`. A
//! change that alters what any corpus line redacts to — a longer marker, a
//! different kept prefix, a span that grew or shrank — shows up as a diff of
//! that file, which a reviewer reads line by line. The scan that decides
//! whether a line holds a secret is gated elsewhere; this pins what is written
//! out in its place.
//!
//! Regenerate after an intended change with
//! `DC_REDACT_BLESS=1 cargo test -p dc-redact --test corpus_golden`, then read
//! the diff of `corpus_golden.tsv`.
//!
//! The golden holds three tab-separated columns: the corpus file, the corpus
//! line, and its redacted output. A line may itself hold tabs (an indented
//! source line), so `\\`, tab and newline are written as `\\\\`, `\\t` and `\\n`.

use dc_redact::redact_secrets;

const CORPORA: [(&str, &str); 3] = [
    ("secrets.tsv", "../dc-verify/tests/corpus/secrets.tsv"),
    (
        "secrets_holdout.tsv",
        "../dc-verify/tests/corpus/secrets_holdout.tsv",
    ),
    (
        "secrets_holdout_2.tsv",
        "../dc-verify/tests/corpus/secrets_holdout_2.tsv",
    ),
];

const GOLDEN: &str = "tests/corpus_golden.tsv";

fn path(relative: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

/// `file<TAB>line<TAB>redacted` for every data row of every corpus.
fn computed() -> Vec<String> {
    let mut rows = Vec::new();
    for (name, relative) in CORPORA {
        let raw = std::fs::read_to_string(path(relative))
            .unwrap_or_else(|e| panic!("cannot read corpus {relative}: {e}"));
        for line in raw.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let content = line
                .splitn(4, '\t')
                .nth(3)
                .unwrap_or_else(|| panic!("{name}: row without four columns: {line}"));
            let redacted = redact_secrets(content);
            rows.push(format!(
                "{name}\t{}\t{}",
                escape(content),
                escape(&redacted)
            ));
        }
    }
    rows
}

#[test]
fn corpus_redaction_matches_the_committed_golden() {
    let rows = computed();
    assert!(
        rows.len() > 200,
        "the corpora shrank to {} rows",
        rows.len()
    );
    if std::env::var_os("DC_REDACT_BLESS").is_some() {
        std::fs::write(path(GOLDEN), rows.join("\n") + "\n").expect("write the golden");
        return;
    }
    let golden = std::fs::read_to_string(path(GOLDEN)).expect("read the golden");
    let golden: Vec<&str> = golden.lines().collect();
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(
            golden.get(i).copied(),
            Some(row.as_str()),
            "golden row {} differs; regenerate with DC_REDACT_BLESS=1 and review the diff",
            i + 1
        );
    }
    assert_eq!(
        golden.len(),
        rows.len(),
        "the golden has rows the corpora lack"
    );
}
