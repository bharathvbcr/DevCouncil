//! The adversarial round: linear time and cancellation, links that leave the
//! repository, GitHub's slug rules, symbols that only look moved, the
//! opt-outs, and the bounds. Resolver tests build a real temporary checkout and
//! index it, so they need the parsing frontend.

use super::*;

use std::time::{Duration, Instant};

const HOSTILE_BYTES: usize = 256 * 1024;

fn link(target: &str) -> RefSpec {
    RefSpec::Link {
        target: target.to_string(),
        symbols: Vec::new(),
    }
}

fn hostile_docs() -> Vec<(&'static str, String)> {
    let repeat = |unit: &str| unit.repeat(HOSTILE_BYTES / unit.len());
    let lines =
        |unit: &str| format!("{}\n", unit.repeat(MAX_LINE_BYTES / unit.len() - 1)).repeat(64);
    vec![
        // One line, a stray backtick, then comment openers: recounted the
        // backticks before every one of them.
        (
            "backtick then comment openers",
            format!("`{}", repeat("<!--")),
        ),
        ("the same, line by line", lines("`<!--")),
        ("unmatched brackets", repeat("[")),
        ("unmatched brackets and backticks", lines("[`(")),
        ("openers of links with no end", lines("[a](")),
        ("alternating backtick runs", repeat("`a``b```c````")),
        (
            "unclosed tags in headings",
            format!("# {}\n", repeat("<a ")),
        ),
        ("entities", repeat("&#x26;&amp;")),
    ]
}

#[test]
fn a_hostile_document_is_scanned_in_linear_time() {
    for (label, doc) in hostile_docs() {
        let started = Instant::now();
        let extraction = extract_references(&doc);
        let anchors = heading_anchors(&doc, None).unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "{label}: {} bytes took {elapsed:?}",
            doc.len()
        );
        // And it still answers rather than giving up.
        let _ = (extraction, anchors);
    }
    // A megabyte of the worst of them.
    let big = format!("`{}", "<!--".repeat(1024 * 1024 / 4));
    let started = Instant::now();
    let _ = extract_references(&big);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_cancelled_scan_stops_instead_of_finishing() {
    let doc = "[a](a.md)\n".repeat(200_000);
    let cancel = Cancel::new();
    cancel.cancel();
    let started = Instant::now();
    assert!(extract_references_cancellable(&doc, Some(&cancel)).is_err());
    assert!(heading_anchors(&doc, Some(&cancel)).is_err());
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "a cancelled scan took {:?}",
        started.elapsed()
    );
    // The same document, not cancelled, is fully scanned.
    let live = Cancel::new();
    assert!(extract_references_cancellable(&doc, Some(&live)).is_ok());
}

fn anchors(text: &str) -> Vec<String> {
    heading_anchors(text, None)
        .unwrap()
        .slugs
        .into_iter()
        .collect()
}

#[test]
fn slugs_follow_github_slugger() {
    let has = |text: &str, slug: &str| {
        let found = anchors(text);
        assert!(
            found.iter().any(|s| s == slug),
            "{slug} not in {found:?} for {text:?}"
        );
    };
    // Emphasis markup is not text.
    has("## _Emphasis_ word\n", "emphasis-word");
    has("## __init__ and snake_case\n", "init-and-snake_case");
    has("## *star* **bold**\n", "star-bold");
    // Entities decode, then the symbol goes: two spaces, two hyphens.
    has("## Tom &amp; Jerry\n", "tom--jerry");
    has("## Fish &#38; Chips\n", "fish--chips");
    // Inline code, links and images are their text.
    has("## The `Store::open` call\n", "the-storeopen-call");
    has(
        "## See [the docs](x.md) and ![logo](l.png)\n",
        "see-the-docs-and-logo",
    );
    has("## Closed ##\n", "closed");
    // Marks and non-ASCII letters survive.
    has("## Crème brûlée\n", "crème-brûlée");
    // Blockquotes and list items hold headings.
    has("> ## Quoted heading\n", "quoted-heading");
    has("- ## Listed heading\n", "listed-heading");
    has("1. ## Numbered heading\n", "numbered-heading");
    has("> - ## Both\n", "both");
    // Setext.
    has("Title\n=====\n", "title");
    has("Sub head\n--------\n", "sub-head");
}

