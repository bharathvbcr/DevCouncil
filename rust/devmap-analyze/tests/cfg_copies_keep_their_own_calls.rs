//! Each `#[cfg]` copy of a function keeps only the calls inside its own span.
//!
//! The extractor already emits both copies. They used to share one qualified
//! name, so every Calls edge of either body was stored on that one identity
//! and the later span — on this host, the `cfg(not(unix))` stub — listed the
//! other body's calls. `5a4a15e7` was not checked out for this test: current
//! HEAD before the fix has the same shared identity, and that is the failure
//! the audit recorded.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_extract::model::Extraction;
use devmap_resolve::{ResolutionResult, Resolver};

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn callee_names(result: &ResolutionResult, source: &str) -> Vec<String> {
    let mut names: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == source)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn has_callee(names: &[String], bare: &str) -> bool {
    let dotted = format!(".{bare}");
    let pathed = format!("::{bare}");
    names
        .iter()
        .any(|target| target == bare || target.ends_with(&dotted) || target.ends_with(&pathed))
}

fn copies<'a>(
    extraction: &'a Extraction,
    name: &str,
) -> Vec<&'a devmap_extract::model::ExtractedSymbol> {
    extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.name == name)
        .collect()
}

fn span_text<'a>(source: &'a str, symbol: &devmap_extract::model::ExtractedSymbol) -> &'a str {
    source
        .get(symbol.span.start_byte..symbol.span.end_byte)
        .unwrap_or("")
}

/// The copy whose own span does not contain `marker` must not carry `forbidden`
/// calls. The copy whose span does contain it must carry every `required` call.
fn assert_split(
    source: &str,
    extraction: &Extraction,
    resolution: &ResolutionResult,
    name: &str,
    marker: &str,
    required: &[&str],
    forbidden: &[&str],
) {
    let found = copies(extraction, name);
    assert_eq!(found.len(), 2, "two bodies of {name}: {found:?}");
    assert_ne!(
        (found[0].span.start_byte, found[0].span.end_byte),
        (found[1].span.start_byte, found[1].span.end_byte),
        "the two bodies are different spans"
    );
    assert_ne!(
        found[0].qualified_name, found[1].qualified_name,
        "shared qualified name attaches both bodies' calls to one identity: {found:?}"
    );
    let live = found
        .iter()
        .copied()
        .find(|symbol| span_text(source, symbol).contains(marker))
        .unwrap_or_else(|| panic!("{name} has no body containing {marker}"));
    let stub = found
        .iter()
        .copied()
        .find(|symbol| !span_text(source, symbol).contains(marker))
        .unwrap_or_else(|| panic!("{name} has no body without {marker}"));
    let live_calls = callee_names(resolution, &live.qualified_name);
    let stub_calls = callee_names(resolution, &stub.qualified_name);
    for bare in required {
        assert!(
            has_callee(&live_calls, bare),
            "{name} live body {} missing {bare}: {live_calls:?}",
            live.qualified_name
        );
        assert!(
            !has_callee(&stub_calls, bare),
            "{name} stub {} carried {bare}: {stub_calls:?}",
            stub.qualified_name
        );
    }
    for bare in forbidden {
        assert!(
            !has_callee(&live_calls, bare),
            "{name} live body {} carried {bare}: {live_calls:?}",
            live.qualified_name
        );
        assert!(
            has_callee(&stub_calls, bare),
            "{name} stub {} missing {bare}: {stub_calls:?}",
            stub.qualified_name
        );
    }
}

fn one_dead_row(files: &[(&str, &str)], bare: &str) {
    let (extractions, resolution) = resolve(files);
    let found = analyze_liveness(&extractions, &resolution);
    let rows: Vec<_> = found
        .iter()
        .filter(|report| {
            report.symbol_name == bare
                || report.symbol_name.ends_with(&format!(".{bare}"))
                || report.symbol_name.contains(&format!("{bare}@"))
        })
        .map(|report| report.symbol_name.as_str())
        .collect();
    assert_eq!(
        rows,
        vec![bare],
        "one unsuffixed dead row; got {rows:?} from {found:?}"
    );
}

const PULSE: &str = "\
fn peak_rss_mb() -> u64 { 0 }
fn signed_chain() {}
fn wanted() -> bool { true }
#[cfg(unix)]
fn pulse_signed_cases() -> Vec<u8> {
    let _ = peak_rss_mb();
    signed_chain();
    if wanted() { Vec::new() } else { Vec::new() }
}
#[cfg(not(unix))]
fn pulse_signed_cases() -> Vec<u8> { Vec::new() }
";

