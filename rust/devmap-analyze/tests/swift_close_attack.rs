//! Closing adversarial probes for 30a5eade, liveness side (the corpus-wide
//! supertype join that exempts private stored properties). A failing test is a
//! BREAK.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(String, String)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

fn reported(reports: &[DeadSymbolReport], name_suffix: &str) -> bool {
    reports
        .iter()
        .any(|r| r.symbol_name.ends_with(name_suffix) && !r.is_exempt)
}

fn files(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn control_an_unrelated_plain_type_private_property_is_reported() {
    let r = reports(&files(&[(
        "Sources/A/Cfg.swift",
        "struct Cfg { private var secret = 1 }\n",
    )]));
    assert!(reported(&r, "secret"), "control");
}

#[test]
#[ignore = "needs module identity from the manifest targets: the path-derived module is not a sound join key for an Xcode target spanning folders, and a wrong key un-exempts a conforming type (fail-closed), ft-915b0a03"]
fn wrong_an_unrelated_type_of_the_same_name_in_another_module_exempts_nothing() {
    let r = reports(&files(&[
        (
            "Sources/A/Cfg.swift",
            "struct Cfg { private var secret = 1 }\n",
        ),
        ("Sources/B/Cfg.swift", "struct Cfg: Codable { var x = 1 }\n"),
    ]));
    assert!(
        reported(&r, "secret"),
        "a same-named Codable type in another target exempted this one"
    );
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97): a nested type's members are parented to the bare inner name, so Outer.Cfg.secret and a top-level Cfg.other share one parent identity, ft-915b0a03"]
fn wrong_an_extension_conformance_of_a_nested_type_exempts_only_that_type() {
    let r = reports(&files(&[
        (
            "Sources/A/Cfg.swift",
            "struct Outer { struct Cfg { private var secret = 1 } }\nstruct Cfg { private var other = 1 }\n",
        ),
        ("Sources/A/E.swift", "extension Outer.Cfg: Codable {}\n"),
    ]));
    assert!(
        reported(&r, "other"),
        "top-level Cfg (no supertypes) exempted by Outer.Cfg's conformance"
    );
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97): the extractor itself emits the StructuralExempt wiring for a private stored property of any type that names a supertype (langdecl/swift.rs property_exemption), so the liveness join is never asked, ft-915b0a03"]
fn wrong_a_non_synthesising_supertype_does_not_exempt() {
    // `class Base {}`: `class D: Base` has a supertype but no synthesized
    // conformance reads `secret`.
    let r = reports(&files(&[(
        "Sources/A/D.swift",
        "class Base {}\nclass D: Base { private var secret = 1 }\n",
    )]));
    assert!(
        reported(&r, "secret"),
        "a plain subclass exempted a private property no one reads"
    );
}

#[test]
fn wrong_a_property_of_a_type_declared_inside_a_function_scope_type_is_not_exempt_by_name_only() {
    let r = reports(&files(&[
        (
            "Sources/A/X.swift",
            "struct Cfg { private var secret = 1 }\n",
        ),
        (
            "Sources/A/Y.swift",
            "enum Mode: String { case a }\nextension Cfg {}\nextension Mode: Equatable {}\n",
        ),
    ]));
    assert!(reported(&r, "secret"));
}

#[test]
fn timing_the_supertype_join_over_five_thousand_types() {
    let mut sources = Vec::new();
    let mut body = String::new();
    for i in 0..5000 {
        body.push_str(&format!(
            "struct T{i}: Codable {{ private var p{i} = {i}\n var q{i} = 2 }}\nextension T{i}: Hashable {{}}\n"
        ));
    }
    sources.push(("Sources/A/Big.swift".to_string(), body));
    let started = std::time::Instant::now();
    let r = reports(&sources);
    let elapsed = started.elapsed();
    eprintln!("5k types analyse: {elapsed:?}, reports={}", r.len());
    assert!(!reported(&r, "p42"), "exempt through Codable");

    // The same corpus with no supertype anywhere: the join has nothing to find.
    let mut plain = String::new();
    for i in 0..5000 {
        plain.push_str(&format!(
            "struct T{i} {{ private var p{i} = {i}\n var q{i} = 2 }}\n"
        ));
    }
    let started = std::time::Instant::now();
    let r = reports(&[("Sources/A/Big.swift".to_string(), plain)]);
    eprintln!(
        "5k plain types analyse: {:?}, reports={}",
        started.elapsed(),
        r.len()
    );
}
