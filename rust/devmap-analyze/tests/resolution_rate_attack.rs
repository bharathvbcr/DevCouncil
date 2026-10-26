//! Adversarial probes for `lane/resolution-rate-honesty`. Only fields that
//! exist at both e93dab27 and d747c124 are read, so these run on the base to
//! prove they fail there and pass at head.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::Resolver;

fn rate_of(extractions: &[Extraction]) -> ResolutionRate {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    resolution_rate(extractions, &resolution)
}

const PY: &str = "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n";
const HTML: &str =
    "<html><body><div id=\"main\" class=\"box\"><span class=\"box\"></span></div></body></html>\n";
const CSS: &str = "#main { color: red; }\n.box { margin: 0; }\n";

fn misses(n: usize) -> String {
    // A bare call whose only namesake is a method: a namesake exists, so the
    // miss is not NoNamesake (explained) and stays in the net denominator.
    let mut s = String::from("class K:\n");
    for i in 0..n {
        s.push_str(&format!("    def m{i}(self):\n        return 1\n"));
    }
    for i in 0..n {
        s.push_str(&format!("def v{i}():\n    return m{i}()\n"));
    }
    s
}

#[test]
fn p1_html_is_not_scored_and_does_not_move_the_headline() {
    let ex = vec![
        extract_file("lib.py", PY),
        extract_file("index.html", HTML),
        extract_file("style.css", CSS),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    let html = rate.by_language.get("html").expect("html row");
    assert_eq!(
        html.net_permille, None,
        "call-blind html must not be scored: {html:?}"
    );
    assert_eq!(html.gross_permille, None, "{html:?}");
    let py = rate.by_language.get("python").unwrap();
    assert_eq!(
        rate.net_permille, py.net_permille,
        "headline covers measured languages only"
    );
}

#[test]
fn p1_vendored_c_leaves_no_row_and_does_not_move_the_headline() {
    let ex = vec![
        extract_file("lib.py", PY),
        extract_file(
            "third_party/zlib/z.c",
            "int f(void){return 1;}\nint g(void){return f()+h();}\n",
        ),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    assert!(
        !rate.by_language.contains_key("c"),
        "vendored C must not get a row: {:?}",
        rate.by_language.keys().collect::<Vec<_>>()
    );
    assert_eq!(rate.net_permille, Some(1000), "{rate:?}");
}

#[test]
fn p1_vendored_misses_do_not_drag_the_headline() {
    let ex = vec![
        extract_file("lib.py", PY),
        extract_file("vendor/dep.py", &misses(20)),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    assert_eq!(
        rate.net_permille,
        Some(1000),
        "vendored misses must not count: {rate:?}"
    );
    assert_eq!(
        rate.by_language["python"].net_permille,
        Some(1000),
        "{rate:?}"
    );
}

#[test]
fn p3_vendored_named_files_and_feature_dirs_are_real_code() {
    let ex = vec![
        extract_file(
            "src/vendored_utils.py",
            "def a():\n    return b()\n\ndef b():\n    return 1\n",
        ),
        extract_file(
            "vendors/feature.py",
            "def c():\n    return d()\n\ndef d():\n    return 1\n",
        ),
        extract_file(
            "src/vendor.py",
            "def e():\n    return f()\n\ndef f():\n    return 1\n",
        ),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    let py = &rate.by_language["python"];
    assert!(py.resolved_sites >= 3, "real code wrongly excluded: {py:?}");
}

#[test]
#[ignore = "follow-up: widen is_vendored_path (.venv, generated) for rate and liveness together; ft-b7362179 log"]
fn p3_dot_venv_misses_do_not_drag_the_headline() {
    // A virtualenv holds third-party code but is not in the predicate.
    let ex = vec![
        extract_file("lib.py", PY),
        extract_file(".venv/lib/site.py", &misses(20)),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    assert_eq!(
        rate.net_permille,
        Some(1000),
        ".venv code drags the repo rate: {rate:?}"
    );
}

#[test]
#[ignore = "follow-up: widen is_vendored_path (.venv, generated) for rate and liveness together; ft-b7362179 log"]
fn p3_generated_code_misses_do_not_drag_the_headline() {
    let ex = vec![
        extract_file("lib.py", PY),
        extract_file("api_pb2.py", &misses(20)),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    assert_eq!(
        rate.net_permille,
        Some(1000),
        "generated code drags the repo rate: {rate:?}"
    );
}

#[test]
fn p3_repo_code_calling_into_vendor_keeps_its_own_site() {
    let ex = vec![
        extract_file("vendor/dep.py", "def dep():\n    return 1\n"),
        extract_file(
            "app.py",
            "from vendor.dep import dep\n\n\ndef main():\n    return dep()\n",
        ),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    let py = &rate.by_language["python"];
    assert!(
        py.resolved_sites >= 1,
        "cross-boundary edge from repo code vanished: {py:?}"
    );
}

#[test]
fn p4_html_embedded_script_calls_are_attributed() {
    let page = "<html><body><script>function f(){return 1}\nfunction g(){return f()}\n</script></body></html>\n";
    let ex = vec![extract_file("page.html", page)];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    let any_calls = ex[0].calls.len();
    eprintln!("calls extracted from html: {any_calls}");
    // Embedded JS calls, if the extractor produces any, must be counted.
    assert!(
        any_calls == 0 || rate.resolved_sites + rate.unresolved_sites > 0,
        "html extracted {any_calls} calls but none were counted: {rate:?}"
    );
}

#[test]
fn p5_zero_resolved_nonzero_unresolved_is_zero_not_none() {
    let ex = vec![extract_file(
        "a.py",
        "def main():\n    return missing_one() + missing_two()\n",
    )];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    assert_eq!(rate.gross_permille, Some(0), "{rate:?}");
}

#[test]
fn p5_everything_excluded_gives_none_headline() {
    let ex = vec![extract_file(
        "vendor/a.py",
        "def main():\n    return missing()\n",
    )];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    assert_eq!(rate.net_permille, None, "{rate:?}");
    assert_eq!(rate.gross_permille, None, "{rate:?}");
}
