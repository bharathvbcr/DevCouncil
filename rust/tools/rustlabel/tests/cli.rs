//! The command line refuses what it cannot bound or does not understand.

use std::time::Duration;

use rustlabel::cli::parse;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_golabel_flags_and_this_tools_bounds_parse() {
    let o = parse(&args(&[
        "-root",
        "/c",
        "-dead",
        "d.json",
        "--o",
        "l.json",
        "-max-sites",
        "50",
        "-deadline-secs",
        "90",
        "-request-secs",
        "5",
        "-no-references",
        "-online",
    ]))
    .unwrap();
    assert_eq!(o.root.to_str(), Some("/c"));
    assert_eq!(o.dead.unwrap().to_str(), Some("d.json"));
    assert_eq!(o.out.unwrap().to_str(), Some("l.json"));
    assert_eq!(o.config.max_sites, 50);
    assert_eq!(o.config.overall_budget, Duration::from_secs(90));
    assert_eq!(o.config.request_budget, Duration::from_secs(5));
    assert!(!o.config.references);
    assert!(!o.config.offline);
    assert!(!o.config.trace, "tracing is off unless -trace names a file");
    let traced = parse(&args(&["-root", "/c", "-trace", "t.txt"])).unwrap();
    assert!(traced.config.trace);
    assert_eq!(traced.trace.unwrap().to_str(), Some("t.txt"));
}

#[test]
fn the_defaults_are_bounded_and_offline() {
    let o = parse(&args(&["-root", "/c"])).unwrap();
    assert!(o.config.offline);
    assert!(o.config.references);
    assert!(o.config.max_sites > 0);
    assert!(o.config.request_budget > Duration::ZERO);
    assert!(o.config.overall_budget > o.config.load_budget);
}

#[test]
fn usage_errors() {
    assert!(parse(&args(&[])).unwrap_err().contains("-root"));
    assert!(parse(&args(&["-root"])).unwrap_err().contains("-root"));
    assert!(parse(&args(&["-root", "/c", "-bogus"]))
        .unwrap_err()
        .contains("unknown"));
    assert!(parse(&args(&["-root", "/c", "stray"]))
        .unwrap_err()
        .contains("unexpected"));
    assert!(parse(&args(&["-root", "/c", "-max-sites", "0"]))
        .unwrap_err()
        .contains("positive"));
    assert!(parse(&args(&["-root", "/c", "-max-sites", "x"]))
        .unwrap_err()
        .contains("positive"));
    assert!(parse(&args(&["-root", "/c", "-max-sites"]))
        .unwrap_err()
        .contains("needs a value"));
}
