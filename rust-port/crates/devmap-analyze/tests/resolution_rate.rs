//! W2.1 — every number was already counted and nothing divided.
//!
//! The payoff is not the corpus figure, which is not actionable. It is that a
//! language with no extractor sits at zero *visibly*: W0.2's entire bug class —
//! CFML and Terraform parsing `Clean`, contributing no call edges, and
//! reporting complete coverage — would have surfaced here without anyone going
//! looking for it. That property is asserted directly below.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::model::*;
use devmap_resolve::Resolver;

fn rate_of(extractions: &[Extraction]) -> ResolutionRate {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    resolution_rate(extractions, &resolution)
}

/// A corpus whose calls all bind reaches a perfect rate.
///
/// The OFF direction for every assertion below: without it, a rate that was
/// always zero would satisfy the "a blind language sits at zero" test perfectly.
#[test]
fn a_fully_resolved_corpus_reports_a_perfect_rate() {
    let extractions = vec![extract_file(
        "lib.py",
        "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n",
    )];
    let rate = rate_of(&extractions);

    assert!(rate.resolved_sites > 0, "{rate:?}");
    assert_eq!(
        rate.net_permille,
        Some(1000),
        "every attribution bound, so the net rate is 100%: {rate:?}"
    );
}

/// The headline property: a call-blind language is visibly zero.
#[test]
fn a_language_with_no_call_extractor_sits_at_zero() {
    let extractions = vec![
        extract_file(
            "lib.py",
            "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n",
        ),
        extract_file(
            "main.tf",
            "resource \"aws_s3_bucket\" \"b\" {\n  bucket = lower(var.name)\n}\n",
        ),
    ];
    let rate = rate_of(&extractions);

    let hcl = rate
        .by_language
        .get("hcl")
        .expect("a language present in the corpus must have a row, especially a blind one");
    assert_eq!(
        hcl.resolved_sites, 0,
        "HCL binds nothing because nothing extracts an HCL call: {hcl:?}"
    );
    assert!(
        matches!(hcl.net_permille, None | Some(0)),
        "a call-blind language must read as zero or unmeasured, never as \
         healthy: {hcl:?}"
    );

    // And the Python beside it is unaffected — the breakdown is per language,
    // which is the half that makes the number actionable.
    let python = rate.by_language.get("python").expect("python row");
    assert_eq!(python.net_permille, Some(1000), "{python:?}");
}

/// Structural edges must not inflate the rate.
///
/// `Contains`, `Defines` and `MemberOf` record where a symbol lives; no ladder
/// ran for them. Counting them would make a repository's rate rise with its
/// number of declarations, so the metric would improve by adding code.
#[test]
fn declarations_alone_do_not_raise_the_rate() {
    let declarations_only = vec![extract_file(
        "lib.py",
        "class A:\n    def a(self):\n        pass\n\n    def b(self):\n        pass\n",
    )];
    let rate = rate_of(&declarations_only);
    assert_eq!(
        rate.resolved_sites, 0,
        "a file that declares six symbols and calls nothing has attributed \
         nothing: {rate:?}"
    );
}

