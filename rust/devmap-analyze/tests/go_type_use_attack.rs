//! Adversarial probes for the Go same-file type-mention exemption and the
//! capped-corpus sentence. Each test asserts the behaviour the lane intends;
//! a failure is a break. `characterise_*` tests pin current behaviour of
//! questions the brief asks to be reported rather than judged.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;
use std::collections::BTreeMap;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

fn row<'a>(rows: &'a [DeadSymbolReport], symbol: &str) -> Option<&'a DeadSymbolReport> {
    rows.iter().find(|r| r.symbol_name == symbol)
}

fn row_in<'a>(
    rows: &'a [DeadSymbolReport],
    symbol: &str,
    file: &str,
) -> Option<&'a DeadSymbolReport> {
    rows.iter()
        .find(|r| r.symbol_name == symbol && r.file_path == file)
}

fn confident(r: Option<&DeadSymbolReport>) -> bool {
    r.is_some_and(|r| r.exemption_reason.is_none() && r.confidence >= 0.9)
}

fn dump(rows: &[DeadSymbolReport]) -> String {
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{}:{} {} {:?}",
                r.file_path, r.symbol_name, r.confidence, r.exemption_reason
            )
        })
        .collect();
    lines.join("\n")
}

const P: &str = "internal/x/a.go";