#[test]
fn a_taken_suffix_is_skipped_exactly_as_github_slugger_does() {
    assert_eq!(
        anchors("## Foo\n## Foo-1\n## Foo\n## Foo\n"),
        vec!["foo", "foo-1", "foo-2", "foo-3"]
    );
    assert_eq!(anchors("# A\n# A\n# A\n"), vec!["a", "a-1", "a-2"]);
    // A suffixed heading that comes later is itself bumped past what is taken.
    assert_eq!(
        anchors("## Foo\n## Foo\n## Foo-1\n"),
        vec!["foo", "foo-1", "foo-1-1"]
    );
}

#[test]
fn html_ids_and_names_are_anchors() {
    let found = anchors("<a name=\"Top-Of-Page\"></a>\n<div id='box'>x</div>\n<p ID=plain>x</p>\n");
    for want in ["top-of-page", "box", "plain"] {
        assert!(found.iter().any(|s| s == want), "{want} not in {found:?}");
    }
    assert!(anchors("a paragraph that mentions id= and name= in prose\n").is_empty());
}

#[test]
fn a_long_heading_line_marks_the_anchor_set_incomplete() {
    let long = format!("# {}\n", "x".repeat(MAX_LINE_BYTES + 10));
    assert!(heading_anchors(&long, None).unwrap().incomplete);
    // A long table row cannot be a heading, so it does not cast doubt.
    let table = format!("| {} |\n# Real\n", "x".repeat(MAX_LINE_BYTES + 10));
    let found = heading_anchors(&table, None).unwrap();
    assert!(!found.incomplete);
    assert!(found.slugs.contains("real"));
    // Unless it is the text of a setext heading.
    let setext = format!("{}\n====\n", "x".repeat(MAX_LINE_BYTES + 10));
    assert!(heading_anchors(&setext, None).unwrap().incomplete);
}

#[test]
fn ignore_comments_drop_references_and_are_counted() {
    let text = "[a](a.md)\n<!-- devmap:doc-drift ignore-next-line -->\n[b](b.md) `VerifyTask`\n[c](c.md)\n";
    let extraction = extract_references(text);
    let targets: Vec<&RefSpec> = extraction.references.iter().map(|r| &r.spec).collect();
    assert_eq!(targets, vec![&link("a.md"), &link("c.md")]);
    assert_eq!(extraction.skipped.get("ignored_by_comment"), Some(&2));

    let file = "[a](a.md)\n<!-- devmap:doc-drift ignore-file -->\n[b](b.md)\n";
    let extraction = extract_references(file);
    assert!(extraction.references.is_empty(), "{extraction:?}");
    assert_eq!(extraction.skipped.get("ignored_by_comment"), Some(&2));

    // Only a whole-line comment outside a fence is a directive.
    for not_a_directive in [
        "prose <!-- devmap:doc-drift ignore-file --> [a](a.md)\n",
        "```\n<!-- devmap:doc-drift ignore-file -->\n```\n[a](a.md)\n",
        "`<!-- devmap:doc-drift ignore-file -->` [a](a.md)\n",
    ] {
        let extraction = extract_references(not_a_directive);
        assert_eq!(extraction.references.len(), 1, "{not_a_directive:?}");
        assert!(!extraction.skipped.contains_key("ignored_by_comment"));
    }
}

#[test]
fn a_directive_does_not_reach_past_the_next_line() {
    let text = "<!-- devmap:doc-drift ignore-next-line -->\n\n[a](a.md)\n";
    assert_eq!(extract_references(text).references.len(), 1);
}

#[test]
fn request_errors_name_the_actual_problem() {
    let owned = |s: &str| vec![s.to_string()];
    let message = |s: &str| clean_prefixes(&owned(s), "path").unwrap_err().to_string();
    assert!(message("/etc").contains("absolute"), "{}", message("/etc"));
    assert!(
        message("docs\\x").contains("backslash"),
        "{}",
        message("docs\\x")
    );
    assert!(message("../x").contains(".."), "{}", message("../x"));
    assert!(!message("/etc").contains("without `..`"));
    assert!(!message("docs\\x").contains("without `..`"));
}

// Resolver tests over a real checkout -----------------------------------------

#[cfg(feature = "parse")]
mod checkout {
    use super::*;
    use std::path::PathBuf;

