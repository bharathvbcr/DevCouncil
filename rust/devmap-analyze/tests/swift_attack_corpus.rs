//! Attack harness for the Swift receiver-typing lane. Both tests are `#[ignore]`d
//! and driven by environment variables; nothing here is part of the gates.
//!
//! * `dump_swift_edges`: `DEVMAP_ATTACK_CORPUS=<checkout> DEVMAP_ATTACK_OUT=<file>`
//!   writes every Swift use edge as `caller<TAB>callee<TAB>millis<TAB>kind`, a
//!   `#sites` section of `file:line<TAB>caller<TAB>callee<TAB>receiver`, and
//!   prints the Failed/Partial counts (the load guard).
//! * `time_swift_extraction`: `DEVMAP_ATTACK_CORPUS=<checkout> DEVMAP_ATTACK_ROUNDS=<n>`
//!   times sequential `extract_file` over every Swift file, minimum of n.

mod common;

use devmap_extract::model::{EdgeKind, ParseOutcome};
use devmap_extract::{extract_file, scan_tree};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

fn corpus() -> PathBuf {
    PathBuf::from(std::env::var_os("DEVMAP_ATTACK_CORPUS").expect("DEVMAP_ATTACK_CORPUS"))
}

fn is_use(kind: EdgeKind) -> bool {
    !matches!(
        kind,
        EdgeKind::Contains | EdgeKind::MemberOf | EdgeKind::Imports | EdgeKind::Defines
    )
}

#[test]
#[ignore = "needs DEVMAP_ATTACK_CORPUS"]
fn dump_swift_edges() {
    let dir = corpus();
    let out = std::env::var_os("DEVMAP_ATTACK_OUT").expect("DEVMAP_ATTACK_OUT");
    let built = common::build(&dir);
    let scanned = scan_tree(&dir).expect("scan");
    let failed = built
        .extractions
        .iter()
        .filter(|e| e.is_parse_failure())
        .count();
    let partial = built
        .extractions
        .iter()
        .filter(|e| matches!(e.parse_outcome, ParseOutcome::Partial { .. }))
        .count();
    eprintln!("LOADGUARD failed={failed} partial={partial}");
    let mut text = String::new();
    let mut edges: Vec<String> = built
        .resolution
        .edges
        .iter()
        .filter(|e| is_use(e.edge_kind) && e.source_file.ends_with(".swift"))
        .map(|e| {
            let kind = e.resolution.as_ref().map_or("none", |r| r.kind().label());
            format!(
                "{}\t{}\t{}\t{}",
                e.source_symbol,
                e.target_symbol,
                e.confidence.to_millis(),
                kind
            )
        })
        .collect();
    edges.sort();
    edges.dedup();
    for e in &edges {
        text.push_str(e);
        text.push('\n');
    }
    text.push_str("#sites\n");
    let sources: std::collections::BTreeMap<&str, &str> = scanned
        .sources
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    for ext in built
        .extractions
        .iter()
        .filter(|e| e.file_path.ends_with(".swift"))
    {
        let Some(src) = sources.get(ext.file_path.as_str()) else {
            continue;
        };
        for call in &ext.calls {
            let line = src[..call.span.start_byte.min(src.len())]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                + 1;
            let _ = writeln!(
                text,
                "{}:{}\t{}\t{}\t{}",
                ext.file_path,
                line,
                call.caller_symbol.as_deref().unwrap_or(""),
                call.callee_name,
                call.receiver_expr.as_deref().unwrap_or("").replace('\n', " ")
            );
        }
    }
    std::fs::write(out, text).expect("write dump");
}

#[test]
#[ignore = "needs DEVMAP_ATTACK_CORPUS"]
fn time_swift_extraction() {
    let dir = corpus();
    let rounds: usize = std::env::var("DEVMAP_ATTACK_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let scanned = scan_tree(&dir).expect("scan");
    let files: Vec<(&String, &String)> = scanned
        .sources
        .iter()
        .filter(|(p, _)| p.ends_with(".swift"))
        .map(|(p, s)| (p, s))
        .collect();
    let mut best = f64::MAX;
    let mut all = Vec::new();
    for _ in 0..rounds {
        let start = Instant::now();
        let mut calls = 0usize;
        for (p, s) in &files {
            calls += extract_file(p, s).calls.len();
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        best = best.min(ms);
        all.push(ms);
        assert!(calls > 0);
    }
    eprintln!(
        "TIMING files={} min_ms={best:.1} all={:?}",
        files.len(),
        all.iter().map(|v| v.round()).collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "synthetic"]
fn time_synthetic_swift_file() {
    let members: usize = std::env::var("DEVMAP_ATTACK_MEMBERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let rounds: usize = std::env::var("DEVMAP_ATTACK_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let mut src = String::from("struct Item { var id: Int; func weight() -> Int { 1 } }\nfinal class Host {\n");
    for i in 0..members {
        let _ = writeln!(src, "    var items{i}: [Item] = []");
    }
    for i in 0..members {
        let _ = writeln!(
            src,
            "    func run{i}() {{ items{i}.forEach {{ $0.weight() }}; _ = items{i}.map {{ $0.id }}; free{i}(); }}"
        );
    }
    src.push_str("}\n");
    for i in 0..members {
        let _ = writeln!(src, "func free{i}() -> [Item] {{ [] }}");
    }
    let mut best = f64::MAX;
    let mut calls = 0;
    for _ in 0..rounds {
        let start = Instant::now();
        let ext = extract_file("big.swift", &src);
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
        assert!(matches!(ext.parse_outcome, ParseOutcome::Clean), "{:?}", ext.parse_outcome);
        calls = ext.calls.len();
    }
    eprintln!("SYNTH members={members} bytes={} calls={calls} min_ms={best:.1}", src.len());
}