#[test]
fn go_name_only_in_comment_string_or_tag_stays_confident() {
    let src = concat!(
        "package x\n",
        "// t is described here, see t\n",
        "type t struct{}\n",
        "type w struct {\n\tF int `json:\"t\"`\n}\n",
        "var s = \"t\"\n",
        "func main() { _ = w{}; _ = s }\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(confident(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn go_name_only_in_own_receivers_and_fields_stays_confident() {
    let src = concat!(
        "package x\n",
        "type t struct {\n\tnext *t\n\tkids []t\n\tm map[string]*t\n}\n",
        "func (x t) a() t { return x }\n",
        "func (x *t) b() *t { return x }\n",
        "func (x *t) c(o t, p *t) {}\n",
        "func main() {}\n",
    );
    let rows = reports(&[(P, src)]);
    // A type with methods is not a dead candidate at base either; the lane's
    // exemption must simply not be what hides it.
    assert!(!exempt_by_type_mention(row(&rows, "t")), "{}", dump(&rows));
}

/// The shape where the exemption can actually fire: a shadowing qualified
/// binding leaves `var params t` unbound.
const SHADOW: &str = concat!(
    "import \"encoding/json\"\n",
    "func budget(params json.RawMessage) int {\n\treturn len(params)\n}\n",
);

#[test]
fn go_self_mention_with_unbound_shadow_stays_confident() {
    let src = format!(
        "package x\n{SHADOW}type t struct {{\n\tnext *t\n\tkids []t\n}}\nfunc main() {{ _ = budget(nil) }}\n"
    );
    let rows = reports(&[(P, src.as_str())]);
    // Base already treats a self-referential field as a use (t is no row at
    // all), so the lane's exemption is the only thing under test.
    assert!(!exempt_by_type_mention(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn go_self_mention_in_method_with_unbound_shadow_is_not_exempt() {
    let src = format!(
        "package x\n{SHADOW}type t struct{{}}\nfunc (x t) m(i any) bool {{\n\tvar params t\n\t_, ok := i.(t)\n\t_ = params\n\treturn ok\n}}\nfunc main() {{ _ = budget(nil) }}\n"
    );
    let rows = reports(&[(P, src.as_str())]);
    assert!(!exempt_by_type_mention(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
#[ignore = "accepted over-exemption while the reference is unbound; resolver half of ft-19604c23 (lane/go-qualifier-leak) removes the shape"]
fn go_unbound_mention_only_in_dead_function_is_hidden_characterise() {
    let src = format!(
        "package x\n{SHADOW}type t struct{{}}\nfunc dead() {{\n\tvar params t\n\t_ = params\n}}\nfunc main() {{ _ = budget(nil) }}\n"
    );
    let rows = reports(&[(P, src.as_str())]);
    eprintln!("CHARACTERISE shadow+dead user:\n{}", dump(&rows));
    assert!(confident(row(&rows, "dead")), "{}", dump(&rows));
    assert!(confident(row(&rows, "t")), "t only reachable via dead(): {}", dump(&rows));
}

#[test]
fn go_unbound_alias_mention_is_hidden_characterise() {
    let src = format!(
        "package x\n{SHADOW}type t struct{{}}\ntype u = t\nfunc main() {{ _ = budget(nil) }}\n"
    );
    let rows = reports(&[(P, src.as_str())]);
    eprintln!("CHARACTERISE shadow+alias:\n{}", dump(&rows));
    assert!(!exempt_by_type_mention(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn go_unbound_dead_cycle_is_hidden_characterise() {
    let src = format!(
        "package x\n{SHADOW}type a struct{{ b *b }}\ntype b struct{{ a *a }}\nfunc fa() {{\n\tvar params a\n\t_ = params\n}}\nfunc main() {{ _ = budget(nil) }}\n"
    );
    let rows = reports(&[(P, src.as_str())]);
    eprintln!("CHARACTERISE shadow+cycle:\n{}", dump(&rows));
    assert!(!exempt_by_type_mention(row(&rows, "a")), "{}", dump(&rows));
    assert!(!exempt_by_type_mention(row(&rows, "b")), "{}", dump(&rows));
}

fn exempt_by_type_mention(r: Option<&DeadSymbolReport>) -> bool {
    r.is_some_and(|r| r.exemption_reason.as_deref() == Some(GO_TYPE_MENTION_REASON))
}

#[test]
fn go_name_only_in_type_assertion_inside_own_methods_stays_confident() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "func (x t) m(i any) bool {\n\t_, ok := i.(t)\n\t_, ok2 := i.(*t)\n\tswitch i.(type) {\n\tcase t:\n\tcase *t:\n\t}\n\treturn ok && ok2\n}\n",
        "func (x t) g() {\n\tvar y []t\n\tvar z = make(map[string]t)\n\t_, _ = y, z\n\tf := func() t { return t{} }\n\t_ = f\n}\n",
        "func main() {}\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(!exempt_by_type_mention(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn go_package_qualified_namesake_does_not_exempt_local_type() {
    let src = concat!(
        "package x\n",
        "import \"other\"\n",
        "type t struct{}\n",
        "func use() {\n\tvar a other.t\n\t_ = a\n}\n",
        "func main() { use() }\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(
        confident(row(&rows, "t")),
        "other.t is a different type: {}",
        dump(&rows)
    );
}

#[test]
fn go_field_or_param_named_like_the_type_does_not_exempt() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "type w struct {\n\tt int\n}\n",
        "func f(t int) int { return t }\n",
        "func main() { _ = w{}; _ = f(1) }\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(confident(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn characterise_go_type_mentioned_only_by_unused_function_is_exempt() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "func dead() {\n\tvar v t\n\t_ = v\n}\n",
        "func main() {}\n",
    );
    let rows = reports(&[(P, src)]);
    eprintln!("CHARACTERISE unused-fn mention:\n{}", dump(&rows));
    assert!(
        confident(row(&rows, "dead")),
        "the user function itself must stay confident-dead: {}",
        dump(&rows)
    );
    // At base the resolved mention already keeps t off the dead list; the
    // lane's exemption is not what hides it in this plain shape.
    assert!(!exempt_by_type_mention(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn go_mutually_mentioning_dead_types_are_not_both_hidden() {
    let src = concat!(
        "package x\n",
        "type a struct{ b *b }\n",
        "type b struct{ a *a }\n",
        "func main() {}\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(
        !exempt_by_type_mention(row(&rows, "a")) && !exempt_by_type_mention(row(&rows, "b")),
        "{}",
        dump(&rows)
    );
}

#[test]
fn go_same_package_other_file_mention_unchanged() {
    let a = "package x\ntype t struct{}\nfunc main() {}\n";
    let b = "package x\nfunc g() {\n\tvar v t\n\t_ = v\n}\n";
    let rows = reports(&[("internal/x/a.go", a), ("internal/x/b.go", b)]);
    eprintln!("CHARACTERISE cross-file:\n{}", dump(&rows));
    if let Some(t) = row(&rows, "t") {
        assert_ne!(
            t.exemption_reason.as_deref(),
            Some(GO_TYPE_MENTION_REASON),
            "the same-file exemption must not fire for another file's mention"
        );
    }
}

#[test]
fn go_same_name_in_two_dirs_mention_in_a_does_not_exempt_b() {
    let a = "package a\ntype t struct{}\nfunc g() {\n\tvar v t\n\t_ = v\n}\nfunc main() {}\n";
    let b = "package b\ntype t struct{}\nfunc main() {}\n";
    let rows = reports(&[("one/a.go", a), ("two/b.go", b)]);
    assert!(confident(row_in(&rows, "t", "two/b.go")), "{}", dump(&rows));
}

#[test]
fn go_same_file_name_in_two_dirs_is_keyed_by_full_path() {
    let a = "package a\ntype t struct{}\nfunc g() {\n\tvar v t\n\t_ = v\n}\nfunc main() {}\n";
    let b = "package b\ntype t struct{}\nfunc main() {}\n";
    let rows = reports(&[("one/x.go", a), ("two/x.go", b)]);
    assert!(confident(row_in(&rows, "t", "two/x.go")), "{}", dump(&rows));
}

#[test]
fn go_embedded_in_another_type_counts_as_use() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "type u struct {\n\tt\n}\n",
        "func main() { _ = u{} }\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(
        !confident(row(&rows, "t")),
        "embedding is a use: {}",
        dump(&rows)
    );
}

#[test]
fn go_embedded_pointer_in_another_type_counts_as_use() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "type u struct {\n\t*t\n}\n",
        "func main() { _ = u{} }\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(
        !confident(row(&rows, "t")),
        "embedding *t is a use: {}",
        dump(&rows)
    );
}

#[test]
fn go_composite_literal_counts_as_use() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "func main() {\n\t_ = t{}\n}\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(!confident(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn characterise_go_alias_mention_exempts_target() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "type u = t\n",
        "func main() {}\n",
    );
    let rows = reports(&[(P, src)]);
    eprintln!("CHARACTERISE alias:\n{}", dump(&rows));
    assert!(!exempt_by_type_mention(row(&rows, "t")), "{}", dump(&rows));
}

#[test]
fn go_unused_type_stays_confident_beside_prefix_and_case_neighbours() {
    let src = concat!(
        "package x\n",
        "type t struct{}\n",
        "type tt struct{}\n",
        "type Tt struct{}\n",
        "func main() {\n\t_ = tt{}\n\t_ = Tt{}\n}\n",
    );
    let rows = reports(&[(P, src)]);
    assert!(
        confident(row(&rows, "t")),
        "prefix/case neighbour exempted t: {}",
        dump(&rows)
    );
}

// ---- sentence ----

fn cov(
    parse_failed: usize,
    blind: &[(&str, usize)],
    covered: &[(&str, usize)],
) -> ExtractionCoverage {
    ExtractionCoverage {
        parse_failed_files: parse_failed,
        files_with_call_extraction: covered.iter().map(|c| c.1).sum(),
        blind_by_language: blind.iter().map(|(l, n)| (l.to_string(), *n)).collect(),
        covered_by_language: covered.iter().map(|(l, n)| (l.to_string(), *n)).collect(),
        ..ExtractionCoverage::default()
    }
}

#[test]
fn sentence_absent_when_complete() {
    let c = cov(0, &[], &[("rust", 3), ("go", 2)]);
    assert!(c.is_complete());
    assert!(c.degraded_reason().is_none());
}

#[test]
fn sentence_capped_list_matches_cap_for_per_language() {
    let c = cov(1, &[("python", 1)], &[("rust", 3), ("python", 2), ("go", 1)]);
    let reason = c.degraded_reason().unwrap();
    for lang in ["rust", "go", "python"] {
        let capped_for = c.cap_for(lang, 0.9) < 0.9;
        let named_capped = reason
            .split("only for ")
            .nth(1)
            .and_then(|s| s.split(';').next())
            .is_some_and(|s| s.split(", ").any(|l| l == lang));
        assert_eq!(capped_for, named_capped, "{lang}: {reason}");
    }
}

#[test]
fn sentence_with_family_blindness_names_every_capped_language() {
    // kotlin is blind; java shares the jvm family so cap_for(java) is capped,
    // yet java is in neither per-language map.
    let c = cov(1, &[("kotlin", 1)], &[("kotlin", 2), ("rust", 3)]);
    assert!(
        c.cap_for("java", 0.9) < 0.9,
        "java is capped by kotlin blindness"
    );
    let reason = c.degraded_reason().unwrap();
    assert!(
        !reason.contains("only for kotlin;") || reason.contains("java"),
        "sentence says the cap is only for kotlin while java is capped too: {reason}"
    );
}

#[test]
fn sentence_with_blind_files_but_no_language_attribution_is_well_formed() {
    let c = cov(1, &[], &[("rust", 3)]);
    // With per-language maps present the aggregate blind count is derived from
    // them, so an unattributed parse failure is not a reachable state: the
    // corpus reads as complete and there is no sentence to malform.
    let Some(reason) = c.degraded_reason() else {
        return;
    };
    assert!(
        !reason.contains("only for ;") && !reason.contains("only for  "),
        "empty capped list: {reason}"
    );
    assert_eq!(
        c.is_complete_for("rust"),
        c.cap_for("rust", 0.9) >= 0.9,
        "{reason}"
    );
}

#[test]
fn sentence_all_incomplete_says_every_language() {
    let c = ExtractionCoverage {
        discovery_refused_files: 2,
        files_with_call_extraction: 4,
        covered_by_language: BTreeMap::from([("rust".into(), 2), ("go".into(), 2)]),
        ..ExtractionCoverage::default()
    };
    let reason = c.degraded_reason().unwrap();
    assert!(
        reason.contains("findings in every language are capped"),
        "{reason}"
    );
}

#[test]
fn sentence_with_no_per_language_maps_is_well_formed() {
    let c = ExtractionCoverage {
        parse_failed_files: 1,
        files_with_call_extraction: 4,
        ..ExtractionCoverage::default()
    };
    let reason = c.degraded_reason().unwrap();
    assert!(reason.contains("every language"), "{reason}");
}

#[test]
fn sentence_length_is_bounded_on_a_forty_language_corpus() {
    let mut blind = BTreeMap::new();
    let mut covered = BTreeMap::new();
    for i in 0..40usize {
        let l = format!("lang{i:02}");
        covered.insert(l.clone(), 2usize);
        if i % 2 == 0 {
            blind.insert(l, 1usize);
        }
    }
    let c = ExtractionCoverage {
        parse_failed_files: 20,
        files_with_call_extraction: 80,
        blind_by_language: blind,
        covered_by_language: covered,
        ..ExtractionCoverage::default()
    };
    let reason = c.degraded_reason().unwrap();
    assert!(
        reason.len() <= 600,
        "unbounded sentence ({} bytes): {reason}",
        reason.len()
    );
}

#[test]
fn sentence_language_with_zero_dead_candidates_is_listed_eligible_characterise() {
    let c = cov(1, &[("python", 1)], &[("go", 3), ("python", 2)]);
    let reason = c.degraded_reason().unwrap();
    eprintln!("CHARACTERISE zero-candidate eligible: {reason}");
    assert!(reason.contains("go remain eligible"), "{reason}");
}

fn mixed_outcome() -> LivenessOutcome {
    let sources: &[(&str, &str)] = &[
        ("g/a.go", "package g\nfunc orphan() {}\nfunc main() {}\n"),
        ("r/a.rs", "fn orphan() {}\nfn main() {}\n"),
        ("p/a.py", "def orphan():\n    pass\n"),
        ("t/a.ts", "function orphan() {}\nexport function main() {}\n"),
        ("j/a.js", "function orphan() {}\n"),
        ("k/a.kt", "fun orphan() {}\nfun main() {}\n"),
        ("s/a.swift", "func orphan() {}\n"),
    ];
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let discovery = DiscoveryCoverage::refused(0).with_over_limit(vec![OverLimitFile {
        path: "k/Big.kt".to_string(),
        bytes: 9_000_000,
        limit: 2_000_000,
    }]);
    analyze_liveness_with_coverage(&extractions, &resolution, discovery)
}

fn language_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "go" => "go",
        "rs" => "rust",
        "py" => "python",
        "ts" => "typescript",
        "js" => "javascript",
        "kt" => "kotlin",
        "swift" => "swift",
        _ => "?",
    }
}

#[test]
fn sentence_never_names_capped_a_language_that_has_a_confident_row_in_a_mixed_corpus() {
    let outcome = mixed_outcome();
    let reason = outcome.coverage.degraded_reason().expect("kotlin is blind");
    eprintln!("MIXED: {reason}");
    let capped: Vec<&str> = reason
        .split("only for ")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .map(|s| s.split(", ").collect())
        .unwrap_or_default();
    for r in &outcome.reports {
        if r.confidence >= 0.9 && r.exemption_reason.is_none() {
            let lang = language_of(&r.file_path);
            assert!(
                !capped.contains(&lang) && !reason.contains("every language"),
                "{lang} has a 0.9 row {r:?} but the sentence caps it: {reason}"
            );
        }
    }
}

#[test]
fn sentence_names_as_eligible_only_languages_that_can_publish_a_confident_row() {
    let outcome = mixed_outcome();
    let reason = outcome.coverage.degraded_reason().expect("kotlin is blind");
    let eligible: Vec<&str> = reason
        .split("; ")
        .find(|s| s.ends_with("remain eligible for it"))
        .map(|s| s.trim_end_matches(" remain eligible for it"))
        .map(|s| s.split(", ").collect())
        .unwrap_or_default();
    assert!(!eligible.is_empty(), "{reason}");
    for lang in eligible {
        assert!(
            confident_tier_eligible(lang),
            "the sentence says {lang} remains eligible for the confident tier, but it is not in \
             CONFIDENT_TIER_ELIGIBLE so every {lang} row is capped at 0.4 anyway: {reason}"
        );
    }
}

// ---- other languages: print tiers for base/lane diff ----

#[test]
fn other_languages_same_file_type_mention_tiers_print() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "rust",
            "src/a.rs",
            "struct T;\nfn user() { let _x: T = T; }\nfn main() {}\n",
        ),
        (
            "typescript",
            "src/a.ts",
            "class T {}\nfunction user() { const x: T = new T(); return x; }\nexport function main() {}\n",
        ),
        (
            "python",
            "pkg/a.py",
            "class T:\n    pass\n\ndef user():\n    x: T = T()\n    return x\n",
        ),
        (
            "kotlin",
            "src/a.kt",
            "class T\nfun user() { val x: T = T() }\nfun main() {}\n",
        ),
        (
            "swift",
            "Sources/a.swift",
            "struct T {}\nfunc user() { let x: T = T(); _ = x }\n",
        ),
        (
            "java",
            "src/A.java",
            "class T {}\nclass U { void user() { T x = new T(); } }\n",
        ),
    ];
    for (lang, path, src) in cases {
        let rows = reports(&[(path, src)]);
        for r in &rows {
            println!(
                "TIER {lang} {} {:.2} {:?}",
                r.symbol_name, r.confidence, r.exemption_reason
            );
        }
    }
}