    pub struct Fixture {
        pub root: PathBuf,
        pub store: Store,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Writes and indexes `files`; `unindexed` are written but not indexed
    /// (symlinks are made by the caller afterwards).
    pub fn fixture(label: &str, files: &[(&str, &str)]) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "devmap-doc-drift-unit-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        let extractions: Vec<_> = files
            .iter()
            .map(|(path, text)| devmap_extract::extract_file(path, text))
            .collect();
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions).unwrap();
        let analysis = devmap_analyze::analyze(&extractions, &resolution);
        let store = Store::open_in_memory().unwrap();
        store
            .save_generation_with_opts(
                &extractions,
                &resolution,
                &analysis,
                devmap_store::GenerationWriteOpts {
                    repo_root: Some(root.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
        Fixture { root, store }
    }

    pub fn run(fixture: &Fixture) -> DocDriftReport {
        run_with(fixture, &[], &[], true, Limits::default())
    }

    pub fn run_with(
        fixture: &Fixture,
        paths: &[&str],
        exclude: &[&str],
        default_excludes: bool,
        limits: Limits,
    ) -> DocDriftReport {
        let owned = |list: &[&str]| list.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        doc_drift_report_with(
            &fixture.store,
            &Cancel::new(),
            &DocDriftOptions {
                paths: &owned(paths),
                exclude: &owned(exclude),
                token_budget: 100_000,
                default_excludes,
                limits,
            },
        )
        .unwrap()
    }

    pub fn found(report: &DocDriftReport) -> Vec<(String, u32, &'static str, String)> {
        report
            .findings
            .iter()
            .map(|f| (f.doc.clone(), f.line, f.kind.as_str(), f.reference.clone()))
            .collect()
    }

    pub fn skipped(report: &DocDriftReport, reason: &str) -> u32 {
        report.skipped.get(reason).copied().unwrap_or(0)
    }
}

#[cfg(feature = "parse")]
use checkout::{fixture, found, run, run_with, skipped};

#[cfg(all(feature = "parse", unix))]
#[test]
fn a_link_out_of_the_repository_is_neither_followed_nor_judged() {
    let outside =
        std::env::temp_dir().join(format!("devmap-doc-drift-outside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("hosts"), "127.0.0.1 localhost\n").unwrap();

    let f = fixture(
        "symlink-out",
        &[(
            "docs/guide.md",
            "Spans `docs/out/hosts` and `docs/out/nope.rs`.\nLinks [a](out/hosts) and [b](out/nope.md).\n",
        )],
    );
    std::os::unix::fs::symlink(&outside, f.root.join("docs/out")).unwrap();

    let report = run(&f);
    let _ = std::fs::remove_dir_all(&outside);
    // Both an existing and a missing path behind the link say the same thing:
    // nothing. A report that said "missing" for one and nothing for the other
    // would be an existence oracle for the host's file system.
    assert_eq!(found(&report), Vec::new(), "{report:?}");
    assert_eq!(skipped(&report, "outside_repo"), 4, "{report:?}");
    assert_eq!(report.checked["path"], 0, "{report:?}");
    assert_eq!(report.checked["link"], 0, "{report:?}");
}

#[cfg(all(feature = "parse", unix))]
#[test]
fn a_link_into_the_repository_is_followed() {
    let f = fixture(
        "symlink-in",
        &[
            (
                "docs/guide.md",
                "[a](alias.md) [b](alias.md#nope) `docs/alias.md:99`\n",
            ),
            ("src/real.md", "# Real\n"),
        ],
    );
    std::os::unix::fs::symlink(f.root.join("src/real.md"), f.root.join("docs/alias.md")).unwrap();
    let report = run(&f);
    assert_eq!(
        found(&report),
        vec![
            (
                "docs/guide.md".into(),
                1,
                "missing_anchor",
                "alias.md#nope".into()
            ),
            (
                "docs/guide.md".into(),
                1,
                "line_out_of_range",
                "docs/alias.md:99".into()
            ),
        ],
        "{report:?}"
    );
}

#[cfg(all(feature = "parse", unix))]
#[test]
fn a_link_to_a_secret_path_is_judged_by_where_it_lands() {
    // `docs/alias.txt` is not a secret name; `secrets/key.txt` is. Its length
    // is a fact about its contents and must not be read to answer an anchor.
    let f = fixture(
        "symlink-secret",
        &[
            (
                "README.md",
                "[k](docs/alias.txt#L99) `docs/alias.txt:99` [ok](docs/guide.md)\n",
            ),
            ("docs/guide.md", "# Guide\n"),
            ("docs/alias.md", "[gone](gone.md)\n"),
            ("secrets/key.txt", "one line\n"),
            ("secrets/notes.md", "[gone](gone.md)\n"),
        ],
    );
    std::os::unix::fs::symlink(
        f.root.join("secrets/key.txt"),
        f.root.join("docs/alias.txt"),
    )
    .unwrap();
    // The document the index knows as `docs/alias.md` is a link to a secret
    // document: its content is never scanned.
    std::fs::remove_file(f.root.join("docs/alias.md")).unwrap();
    std::os::unix::fs::symlink(
        f.root.join("secrets/notes.md"),
        f.root.join("docs/alias.md"),
    )
    .unwrap();

    let report = run(&f);
    assert_eq!(found(&report), Vec::new(), "{report:?}");
    assert_eq!(skipped(&report, "secret_path"), 2, "{report:?}");
    assert!(
        report
            .walk_incomplete
            .as_deref()
            .is_some_and(|r| r.contains("secret")),
        "{report:?}"
    );
}

#[cfg(feature = "parse")]
#[test]
fn anchors_follow_github_and_an_unscanned_target_is_not_judged() {
    let long_heading = format!("# {}\n", "x".repeat(MAX_LINE_BYTES + 10));
    let f = fixture(
        "anchors",
        &[
            (
                "docs/guide.md",
                "[a](t.md#) [b](t.md#top) [c](t.md#foo-3) [d](t.md#custom-id) [e](t.md#emphasis-word) [f](t.md#nope) [g](long.md#nope)\n",
            ),
            (
                "docs/t.md",
                "## Foo\n## Foo-1\n## Foo\n## Foo\n<a name=\"Custom-ID\"></a>\n> ## _Emphasis_ word\n",
            ),
            ("docs/long.md", &long_heading),
        ],
    );
    let report = run(&f);
    assert_eq!(
        found(&report),
        vec![(
            "docs/guide.md".into(),
            1,
            "missing_anchor",
            "t.md#nope".into()
        )],
        "{report:?}"
    );
    assert_eq!(skipped(&report, "anchor_target_unscanned"), 1, "{report:?}");
    assert!(
        report
            .walk_incomplete
            .as_deref()
            .is_some_and(|r| r.contains("may hold the heading")),
        "{report:?}"
    );
}

#[cfg(feature = "parse")]
#[test]
fn a_symbol_is_moved_only_when_it_is_confidently_gone_from_the_file() {
    let f = fixture(
        "moved",
        &[
            ("src/lib.rs", "pub enum Kind {\n    Widget,\n}\n\npub fn here() {}\n"),
            ("src/other.rs", "pub struct Widget;\npub struct GoneThing;\n"),
            (
                "docs/guide.md",
                "[`Kind::Widget`](../src/lib.rs) [`GoneThing`](../src/lib.rs) [`Other::here`](../src/lib.rs)\n",
            ),
        ],
    );
    let report = run(&f);
    // `Widget` is a variant of `Kind` in lib.rs: the index does not model
    // variants, but the file says it. `GoneThing` is nowhere in lib.rs and
    // defined in other.rs: that is a move.
    assert_eq!(
        found(&report),
        vec![(
            "docs/guide.md".into(),
            1,
            "symbol_moved",
            "GoneThing".into()
        )],
        "{report:?}"
    );
    assert_eq!(report.findings[0].current_locations, vec!["src/other.rs"]);
    assert_eq!(
        skipped(&report, "defined_outside_symbol_index"),
        1,
        "{report:?}"
    );
    // `here` is defined in lib.rs, but not under `Other`: the qualifier is
    // honoured, and an unknowable renamed owner is a skip, never a finding.
    assert_eq!(skipped(&report, "qualifier_unmatched"), 1, "{report:?}");
}

#[cfg(feature = "parse")]
#[test]
fn a_link_reports_a_missing_directory_and_a_span_does_not() {
    // Links resolve against the document, so a missing target is certainly
    // broken. A span's root is ambiguous, so a span whose first directory is
    // nowhere in the tree may belong to another repository: counted, not
    // reported. The asymmetry is deliberate and both halves are visible.
    let f = fixture(
        "unrooted",
        &[(
            "docs/guide.md",
            "[main](oldpkg/main.go) `oldpkg/main.go` `docs/gone.md`\n",
        )],
    );
    let report = run(&f);
    assert_eq!(
        found(&report),
        vec![
            (
                "docs/guide.md".into(),
                1,
                "missing_file",
                "oldpkg/main.go".into()
            ),
            (
                "docs/guide.md".into(),
                1,
                "missing_file",
                "docs/gone.md".into()
            ),
        ],
        "{report:?}"
    );
    assert_eq!(skipped(&report, "unrooted_path"), 1, "{report:?}");
}

#[cfg(feature = "parse")]
#[test]
fn archived_documents_are_left_out_unless_asked_for() {
    let f = fixture(
        "archive",
        &[
            ("docs/new.md", "[gone](gone.md)\n"),
            ("docs/archive/old.md", "[gone](gone.md)\n"),
        ],
    );
    let default = run(&f);
    assert_eq!(default.docs_scanned, 1, "{default:?}");
    assert_eq!(default.docs_total, 1);
    assert_eq!(default.default_excludes, vec!["docs/archive"]);
    assert_eq!(default.docs_excluded_by_default, 1);
    assert_eq!(found(&default).len(), 1);

    let everything = run_with(&f, &[], &[], false, Limits::default());
    assert_eq!(everything.docs_scanned, 2);
    assert!(everything.default_excludes.is_empty());
    assert_eq!(found(&everything).len(), 2);

    // Naming a path inside it lifts the default; naming `docs` does not.
    let named = run_with(&f, &["docs/archive"], &[], true, Limits::default());
    assert_eq!(named.docs_scanned, 1);
    assert!(named.default_excludes.is_empty());
    assert_eq!(found(&named)[0].0, "docs/archive/old.md");
    let broad = run_with(&f, &["docs"], &[], true, Limits::default());
    assert_eq!(broad.docs_scanned, 1);
    assert_eq!(broad.docs_excluded_by_default, 1);
}

#[cfg(feature = "parse")]
#[test]
fn an_ignore_comment_in_a_document_is_counted_in_the_report() {
    let f = fixture(
        "ignore",
        &[
            (
                "docs/a.md",
                "<!-- devmap:doc-drift ignore-next-line -->\n[gone](gone.md)\n[also gone](gone2.md)\n",
            ),
            ("docs/b.md", "<!-- devmap:doc-drift ignore-file -->\n[gone](gone.md) `Vanished()`\n"),
        ],
    );
    let report = run(&f);
    assert_eq!(
        found(&report),
        vec![("docs/a.md".into(), 3, "missing_file", "gone2.md".into())],
        "{report:?}"
    );
    assert_eq!(skipped(&report, "ignored_by_comment"), 3, "{report:?}");
}

#[cfg(feature = "parse")]
#[test]
fn the_finding_cap_keeps_the_first_by_document_and_line() {
    // The missing symbol is found last (it waits for the word search) and
    // sorts first: it must displace later findings, not be displaced by them.
    let f = fixture(
        "cap",
        &[
            ("docs/a.md", "`Vanished()`\n"),
            ("docs/b.md", "[1](m1.md)\n[2](m2.md)\n[3](m3.md)\n"),
        ],
    );
    let limits = Limits {
        max_findings: 2,
        ..Limits::default()
    };
    let report = run_with(&f, &[], &[], true, limits);
    assert_eq!(
        found(&report),
        vec![
            ("docs/a.md".into(), 1, "missing_symbol", "Vanished()".into()),
            ("docs/b.md".into(), 1, "missing_file", "m1.md".into()),
        ],
        "{report:?}"
    );
    assert_eq!(report.total, 4, "{report:?}");
    assert_eq!(report.hidden, 2);
    assert!(report.truncated);
    let reason = report.walk_incomplete.as_deref().unwrap_or("");
    assert!(
        reason.contains("cap of 2") && reason.contains("4 references"),
        "{reason}"
    );
}

#[cfg(feature = "parse")]
#[test]
fn a_cut_word_search_says_so_and_reports_nothing_it_did_not_look_for() {
    let f = fixture(
        "wordcap",
        &[
            ("src/a.rs", "pub fn first() {}\n"),
            (
                "src/b.rs",
                "// LaterOnly lives in the second file\npub fn second() {}\n",
            ),
            ("docs/guide.md", "`LaterOnly()` `NowhereAtAll()`\n"),
        ],
    );
    let limits = Limits {
        max_word_search_bytes: 1,
        ..Limits::default()
    };
    let report = run_with(&f, &[], &[], true, limits);
    // The bound stopped the search after the first file: neither name was
    // found, and neither is reported as missing.
    assert_eq!(found(&report), Vec::new(), "{report:?}");
    assert_eq!(skipped(&report, "word_search_incomplete"), 2, "{report:?}");
    let reason = report.walk_incomplete.as_deref().unwrap_or("");
    assert!(reason.contains("its bound is 1"), "{reason}");
    // The default bound is stated and well under the old half gigabyte.
    assert_eq!(Limits::default().max_word_search_bytes, 64 * 1024 * 1024);
    // Uncut, the same call finds one and reports the other.
    let full = run(&f);
    assert_eq!(
        found(&full),
        vec![(
            "docs/guide.md".into(),
            1,
            "missing_symbol",
            "NowhereAtAll()".into()
        )],
        "{full:?}"
    );
}

#[cfg(feature = "parse")]
#[test]
fn a_cancelled_report_returns_the_cancellation() {
    let f = fixture("cancel", &[("docs/a.md", "[x](x.md)\n")]);
    let cancel = Cancel::new();
    cancel.cancel();
    let error = doc_drift_report_with(
        &f.store,
        &cancel,
        &DocDriftOptions {
            paths: &[],
            exclude: &[],
            token_budget: 1000,
            default_excludes: true,
            limits: Limits::default(),
        },
    )
    .unwrap_err();
    assert!(error.downcast_ref::<QueryCancelled>().is_some(), "{error}");
}

// Round two ---------------------------------------------------------------------

#[test]
fn a_directive_counts_only_on_a_prose_line() {
    let a_link = "[a](a.md)\n";
    for (label, text) in [
        (
            "indented code",
            format!("text\n\n    <!-- devmap:doc-drift ignore-file -->\n\n{a_link}"),
        ),
        (
            "indented with a tab",
            format!("text\n\n\t<!-- devmap:doc-drift ignore-file -->\n\n{a_link}"),
        ),
        (
            "inside an open comment",
            format!("<!--\n<!-- devmap:doc-drift ignore-file -->\n-->\n{a_link}"),
        ),
        (
            "a next-line directive in code",
            "text\n\n    <!-- devmap:doc-drift ignore-next-line -->\n[a](a.md)\n".to_string(),
        ),
    ] {
        let extraction = extract_references(&text);
        assert_eq!(extraction.references.len(), 1, "{label}: {extraction:?}");
        assert!(
            !extraction.skipped.contains_key("ignored_by_comment"),
            "{label}"
        );
    }
    // Three columns is still prose.
    let prose = format!("   <!-- devmap:doc-drift ignore-file -->\n{a_link}");
    assert!(extract_references(&prose).references.is_empty());
}

#[test]
fn manifests_name_their_dependencies() {
    let cargo = "[package]\nname = \"x\"\n\n[workspace.dependencies]\nserde_json = \"1\"\n\n[dependencies]\ntokio = { version = \"1\" }\nmy-lib = { package = \"real_name\", version = \"1\" }\n# commented = \"1\"\n\n[dev-dependencies.anyhow]\nversion = \"1\"\n\n[dependencies.renamed]\npackage = \"origin\"\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n\n[features]\nnot_a_dependency = []\n";
    let mut found = cargo_dependencies(cargo);
    found.sort();
    assert_eq!(
        found,
        vec![
            "anyhow",
            "libc",
            "my-lib",
            "origin",
            "real_name",
            "renamed",
            "serde_json",
            "tokio"
        ]
    );

    let go = "module x\n\ngo 1.22\n\nrequire (\n\tgolang.org/x/sync v0.1.0 // indirect\n\tgithub.com/stretchr/testify/v2 v2.0.0\n\tgopkg.in/yaml.v3 v3.0.1\n)\n\nrequire github.com/google/go-cmp v0.6.0\n";
    let mut found = go_module_names(go);
    found.sort();
    assert_eq!(found, vec!["go-cmp", "sync", "testify", "yaml", "yaml.v3"]);

    let npm = r#"{"name":"x","dependencies":{"react":"18"},"devDependencies":{"@scope/widgets":"1"},"peerDependencies":{"left-pad":"1"},"scripts":{"build":"x"}}"#;
    let mut found = package_json_dependencies(npm);
    found.sort();
    assert_eq!(
        found,
        vec!["@scope/widgets", "left-pad", "react", "widgets"]
    );
    assert!(package_json_dependencies("not json").is_empty());
}

#[cfg(feature = "parse")]
#[test]
fn external_means_declared_and_a_vanished_module_is_reported() {
    let f = fixture(
        "declared",
        &[
            (
                "Cargo.toml",
                "[workspace.dependencies]\nserde_json = \"1\"\n[dependencies]\ntokio = \"1\"\nmy-lib = { package = \"real_name\" }\n[dev-dependencies.anyhow]\nversion = \"1\"\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n",
            ),
            (
                "web/package.json",
                "{\"dependencies\":{\"react\":\"18\"},\"devDependencies\":{\"@scope/widgets\":\"1\"}}",
            ),
            (
                "go.mod",
                "module x\nrequire (\n\tgolang.org/x/sync v0.1.0\n)\n",
            ),
            ("src/store.rs", "pub struct Store;\nimpl Store {\n    pub fn build() {}\n}\n"),
            (
                "docs/guide.md",
                "`serde_json::Value` `tokio::spawn()` `anyhow::Result` `my_lib::Thing()` `real_name::Other()` `libc::getpid()` `React.Component` `Widgets.Button` `Sync.Mutex` `std::fmt::Debug`\n\
                 `oldmod::run_task()` `oldmod::RunTask` `undeclared_crate::Thing` `Store::missingMethod()` `Store::build`\n",
            ),
        ],
    );
    let report = run(&f);
    // Ten qualifiers are external by a positive fact: nine a manifest declares
    // (a renamed dependency by either name) and `std`.
    assert_eq!(skipped(&report, "external_qualifier"), 10, "{report:?}");
    // A module nothing declares and nothing defines is a vanished module, not
    // somebody else's code.
    assert_eq!(
        found(&report),
        vec![
            (
                "docs/guide.md".into(),
                2,
                "missing_symbol",
                "oldmod::run_task()".into()
            ),
            (
                "docs/guide.md".into(),
                2,
                "missing_symbol",
                "oldmod::RunTask".into()
            ),
            (
                "docs/guide.md".into(),
                2,
                "missing_symbol",
                "undeclared_crate::Thing".into()
            ),
            (
                "docs/guide.md".into(),
                2,
                "missing_symbol",
                "Store::missingMethod()".into()
            ),
        ],
        "{report:?}"
    );
}

#[cfg(all(feature = "parse", unix))]
#[test]
fn a_dangling_link_is_judged_by_where_it_points() {
    let outside =
        std::env::temp_dir().join(format!("devmap-doc-drift-dangle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let f = fixture(
        "dangle",
        &[(
            "docs/guide.md",
            "[a](dangle) [b](dangle-out) [c](loop-a) [d](dangle-deep/x.md)\n",
        )],
    );
    let link = |target: &std::path::Path, name: &str| {
        std::os::unix::fs::symlink(target, f.root.join("docs").join(name)).unwrap();
    };
    link(std::path::Path::new("../nowhere.md"), "dangle");
    link(&outside.join("nope.md"), "dangle-out");
    link(std::path::Path::new("loop-b"), "loop-a");
    link(std::path::Path::new("loop-a"), "loop-b");
    link(std::path::Path::new("../gone-dir/deep"), "dangle-deep");
    let report = run(&f);
    let _ = std::fs::remove_dir_all(&outside);
    // Inside the repository and absent: missing. Outside, or a loop: not
    // judged.
    assert_eq!(
        found(&report),
        vec![
            ("docs/guide.md".into(), 1, "missing_file", "dangle".into()),
            (
                "docs/guide.md".into(),
                1,
                "missing_file",
                "dangle-deep/x.md".into()
            ),
        ],
        "{report:?}"
    );
    assert_eq!(skipped(&report, "outside_repo"), 2, "{report:?}");
}

#[cfg(all(feature = "parse", unix))]
#[test]
fn an_anchor_through_a_symlinked_markdown_file_is_checked() {
    let f = fixture(
        "md-link",
        &[
            ("docs/guide.md", "[x](c1#nope) [y](c1#real) [z](c2#nope)\n"),
            ("GUIDE.md", "# Real\n"),
            ("notes.txt", "plain\n"),
        ],
    );
    std::os::unix::fs::symlink(f.root.join("GUIDE.md"), f.root.join("docs/c1")).unwrap();
    std::os::unix::fs::symlink(f.root.join("notes.txt"), f.root.join("docs/c2")).unwrap();
    let report = run(&f);
    assert_eq!(
        found(&report),
        vec![(
            "docs/guide.md".into(),
            1,
            "missing_anchor",
            "c1#nope".into()
        )],
        "{report:?}"
    );
    assert_eq!(skipped(&report, "non_markdown_anchor"), 1, "{report:?}");
}

#[test]
fn backslash_spans_are_a_named_skip() {
    let extraction = extract_references("`docs\\out\\hosts.md` `docs\\out/x.rs` `\\d+\\.rs`\n");
    assert!(extraction.references.is_empty(), "{extraction:?}");
    // The regex-looking span has no path shape and no extension... it does end
    // in `.rs`, so it is a skipped path-shaped span too; what matters is that
    // none is resolved.
    assert!(
        extraction
            .skipped
            .get("backslash_path")
            .copied()
            .unwrap_or(0)
            >= 2
    );
}

#[cfg(feature = "parse")]
#[test]
fn a_backslash_link_is_never_resolved() {
    let f = fixture(
        "backslash",
        &[(
            "docs/guide.md",
            "[a](docs\\out\\x.md) [b](..%5Cx.md) `docs\\out\\y.md`\n",
        )],
    );
    let report = run(&f);
    assert_eq!(found(&report), Vec::new(), "{report:?}");
    assert_eq!(skipped(&report, "backslash_path"), 3, "{report:?}");
    assert_eq!(report.checked["link"], 0, "{report:?}");
}

#[cfg(feature = "parse")]
#[test]
fn the_report_says_an_empty_list_covers_only_the_scanned_documents() {
    let f = fixture(
        "scope",
        &[
            ("docs/new.md", "fine\n"),
            ("docs/archive/old.md", "[gone](gone.md)\n"),
        ],
    );
    let report = run(&f);
    assert!(report.findings.is_empty());
    assert!(
        report.walk_incomplete.is_none(),
        "an exclusion is not an incomplete walk"
    );
    let note = report.scope_note.as_deref().unwrap_or("");
    assert!(
        note.contains("cover only the 1 scanned") && note.contains("1 under docs/archive"),
        "{note}"
    );
    // Nothing excluded, nothing to say.
    let all = run_with(&f, &[], &[], false, Limits::default());
    assert!(all.scope_note.is_none());
}

fn heading_flood(unit: &str, bytes: usize) -> String {
    let line = format!("# {}\n", unit.repeat((MAX_LINE_BYTES - 8) / unit.len()));
    line.repeat(bytes / line.len() + 1)
}

#[test]
fn a_heading_of_link_openers_is_linear() {
    for unit in ["](", "][", "<a ", "`a``", "_a"] {
        let doc = heading_flood(unit, HOSTILE_BYTES);
        let started = Instant::now();
        let _ = heading_anchors(&doc, None).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{unit:?}: {} bytes took {:?}",
            doc.len(),
            started.elapsed()
        );
    }
}

#[test]
fn the_heading_loop_stops_when_cancelled() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    // 2000 lines: four checks while the lines are collected, and the heading
    // loop's own checks after them. The flag is raised on the fifth, which only
    // the heading loop reaches.
    let doc = "# Heading\n".repeat(2000);
    let cancel = Cancel::new();
    let handle = cancel.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let cancel = cancel.with_check_probe(move || {
        if counter.fetch_add(1, Ordering::SeqCst) + 1 == 5 {
            handle.cancel();
        }
    });
    assert!(heading_anchors(&doc, Some(&cancel)).is_err());
    assert!(
        calls.load(Ordering::SeqCst) <= 6,
        "{}",
        calls.load(Ordering::SeqCst)
    );
}