const PULSE_STUB_FIRST: &str = "\
fn peak_rss_mb() -> u64 { 0 }
fn signed_chain() {}
fn wanted() -> bool { true }
#[cfg(not(unix))]
fn pulse_signed_cases() -> Vec<u8> { Vec::new() }
#[cfg(unix)]
fn pulse_signed_cases() -> Vec<u8> {
    let _ = peak_rss_mb();
    signed_chain();
    if wanted() { Vec::new() } else { Vec::new() }
}
";

const GROUP: &str = "\
fn signal_group(leader: u32) -> Result<(), ()> { let _ = leader; Ok(()) }
#[cfg(unix)]
fn group_has_members(leader: u32) -> bool { signal_group(leader).is_ok() }
#[cfg(not(unix))]
fn group_has_members(_leader: u32) -> bool { false }
";

#[test]
fn the_not_unix_stub_of_pulse_signed_cases_does_not_carry_the_unix_calls() {
    let (extractions, resolution) = resolve(&[("src/command_timings.rs", PULSE)]);
    assert_split(
        PULSE,
        &extractions[0],
        &resolution,
        "pulse_signed_cases",
        "peak_rss_mb",
        &["peak_rss_mb", "signed_chain", "wanted"],
        &[],
    );
    one_dead_row(&[("src/command_timings.rs", PULSE)], "pulse_signed_cases");
}

#[test]
fn the_stub_keeps_no_unix_calls_when_it_is_the_earlier_copy() {
    let (extractions, resolution) = resolve(&[("src/command_timings.rs", PULSE_STUB_FIRST)]);
    assert_split(
        PULSE_STUB_FIRST,
        &extractions[0],
        &resolution,
        "pulse_signed_cases",
        "peak_rss_mb",
        &["peak_rss_mb", "signed_chain", "wanted"],
        &[],
    );
}

#[test]
fn the_not_unix_stub_of_group_has_members_does_not_call_signal_group() {
    let (extractions, resolution) = resolve(&[("src/lib.rs", GROUP)]);
    assert_split(
        GROUP,
        &extractions[0],
        &resolution,
        "group_has_members",
        "signal_group",
        &["signal_group"],
        &[],
    );
    one_dead_row(&[("src/lib.rs", GROUP)], "group_has_members");
}

#[test]
fn a_call_that_exists_in_only_the_stub_stays_off_the_live_body() {
    let source = "\
fn only_stub() {}
#[cfg(unix)]
fn host() {}
#[cfg(not(unix))]
fn host() { only_stub(); }
";
    let (extractions, resolution) = resolve(&[("src/lib.rs", source)]);
    assert_split(
        source,
        &extractions[0],
        &resolution,
        "host",
        "only_stub",
        &["only_stub"],
        &[],
    );
}

#[test]
fn a_call_in_both_bodies_is_on_each_body() {
    let source = "\
fn shared() {}
fn only_live() {}
fn only_stub() {}
#[cfg(unix)]
fn both() { shared(); only_live(); }
#[cfg(not(unix))]
fn both() { shared(); only_stub(); }
";
    let (extractions, resolution) = resolve(&[("src/lib.rs", source)]);
    assert_split(
        source,
        &extractions[0],
        &resolution,
        "both",
        "only_live",
        &["only_live"],
        &["only_stub"],
    );
    for symbol in copies(&extractions[0], "both") {
        let calls = callee_names(&resolution, &symbol.qualified_name);
        assert!(
            has_callee(&calls, "shared"),
            "{} should carry the call both bodies make: {calls:?}",
            symbol.qualified_name
        );
    }
}