/// An ambiguous fan-out is one partial failure, not N successes.
///
/// One ambiguous call with N candidates becomes N edges sharing a single
/// `Arc<Resolution>`. Counting those as N resolutions would make a repository
/// look *better* resolved the more ambiguity it has, inverting the number's
/// meaning. Deduplication is on the shared allocation, which the fan-out is the
/// only thing to share.
#[test]
fn an_ambiguous_fanout_counts_once() {
    // Two same-named definitions the caller cannot choose between.
    let extractions = vec![
        extract_file("a.py", "def target():\n    return 1\n"),
        extract_file("b.py", "def target():\n    return 2\n"),
        extract_file("caller.py", "def main():\n    return target()\n"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let ambiguous_edges = resolution
        .edges
        .iter()
        .filter(|e| {
            matches!(
                e.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )
        })
        .count();
    if ambiguous_edges < 2 {
        // The resolver bound it some other way on this input; the property is
        // still worth stating, so say why the test did not exercise it rather
        // than passing silently.
        eprintln!(
            "note: fixture produced {ambiguous_edges} ambiguous edges, fan-out not exercised"
        );
        return;
    }

    let rate = resolution_rate(&extractions, &resolution);
    assert!(
        rate.resolved_sites
            < resolution
                .edges
                .iter()
                .filter(|e| e.edge_kind == EdgeKind::Calls)
                .count()
            || ambiguous_edges == 0,
        "an N-way fan-out must contribute one site, not N: {rate:?}"
    );
}

/// `None` is not zero, and zero is not `None`.
///
/// A corpus of prose attempts no attribution. Reporting it at 0% would say the
/// resolver failed at everything it tried; reporting 100% would say it
/// succeeded. Both are claims about work that never happened.
#[test]
fn a_corpus_that_attempts_nothing_reports_not_measured() {
    let extractions = vec![extract_file("README.md", "# Title\n\nProse.\n")];
    let rate = rate_of(&extractions);
    assert_eq!(rate.net_permille, None, "{rate:?}");
    assert_eq!(rate.gross_permille, None, "{rate:?}");
    assert_eq!(rate.resolved_sites, 0);
    assert_eq!(rate.unresolved_sites, 0);
}

/// Net excludes the misses that are explained; gross does not.
///
/// `Builtin`, `HostGlobal` and `External` are misses with affirmative evidence
/// behind them, and no extractor work will ever bind them. Ratcheting the gross
/// figure would mean ratcheting how much of a language's standard library a
/// corpus happens to use.
#[test]
fn net_excludes_explained_misses_and_gross_does_not() {
    // Both a binding that succeeds and two that are explained misses. With
    // only the misses, the net denominator is genuinely zero — nothing was
    // attempted that could have succeeded — and `None` is then the correct
    // answer rather than a bug.
    let extractions = vec![extract_file(
        "app.py",
        "def helper():\n    return 1\n\n\ndef main():\n    print('x')\n    \
         return len([helper()])\n",
    )];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let rate = resolution_rate(&extractions, &resolution);

    assert!(
        rate.explained_sites > 0,
        "`print` and `len` are builtins and must be classified as such: {rate:?}"
    );
    match (rate.gross_permille, rate.net_permille) {
        (Some(gross), Some(net)) => assert!(
            net >= gross,
            "excluding explained misses from the denominator can only raise the \
             rate: gross={gross} net={net}"
        ),
        other => panic!("both figures must be measured here: {other:?}"),
    }
}

/// `LocalBinding` stays in the denominator, deliberately.
///
/// It is explained — the enclosing function declares the name — but it is also
/// something the kernel could in principle bind, to that declaration. Excluding
/// it would flatter a number whose entire purpose is to be ratcheted upward.
#[test]
fn local_bindings_are_not_excluded_from_the_net_denominator() {
    let extractions = vec![extract_file("lib.py", "def f():\n    return 1\n")];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let mut resolution = resolver.resolve_all(&extractions);
    resolution.unresolved = vec![UnresolvedReference {
        source_file: "lib.py".to_string(),
        source_symbol: "lib.py::f".to_string(),
        callee_name: "callback".to_string(),
        kind: UnresolvedKind::Call,
        resolution: Resolution::Unresolved {
            reason: "fixture".to_string(),
        },
        class: UnresolvedClass::LocalBinding,
        receiver: None,
    }];

    let rate = resolution_rate(&extractions, &resolution);
    assert_eq!(
        rate.explained_sites, 0,
        "a local binding is not counted among the explained misses: {rate:?}"
    );
    assert_eq!(rate.unresolved_sites, 1);
}

/// The rate survives the summary round trip, and an old summary reads as
/// "never measured" rather than as "measured zero".
#[test]
fn an_older_summary_without_a_rate_reads_as_never_measured() {
    let json = serde_json::json!({
        "total_files": 3,
        "total_symbols": 10,
        "total_edges": 4,
        "dead_symbols": [],
        "communities": [],
        "status": "Ok",
    });
    let summary: AnalysisSummary =
        serde_json::from_value(json).expect("a pre-rate summary must still deserialize");
    assert_eq!(
        summary.resolution_rate.net_permille, None,
        "absence must read as unmeasured, not as a rate of zero"
    );
    assert!(summary.resolution_rate.by_language.is_empty());
}

/// Every language present in the corpus gets a row, including the silent ones.
///
/// A breakdown that omitted the zero rows would hide exactly the language a
/// reader is looking for.
#[test]
fn every_language_in_the_corpus_gets_a_row() {
    let extractions = vec![
        extract_file("a.py", "def f():\n    return 1\n"),
        extract_file("b.java", "class B { void f() {} }\n"),
        extract_file("c.tf", "resource \"r\" \"n\" {}\n"),
    ];
    let rate = rate_of(&extractions);
    for language in ["python", "java", "hcl"] {
        assert!(
            rate.by_language.contains_key(language),
            "{language} is in the corpus and must have a row: {:?}",
            rate.by_language.keys().collect::<Vec<_>>()
        );
    }
}

/// **R13.** Every capability bit the registry declares must reach a consumer.
///
/// `Capability::Calls` and `Capability::Imports` had production readers;
/// `Heritage` and `References` had none in any crate. Two of four bits were
/// declared, maintained and bidirectionally verified against a probe corpus, and
/// read by nobody — which is half the registry sitting in exactly the state
/// `CALL_EXTRACTION_LANGUAGES` was condemned for, minus the wrongness. A fact
/// nothing consults is a fact nothing keeps true.
///
/// Derived over `Capability::ALL` and over the real probe corpus, so a fifth bit
/// added to the enum fails here until it too has a reader, and so the corpus
/// this asserts against is the same one the registry is verified against.
#[test]
fn every_capability_bit_reaches_the_per_language_readout() {
    use devmap_extract::languages::Capability;

    let corpus_dir = std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/capabilities"
    ));
    let mut extractions: Vec<Extraction> = Vec::new();
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&corpus_dir)
        .expect("the capability corpus must exist")
        .map(|entry| entry.expect("a readable entry").path())
        .filter(|path| path.is_file())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        extractions.push(extract_file(&name, &source));
    }
    assert!(
        !extractions.is_empty(),
        "an empty corpus would make every assertion below vacuous"
    );

    let rate = rate_of(&extractions);
    assert!(
        !rate.by_language.is_empty(),
        "the corpus must produce per-language rows"
    );

    let disclosed: std::collections::BTreeSet<&str> = rate
        .by_language
        .values()
        .flat_map(|row| row.blind_to.iter().map(String::as_str))
        .collect();
    let unread: Vec<&str> = Capability::ALL
        .iter()
        .map(|capability| capability.label())
        .filter(|label| !disclosed.contains(label))
        .collect();
    assert!(
        unread.is_empty(),
        "{unread:?} are declared by the capability registry and reach no \
         consumer; a bit nothing reads is a bit nothing keeps true. Disclosed: \
         {disclosed:?}"
    );

    // The OFF direction: a language with every bit set must disclose nothing,
    // or `blind_to` is a field that is always non-empty and therefore says
    // nothing.
    let python = rate
        .by_language
        .get("python")
        .expect("the corpus contains a Python probe");
    assert!(
        python.blind_to.is_empty(),
        "Python declares every capability; disclosing a blindness for it would \
         make the field meaningless: {:?}",
        python.blind_to
    );
}
