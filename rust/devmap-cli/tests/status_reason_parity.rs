//! Adversarial probes for the shared `degraded_reason` join: the CLI's
//! `devmap status`, the daemon's `status` and the MCP `devmap_status` must say
//! the same thing about the same store.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use devmap_analyze::model::AnalysisStatus;
use devmap_serve::protocol::{daemon_index_is_fresh, daemon_status_reason, UnappliedEdits};
use devmap_store::{GenerationWriteOpts, Store};
use serde_json::{json, Value};

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-reason-parity-{label}-{}-{stamp}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[derive(Clone, Copy, Debug)]
struct Shape {
    stale: bool,
    budget: bool,
}

const CLEAN: Shape = Shape {
    stale: false,
    budget: false,
};

/// A file-backed store rooted at `dir` whose latest analysis carries `status`.
fn build_store(dir: &Path, status: AnalysisStatus, shape: Shape) -> PathBuf {
    let src = "def fine():\n    return 1\n";
    std::fs::write(dir.join("fine.py"), src).unwrap();
    let mut extractions = vec![devmap_extract::extract_file("fine.py", src)];
    if shape.budget {
        std::fs::write(dir.join("slow.py"), src).unwrap();
        let refused = devmap_extract::extract_file_with_budget("slow.py", src, Duration::ZERO);
        assert!(refused.is_budget_refusal());
        extractions.push(refused);
    }
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let mut analysis = devmap_analyze::analyze(&extractions, &resolution);
    analysis.status = status;
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(dir.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    if shape.stale {
        let report = store
            .enqueue_pending_paths_under_root(dir, &["fine.py".to_string()])
            .unwrap();
        assert_eq!(
            report.enqueued.len(),
            1,
            "the stale fixture must be pending"
        );
    }
    db
}

fn cli_status(dir: &Path, db: &Path) -> Value {
    let out = Command::new(devmap())
        .arg("--db")
        .arg(db)
        .args(["--json", "status"])
        .current_dir(dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "status is not JSON ({e}): {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn unapplied(on: bool) -> UnappliedEdits {
    let u = UnappliedEdits::default();
    if on {
        u.record(3, "disk full");
    }
    u
}

fn daemon(db: &Path, u: &UnappliedEdits) -> (Option<String>, bool) {
    let store = Store::open_existing(db).unwrap().unwrap();
    let status = store.status("daemon").unwrap();
    (
        daemon_status_reason(&store, &status, u).unwrap(),
        daemon_index_is_fresh(&status, u),
    )
}

fn statuses() -> Vec<(&'static str, AnalysisStatus)> {
    vec![
        ("complete", AnalysisStatus::Ok),
        (
            "partial",
            AnalysisStatus::Partial {
                reason: "1 file(s) refused by discovery".into(),
            },
        ),
        (
            "timeout",
            AnalysisStatus::Timeout {
                reason: "community detection hit its ceiling".into(),
            },
        ),
    ]
}

/// Probe 1: every combination, CLI reason == daemon reason minus `Also:`.
#[test]
fn p1_cli_and_daemon_agree_on_every_combination() {
    for (label, status) in statuses() {
        for stale in [false, true] {
            for budget in [false, true] {
                let dir = scratch("p1");
                let shape = Shape { stale, budget };
                let db = build_store(&dir, status.clone(), shape);
                let cli = cli_status(&dir, &db);
                for un in [false, true] {
                    let u = unapplied(un);
                    let (reason, _) = daemon(&db, &u);
                    let ctx = format!("{label} {shape:?} unapplied={un}");
                    let stripped = match (&reason, u.describe()) {
                        (Some(r), Some(d)) => {
                            let tail = format!(" Also: {d}");
                            match r.strip_suffix(&tail) {
                                Some(head) => Some(head.to_string()),
                                // unapplied-only: the whole reason is the clause
                                None if *r == d => None,
                                None => panic!("{ctx}: unexpected shape {r:?}"),
                            }
                        }
                        (r, None) => r.clone(),
                        (None, Some(_)) => panic!("{ctx}: unapplied edits vanished"),
                    };
                    let cli_reason = cli["degraded_reason"].as_str().map(str::to_string);
                    assert_eq!(stripped, cli_reason, "{ctx}\ncli={cli}");
                }
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

/// Probe 2: clean is null in both surfaces; nothing is ever the empty string.
#[test]
fn p2_clean_store_is_null_and_no_reason_is_empty() {
    let dir = scratch("p2");
    let db = build_store(&dir, AnalysisStatus::Ok, CLEAN);
    let cli = cli_status(&dir, &db);
    assert!(cli["degraded_reason"].is_null(), "{cli}");
    let (reason, fresh) = daemon(&db, &unapplied(false));
    assert_eq!(reason, None);
    assert!(fresh);
    let _ = std::fs::remove_dir_all(&dir);

    for (label, status) in statuses() {
        for stale in [false, true] {
            for budget in [false, true] {
                for un in [false, true] {
                    let dir = scratch("p2b");
                    let db = build_store(&dir, status.clone(), Shape { stale, budget });
                    let cli = cli_status(&dir, &db);
                    let (reason, _) = daemon(&db, &unapplied(un));
                    assert_ne!(cli["degraded_reason"], json!(""), "{label} {stale} {budget}");
                    assert_ne!(reason.as_deref(), Some(""), "{label} {stale} {budget} {un}");
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
    }
}

/// Probe 2b: an analysis that carries an empty reason must not leave a
/// dangling `partial: ` clause.
#[test]
fn p2b_empty_analysis_reason_leaves_no_dangling_clause() {
    for status in [
        AnalysisStatus::Partial { reason: "".into() },
        AnalysisStatus::Timeout { reason: "".into() },
    ] {
        let dir = scratch("p2c");
        let db = build_store(&dir, status, CLEAN);
        let (reason, _) = daemon(&db, &unapplied(false));
        let reason = reason.expect("a partial/timeout analysis is degraded");
        assert_eq!(reason, reason.trim_end(), "dangling space: {reason:?}");
        assert!(!reason.ends_with(':'), "dangling colon: {reason:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Probe 2c: an unapplied-edits record with an empty cause must not read as a
/// sentence with a bare `: .`.
#[test]
fn p2c_empty_unapplied_reason_leaves_no_dangling_clause() {
    let dir = scratch("p2d");
    let db = build_store(&dir, AnalysisStatus::Ok, CLEAN);
    let u = UnappliedEdits::default();
    u.record(1, "");
    let (reason, _) = daemon(&db, &u);
    let reason = reason.unwrap();
    assert!(!reason.contains(": ."), "dangling clause: {reason:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Probe 3: unapplied edits alone.
#[test]
fn p3_unapplied_only_has_no_leading_also_or_dangling_separator() {
    let dir = scratch("p3");
    let db = build_store(&dir, AnalysisStatus::Ok, CLEAN);
    let sweep = UnappliedEdits::default();
    sweep.begin_initial_sweep();
    for u in [unapplied(true), sweep] {
        let (reason, fresh) = daemon(&db, &u);
        let reason = reason.expect("unapplied edits must surface");
        assert!(!reason.starts_with("Also:"), "{reason}");
        assert!(!reason.starts_with("; "), "{reason}");
        assert!(!reason.contains("Also:"), "{reason}");
        assert!(!reason.trim_end().ends_with(';'), "{reason}");
        assert!(!fresh);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Probe 4: stale + partial ordering and separators.
#[test]
fn p4_stale_then_partial_then_budget_then_also_with_single_separators() {
    let dir = scratch("p4");
    let db = build_store(
        &dir,
        AnalysisStatus::Partial {
            reason: "1 file(s) refused by discovery".into(),
        },
        Shape {
            stale: true,
            budget: true,
        },
    );
    let (reason, _) = daemon(&db, &unapplied(true));
    let r = reason.unwrap();
    let pending = r.find("source change(s) are pending").expect(&r);
    let partial = r.find("partial: ").expect(&r);
    let budget = r.find("budget-degraded: ").expect(&r);
    let also = r.find(" Also: ").expect(&r);
    assert!(pending < partial && partial < budget && budget < also, "{r}");
    for bad in [";;", "; ;", "Also: Also:", "  ", "; Also", ";  "] {
        assert!(!r.contains(bad), "doubled separator {bad:?} in {r}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Probe 5: hostile analysis reasons.
fn hostile(reason: String) {
    let dir = scratch("p5");
    let db = build_store(
        &dir,
        AnalysisStatus::Partial {
            reason: reason.clone(),
        },
        CLEAN,
    );
    let cli = cli_status(&dir, &db);
    let cli_reason = cli["degraded_reason"].as_str().unwrap().to_string();
    let (daemon_reason, _) = daemon(&db, &unapplied(false));
    assert_eq!(Some(cli_reason.clone()), daemon_reason, "surfaces diverge");
    // Escaped in JSON framing: a single serialised line, no raw control bytes.
    let wire = serde_json::to_string(&json!({"degraded_reason": cli_reason})).unwrap();
    assert!(!wire.bytes().any(|b| b < 0x20), "raw control byte on wire");
    // A status reason is a sentence, not a transport for arbitrary bytes:
    // control characters must not survive into it, and it must be bounded.
    assert!(
        !cli_reason.chars().any(|c| c.is_control()),
        "control characters survive into degraded_reason: {:?}",
        &cli_reason[..cli_reason.len().min(80)]
    );
    assert!(
        cli_reason.len() <= 16 * 1024,
        "degraded_reason is unbounded: {} bytes",
        cli_reason.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn p5a_control_characters_in_an_analysis_reason_are_not_forwarded() {
    hostile("line1\nline2\r\u{7}\u{1b}[31mred\u{0}".to_string());
}

#[test]
fn p5b_a_100kb_analysis_reason_is_capped() {
    hostile("x".repeat(100 * 1024));
}

/// Human (non --json) output must not carry raw escape bytes either.
#[test]
fn p5c_human_status_output_carries_no_raw_escape_byte() {
    let dir = scratch("p5c");
    let db = build_store(
        &dir,
        AnalysisStatus::Partial {
            reason: "bad\u{1b}[31mred\nsecond line".into(),
        },
        CLEAN,
    );
    let out = Command::new(devmap())
        .arg("--db")
        .arg(&db)
        .arg("status")
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!out.stdout.contains(&0x1b), "raw ESC in human status output");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Probe 6: is_fresh semantics.
#[test]
fn p6_a_partial_reason_never_flips_is_fresh() {
    for (label, status) in statuses() {
        for budget in [false, true] {
            let dir = scratch("p6");
            let db = build_store(
                &dir,
                status.clone(),
                Shape {
                    stale: false,
                    budget,
                },
            );
            let cli = cli_status(&dir, &db);
            let (reason, fresh) = daemon(&db, &unapplied(false));
            assert_eq!(cli["is_fresh"], json!(true), "{label} budget={budget} {cli}");
            assert!(fresh, "{label} budget={budget}: daemon flipped is_fresh");
            assert_eq!(cli["pending_count"], json!(0));
            if label != "complete" {
                assert!(reason.is_some());
            }
            let _ = std::fs::remove_dir_all(&dir);

            let dir = scratch("p6s");
            let db = build_store(&dir, status.clone(), Shape { stale: true, budget });
            let cli = cli_status(&dir, &db);
            let (_, fresh) = daemon(&db, &unapplied(false));
            assert_eq!(cli["is_fresh"], json!(false), "{label} stale {cli}");
            assert!(!fresh);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Probe 7: the MCP envelope carries the same string over a real pipe.
#[test]
fn p7_mcp_devmap_status_carries_the_cli_degraded_reason() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    for big in [false, true] {
        let dir = scratch("p7");
        let reason = if big {
            "y".repeat(100 * 1024)
        } else {
            "1 file(s) refused by discovery".to_string()
        };
        let db = build_store(
            &dir,
            AnalysisStatus::Partial { reason },
            Shape {
                stale: false,
                budget: true,
            },
        );
        let cli = cli_status(&dir, &db);
        let mut child = Command::new(devmap())
            .arg("--db")
            .arg(&db)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut ask = |id: i64, method: &str, params: Value| -> Value {
            writeln!(
                stdin,
                "{}",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .unwrap();
            stdin.flush().unwrap();
            serde_json::from_str(&lines.next().expect("a reply").unwrap()).unwrap()
        };
        ask(
            1,
            "initialize",
            json!({"protocolVersion":"2025-11-25","capabilities":{},
                   "clientInfo":{"name":"probe","version":"0"}}),
        );
        let reply = ask(
            2,
            "tools/call",
            json!({"name":"devmap_status","arguments":{}}),
        );
        drop(stdin);
        let _ = child.wait();
        let text = reply.to_string();
        assert_eq!(
            reply["result"]["isError"],
            json!(false),
            "{}",
            &text[..text.len().min(300)]
        );
        let mcp = &reply["result"]["structuredContent"]["degraded_reason"];
        assert_eq!(mcp, &cli["degraded_reason"], "MCP and CLI reasons differ");
        assert!(mcp.as_str().is_some_and(|s| s.contains("partial: ")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