#[test]
fn three_copies_keep_three_call_sets() {
    let source = "\
fn a() {}
fn b() {}
#[cfg(unix)]
fn tri() { a(); }
#[cfg(windows)]
fn tri() { b(); }
#[cfg(all())]
fn tri() {}
";
    let (extractions, resolution) = resolve(&[("src/lib.rs", source)]);
    let found = copies(&extractions[0], "tri");
    assert_eq!(found.len(), 3, "{found:?}");
    let names: Vec<_> = found
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(
        names
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "three identities: {names:?}"
    );
    let calls_a = found
        .iter()
        .find(|symbol| span_text(source, symbol).contains("a()"))
        .unwrap();
    let calls_b = found
        .iter()
        .find(|symbol| span_text(source, symbol).contains("b()"))
        .unwrap();
    let calls_neither = found
        .iter()
        .find(|symbol| {
            !span_text(source, symbol).contains("a()") && !span_text(source, symbol).contains("b()")
        })
        .unwrap();
    let a_calls = callee_names(&resolution, &calls_a.qualified_name);
    let b_calls = callee_names(&resolution, &calls_b.qualified_name);
    let empty_calls = callee_names(&resolution, &calls_neither.qualified_name);
    assert!(has_callee(&a_calls, "a"), "{a_calls:?}");
    assert!(!has_callee(&a_calls, "b"), "{a_calls:?}");
    assert!(has_callee(&b_calls, "b"), "{b_calls:?}");
    assert!(!has_callee(&b_calls, "a"), "{b_calls:?}");
    assert!(
        !has_callee(&empty_calls, "a") && !has_callee(&empty_calls, "b"),
        "{empty_calls:?}"
    );
}

#[test]
fn an_empty_pair_is_two_symbols_and_one_dead_row() {
    let source = "\
#[cfg(unix)]
fn empty() {}
#[cfg(not(unix))]
fn empty() {}
";
    let (extractions, resolution) = resolve(&[("src/lib.rs", source)]);
    let found = copies(&extractions[0], "empty");
    assert_eq!(found.len(), 2);
    assert_ne!(found[0].qualified_name, found[1].qualified_name);
    for symbol in &found {
        assert!(
            callee_names(&resolution, &symbol.qualified_name).is_empty(),
            "{symbol:?}"
        );
    }
    one_dead_row(&[("src/lib.rs", source)], "empty");
}

#[test]
fn a_unicode_name_on_an_odd_path_keeps_each_bodys_calls() {
    let source = "\
fn 関数() {}
#[cfg(unix)]
fn 名前() { 関数(); }
#[cfg(not(unix))]
fn 名前() {}
";
    let path = "src/odd 'quote' 名前.rs";
    let (extractions, resolution) = resolve(&[(path, source)]);
    assert_split(
        source,
        &extractions[0],
        &resolution,
        "名前",
        "関数",
        &["関数"],
        &[],
    );
    assert!(
        !extractions[0]
            .symbols
            .iter()
            .any(|symbol| symbol.name == "関数" && symbol.qualified_name.contains('@')),
        "a function with one body is not suffixed: {:?}",
        extractions[0].symbols
    );
}

#[test]
fn a_method_pair_does_not_lend_its_calls_to_the_stub() {
    let source = "\
fn signal_group(leader: u32) -> Result<(), ()> { let _ = leader; Ok(()) }
struct S;
#[cfg(unix)]
impl S {
    fn group_has_members(leader: u32) -> bool { signal_group(leader).is_ok() }
}
#[cfg(not(unix))]
impl S {
    fn group_has_members(_leader: u32) -> bool { false }
}
";
    let (extractions, resolution) = resolve(&[("src/lib.rs", source)]);
    assert_split(
        source,
        &extractions[0],
        &resolution,
        "group_has_members",
        "signal_group",
        &["signal_group"],
        &[],
    );
}

#[test]
fn a_nested_helper_inside_the_later_copy_keeps_its_own_call() {
    let source = "\
fn helper() {}
#[cfg(not(unix))]
fn outer() {}
#[cfg(unix)]
fn outer() {
    fn inner() { helper(); }
}
";
    let (extractions, resolution) = resolve(&[("src/lib.rs", source)]);
    let outers = copies(&extractions[0], "outer");
    assert_eq!(outers.len(), 2);
    let stub = outers
        .iter()
        .find(|symbol| !span_text(source, symbol).contains("helper"))
        .unwrap();
    let stub_calls = callee_names(&resolution, &stub.qualified_name);
    assert!(!has_callee(&stub_calls, "helper"), "{stub_calls:?}");
    let inner = copies(&extractions[0], "inner");
    assert_eq!(inner.len(), 1, "{inner:?}");
    let inner_calls = callee_names(&resolution, &inner[0].qualified_name);
    assert!(
        has_callee(&inner_calls, "helper"),
        "{inner_calls:?} name {}",
        inner[0].qualified_name
    );
}

#[test]
fn a_single_function_is_not_suffixed() {
    let source = "fn only() { only(); }\n";
    let (extractions, _) = resolve(&[("src/lib.rs", source)]);
    let found = copies(&extractions[0], "only");
    assert_eq!(found.len(), 1);
    assert!(
        !found[0].qualified_name.contains('@'),
        "{}",
        found[0].qualified_name
    );
}
