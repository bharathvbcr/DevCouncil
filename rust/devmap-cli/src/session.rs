//! Session-end insights: what DevMap was asked, what it withheld, what to build next.
//!
//! SessionEnd hooks share a short budget, so this command only reads the live
//! query log and a status snapshot. It never builds an index.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use devmap_serve::session_log;

/// Known GitNexus-shaped questions DevMap does not yet answer as first-class tools.
const MISSING_CAPABILITIES: &[(&str, &str)] = &[
    (
        "detect_changes",
        "git-diff → affected flows; workaround: `devmap impact` / `devmap affected` on changed symbols",
    ),
    (
        "rename",
        "graph-backed coordinated rename; workaround: `devmap search` + `devmap preview`",
    ),
    (
        "cypher",
        "ad-hoc graph query language; workaround: compose `explore` / `neighbors` / `trace`",
    ),
    (
        "pdg_query",
        "control/data dependence; Python `dev map --pdg` exists, the kernel MCP does not",
    ),
    (
        "taint_explain",
        "source→sink taint findings; Python `dev map --pdg` exists, the kernel MCP does not",
    ),
    (
        "route_map",
        "HTTP route → handler → consumer map; not in the kernel tool list",
    ),
    (
        "clusters_processes",
        "precomputed execution-flow resources; workaround: subsystems in repo_map.json + `explore`",
    ),
];

pub fn gaps_path(db: &Path) -> PathBuf {
    session_log::sessions_dir(db).join("gaps.jsonl")
}

/// Most bytes one gap entry may occupy.
///
/// A gap's `reason` is prose an agent writes, so it is the field with no
/// natural bound. Capped here rather than at the caller because this is the
/// only writer, and a ledger one entry can fill is a ledger the next agent
/// cannot append to.
const MAX_GAP_BYTES: usize = 8 * 1024;

/// Append one gap to the ledger the agent guide tells every agent to keep.
///
/// **The instruction had no writer behind it.** `CLAUDE.md` has said "record a
/// gap in `.devcouncil/codeintel/sessions/gaps.jsonl`" for as long as it has
/// existed; [`gaps_path`] names the file and `read_gaps` reads it, and nothing
/// in the kernel, the CLI or the MCP surface ever appended to it. An agent
/// following the guide reached for a shell redirect, and the harness refuses
/// `.devcouncil/` as a protected path — correctly, because the store beside it
/// is what every later answer is read from. So the documented process could not
/// be carried out at all: the gaps it asks for went into session scratch files
/// that nothing reads, and `session-report` kept reporting a ledger that only
/// ever grew by hand.
///
/// Written through the owner instead. The kernel already owns this directory,
/// and a tool writing its own state is the arrangement that protection exists
/// to preserve rather than the one it exists to stop.
///
/// Appends rather than rewrites, so two agents recording at once interleave
/// whole lines instead of truncating each other — the same reason the query log
/// is opened [`Access::Append`](devmap_extract::safe_fs::Access::Append).
pub fn record_gap(
    db: &Path,
    tool: &str,
    gap_id: &str,
    reason: &str,
    repo_path: Option<&str>,
    resolved: bool,
) -> anyhow::Result<Value> {
    use devmap_extract::safe_fs::{Access, Creation, SafeFile};

    // Each field is what a reader keys on, so an empty one is a row that names
    // nothing. Refused rather than written, because a ledger of blanks reads
    // exactly like a ledger nobody kept.
    for (label, value) in [("--tool", tool), ("--gap-id", gap_id), ("--reason", reason)] {
        if value.trim().is_empty() {
            anyhow::bail!("{label} must not be empty: a gap that names nothing records nothing");
        }
    }

    let mut entry = Map::new();
    entry.insert(
        "ts_ms".into(),
        json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)),
    );
    entry.insert("tool".into(), json!(tool.trim()));
    entry.insert("gap_id".into(), json!(gap_id.trim()));
    entry.insert("reason".into(), json!(reason.trim()));
    if let Some(repo_path) = repo_path.filter(|path| !path.trim().is_empty()) {
        entry.insert("repo_path".into(), json!(repo_path.trim()));
    }
    if resolved {
        entry.insert("resolved".into(), json!(true));
    }
    let value = Value::Object(entry);

    let line = serde_json::to_string(&value)?;
    if line.len() > MAX_GAP_BYTES {
        anyhow::bail!(
            "gap entry is {} bytes, over the {MAX_GAP_BYTES}-byte limit; \
             shorten --reason or link the detail from it",
            line.len()
        );
    }

    let path = gaps_path(db);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = SafeFile::open(&path, Access::Append, Creation::IfMissing)?;
    if file.metadata()?.len().saturating_add(line.len() as u64 + 1) > session_log::MAX_SESSION_BYTES
    {
        anyhow::bail!(
            "gap ledger is at its {} byte limit; rotate {} before recording more",
            session_log::MAX_SESSION_BYTES,
            path.display()
        );
    }
    writeln!(file, "{line}")?;
    file.flush()?;
    Ok(json!({ "recorded": value, "path": path.display().to_string() }))
}

/// Write a report from the live log (or print the previous one).
pub fn run(
    db: &Path,
    last: bool,
    session_id: Option<&str>,
    json_out: bool,
) -> anyhow::Result<Value> {
    if last {
        return print_last(db, json_out);
    }
    let report = build_report(db, session_id)?;
    let stamp = report
        .get("stamp")
        .and_then(Value::as_str)
        .unwrap_or("session")
        .to_string();
    let dir = session_log::sessions_dir(db);
    let json_path = dir.join(format!("{stamp}.json"));
    let md_path = dir.join(format!("{stamp}.md"));
    let rendered = serde_json::to_vec_pretty(&report)?;
    devmap_extract::safe_fs::preflight_write(&json_path)?;
    devmap_extract::safe_fs::preflight_write(&md_path)?;
    devmap_query::write_atomic(&json_path, &rendered)?;
    devmap_query::write_atomic(&md_path, render_markdown(&report).as_bytes())?;
    session_log::rotate_live(db, &stamp)?;
    if json_out {
        return Ok(report);
    }
    // SessionStart/SessionEnd stdout is the only channel the host keeps.
    // Keep it short: the files hold the rest.
    println!("{}", brief(&report));
    let _ = writeln!(
        std::io::stderr(),
        "wrote {} and {}",
        json_path.display(),
        md_path.display()
    );
    Ok(report)
}

fn print_last(db: &Path, json_out: bool) -> anyhow::Result<Value> {
    let Some(report) = newest_report(db)? else {
        if json_out {
            return Ok(json!({"found": false}));
        }
        return Ok(json!({"found": false}));
    };
    if json_out {
        return Ok(report);
    }
    println!("{}", brief(&report));
    Ok(report)
}

fn newest_report(db: &Path) -> anyhow::Result<Option<Value>> {
    use devmap_extract::safe_fs::{Access, Creation, PinnedDir};
    let dir = session_log::sessions_dir(db);
    let directory = match PinnedDir::open(&dir, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut newest = None;
    for (index, entry) in fs::read_dir(&dir)?.enumerate() {
        anyhow::ensure!(
            index < 4096,
            "session report inventory exceeds 4096 entries"
        );
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json")
            || path.file_name().and_then(|n| n.to_str()) == Some("gaps.json")
        {
            continue;
        }
        let file = directory.open_file(&entry.file_name(), Access::Read, Creation::Never)?;
        let modified = file.metadata()?.modified()?;
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, entry.file_name()));
        }
    }
    let Some((_, name)) = newest else {
        return Ok(None);
    };
    let text = directory
        .open_file(&name, Access::Read, Creation::Never)?
        .read_text(session_log::MAX_SESSION_BYTES)?;
    Ok(Some(serde_json::from_str(&text)?))
}

fn build_report(db: &Path, session_id: Option<&str>) -> anyhow::Result<Value> {
    let queries = session_log::read_live(db)?;
    let gaps = read_gaps(db)?;
    let stamp = stamp_now();
    let mut truncated = 0u64;
    let mut walk_incomplete = 0u64;
    let mut empty = 0u64;
    let mut errors = 0u64;
    let mut tools: Map<String, Value> = Map::new();
    for row in &queries {
        let tool = row
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let entry = tools.entry(tool).or_insert_with(|| {
            json!({
                "count": 0,
                "truncated": 0,
                "walk_incomplete": 0,
                "empty": 0,
                "errors": 0,
            })
        });
        if let Some(obj) = entry.as_object_mut() {
            bump(obj, "count");
            if row.get("truncated") == Some(&Value::Bool(true)) {
                bump(obj, "truncated");
                truncated += 1;
            }
            if row
                .get("walk_incomplete")
                .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
            {
                bump(obj, "walk_incomplete");
                walk_incomplete += 1;
            }
            if row.get("empty") == Some(&Value::Bool(true)) {
                bump(obj, "empty");
                empty += 1;
            }
            if row.get("ok") == Some(&Value::Bool(false)) {
                bump(obj, "errors");
                errors += 1;
            }
        }
    }
    let issues = notable_queries(&queries);
    Ok(json!({
        "stamp": stamp,
        "session_id": session_id,
        "store": db.display().to_string(),
        "query_count": queries.len(),
        "truncated": truncated,
        "walk_incomplete": walk_incomplete,
        "empty": empty,
        "errors": errors,
        "tools": tools,
        "issues": issues,
        "gaps": gaps,
        "missing_capabilities": MISSING_CAPABILITIES.iter().map(|(name, note)| json!({
            "capability": name,
            "note": note,
        })).collect::<Vec<_>>(),
        "queries": queries,
    }))
}

fn notable_queries(queries: &[Map<String, Value>]) -> Vec<Value> {
    queries
        .iter()
        .filter(|row| {
            row.get("truncated") == Some(&Value::Bool(true))
                || row.get("empty") == Some(&Value::Bool(true))
                || row.get("ok") == Some(&Value::Bool(false))
                || row
                    .get("walk_incomplete")
                    .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
        })
        .map(|row| Value::Object(row.clone()))
        .collect()
}

fn read_gaps(db: &Path) -> anyhow::Result<Vec<Value>> {
    use devmap_extract::safe_fs::{Access, Creation, SafeFile};
    let mut file = match SafeFile::open(&gaps_path(db), Access::Read, Creation::Never) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    file.read_text(session_log::MAX_SESSION_BYTES)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn bump(obj: &mut Map<String, Value>, key: &str) {
    let next = obj.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
    obj.insert(key.into(), json!(next));
}

fn stamp_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("session-{secs}")
}

fn brief(report: &Value) -> String {
    let queries = report
        .get("query_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let truncated = report.get("truncated").and_then(Value::as_u64).unwrap_or(0);
    let incomplete = report
        .get("walk_incomplete")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let empty = report.get("empty").and_then(Value::as_u64).unwrap_or(0);
    let errors = report.get("errors").and_then(Value::as_u64).unwrap_or(0);
    let gaps = report
        .get("gaps")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    if queries == 0 && gaps == 0 {
        return "DevMap session: no queries logged. Prefer `devmap_*` MCP tools over GitNexus."
            .to_string();
    }
    format!(
        "DevMap session: {queries} queries, {truncated} truncated, {incomplete} walk_incomplete, \
{empty} empty, {errors} errors, {gaps} recorded gaps. Read truncated/walk_incomplete before \
treating an empty list as 'does not exist'. Do not fall back to GitNexus — record a gap instead."
    )
}

fn render_markdown(report: &Value) -> String {
    let mut out = String::new();
    out.push_str("# DevMap session insights\n\n");
    out.push_str(&format!("{}\n\n", brief(report)));
    out.push_str(&format!(
        "- stamp: {}\n- store: {}\n- session_id: {}\n\n",
        report.get("stamp").and_then(Value::as_str).unwrap_or("-"),
        report.get("store").and_then(Value::as_str).unwrap_or("-"),
        report
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("-"),
    ));
    out.push_str("## Tools\n\n");
    if let Some(tools) = report.get("tools").and_then(Value::as_object) {
        if tools.is_empty() {
            out.push_str("No MCP queries this session.\n\n");
        } else {
            out.push_str("| tool | count | truncated | walk_incomplete | empty | errors |\n");
            out.push_str("|---|---:|---:|---:|---:|---:|\n");
            for (name, stats) in tools {
                out.push_str(&format!(
                    "| `{name}` | {} | {} | {} | {} | {} |\n",
                    stats.get("count").and_then(Value::as_u64).unwrap_or(0),
                    stats.get("truncated").and_then(Value::as_u64).unwrap_or(0),
                    stats
                        .get("walk_incomplete")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    stats.get("empty").and_then(Value::as_u64).unwrap_or(0),
                    stats.get("errors").and_then(Value::as_u64).unwrap_or(0),
                ));
            }
            out.push('\n');
        }
    }
    out.push_str("## Issues worth fixing\n\n");
    match report.get("issues").and_then(Value::as_array) {
        Some(issues) if !issues.is_empty() => {
            for issue in issues {
                let tool = issue.get("tool").and_then(Value::as_str).unwrap_or("?");
                let mut flags = Vec::new();
                if issue.get("truncated") == Some(&Value::Bool(true)) {
                    flags.push("truncated");
                }
                if issue
                    .get("walk_incomplete")
                    .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
                {
                    flags.push("walk_incomplete");
                }
                if issue.get("empty") == Some(&Value::Bool(true)) {
                    flags.push("empty");
                }
                if issue.get("ok") == Some(&Value::Bool(false)) {
                    flags.push("error");
                }
                out.push_str(&format!(
                    "- `{tool}` [{}] args={} err={}\n",
                    flags.join(", "),
                    issue.get("args").unwrap_or(&Value::Null),
                    issue.get("error").unwrap_or(&Value::Null),
                ));
            }
            out.push('\n');
        }
        _ => out.push_str("None recorded.\n\n"),
    }
    out.push_str("## Agent-recorded gaps\n\n");
    match report.get("gaps").and_then(Value::as_array) {
        Some(gaps) if !gaps.is_empty() => {
            for gap in gaps {
                out.push_str(&format!("- {}\n", gap));
            }
            out.push('\n');
        }
        _ => out.push_str("None recorded this session.\n\n"),
    }
    out.push_str("## Capabilities GitNexus has that DevMap does not (yet)\n\n");
    if let Some(caps) = report.get("missing_capabilities").and_then(Value::as_array) {
        for cap in caps {
            out.push_str(&format!(
                "- `{}`: {}\n",
                cap.get("capability").and_then(Value::as_str).unwrap_or("?"),
                cap.get("note").and_then(Value::as_str).unwrap_or(""),
            ));
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A store path in its own directory, so parallel tests cannot collide.
    fn scratch_db(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQUENCE: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "devmap-gap-record-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root.join("store.sqlite")
    }

    fn ledger_lines(db: &Path) -> Vec<Value> {
        fs::read_to_string(gaps_path(db))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
            .collect()
    }

    /// The writer exists and what it writes is what `read_gaps` reads back.
    ///
    /// The ledger had a reader and no writer for as long as the guide has asked
    /// agents to keep it, so this is the first test that the two halves agree
    /// on a format at all.
    #[test]
    fn a_recorded_gap_is_one_line_the_reader_accepts() {
        let db = scratch_db("roundtrip");
        record_gap(
            &db,
            "devmap_dead_symbols",
            "GAP-X",
            "nothing came back",
            None,
            false,
        )
        .unwrap();
        let rows = read_gaps(&db).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["gap_id"], json!("GAP-X"));
        assert_eq!(rows[0]["tool"], json!("devmap_dead_symbols"));
        assert_eq!(rows[0]["reason"], json!("nothing came back"));
        assert!(
            rows[0]["ts_ms"].as_u64().is_some_and(|ms| ms > 0),
            "an entry carries when it was recorded: {rows:?}"
        );
        assert!(
            rows[0].get("resolved").is_none(),
            "an open gap says nothing about being resolved: {rows:?}"
        );
        let _ = fs::remove_dir_all(gaps_path(&db).parent().unwrap().parent().unwrap());
    }

    /// Appending, not rewriting: the second entry must not cost the first.
    #[test]
    fn recording_twice_keeps_both() {
        let db = scratch_db("append");
        record_gap(&db, "t", "GAP-1", "first", None, false).unwrap();
        record_gap(&db, "t", "GAP-2", "second", Some("/elsewhere"), true).unwrap();
        let rows = ledger_lines(&db);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0]["gap_id"], json!("GAP-1"));
        assert_eq!(rows[1]["gap_id"], json!("GAP-2"));
        assert_eq!(rows[1]["repo_path"], json!("/elsewhere"));
        assert_eq!(rows[1]["resolved"], json!(true));
        let _ = fs::remove_dir_all(gaps_path(&db).parent().unwrap().parent().unwrap());
    }

    /// A row whose key fields are blank records nothing, and reads exactly like
    /// a ledger nobody kept — so it is refused before the file is touched.
    #[test]
    fn a_gap_naming_nothing_is_refused_and_writes_no_line() {
        let db = scratch_db("blank");
        for (tool, id, reason) in [("", "GAP", "why"), ("t", "   ", "why"), ("t", "GAP", "\t")] {
            assert!(
                record_gap(&db, tool, id, reason, None, false).is_err(),
                "({tool:?}, {id:?}, {reason:?}) was accepted"
            );
        }
        assert!(
            !gaps_path(&db).exists(),
            "a refused entry must not leave a ledger behind"
        );
        let _ = fs::remove_dir_all(gaps_path(&db).parent().unwrap().parent().unwrap());
    }

    /// One oversized entry must not be able to fill the ledger the next agent
    /// has to append to.
    #[test]
    fn an_oversized_gap_is_refused_and_the_ledger_stays_appendable() {
        let db = scratch_db("oversized");
        record_gap(&db, "t", "GAP-SMALL", "fits", None, false).unwrap();
        let huge = "x".repeat(MAX_GAP_BYTES + 1);
        assert!(record_gap(&db, "t", "GAP-HUGE", &huge, None, false).is_err());
        record_gap(&db, "t", "GAP-AFTER", "still works", None, false).unwrap();
        let rows = ledger_lines(&db);
        assert_eq!(
            rows.iter().map(|r| r["gap_id"].clone()).collect::<Vec<_>>(),
            vec![json!("GAP-SMALL"), json!("GAP-AFTER")],
            "the refused entry left no partial line: {rows:?}"
        );
        let _ = fs::remove_dir_all(gaps_path(&db).parent().unwrap().parent().unwrap());
    }

    /// A reason spanning lines would otherwise split one entry into several,
    /// and every line after the first would fail to parse as JSON.
    #[test]
    fn a_multi_line_reason_stays_one_entry() {
        let db = scratch_db("newlines");
        record_gap(
            &db,
            "t",
            "GAP-NL",
            "line one\nline two\r\nline three",
            None,
            false,
        )
        .unwrap();
        let rows = ledger_lines(&db);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["reason"], json!("line one\nline two\r\nline three"));
        let _ = fs::remove_dir_all(gaps_path(&db).parent().unwrap().parent().unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn linked_session_reports_and_gaps_are_refused() {
        for case in ["report", "gaps"] {
            let root = std::env::temp_dir()
                .join(format!("devmap-session-read-{case}-{}", std::process::id()));
            fs::create_dir_all(root.join("sessions")).unwrap();
            let victim = root.join("outside");
            fs::write(&victim, "{\"private\":\"outside sentinel\"}\n").unwrap();
            let leaf = if case == "report" {
                "report.json"
            } else {
                "gaps.jsonl"
            };
            std::os::unix::fs::symlink(&victim, root.join("sessions").join(leaf)).unwrap();
            let db = root.join("store.sqlite");
            let refused = if case == "report" {
                newest_report(&db).is_err()
            } else {
                build_report(&db, None).is_err()
            };
            fs::remove_dir_all(root).unwrap();
            assert!(refused, "linked {case} source was accepted");
        }
    }

    #[test]
    fn brief_names_counts() {
        let report = json!({
            "query_count": 4,
            "truncated": 1,
            "walk_incomplete": 1,
            "empty": 2,
            "errors": 0,
            "gaps": []
        });
        let text = brief(&report);
        assert!(text.contains("4 queries"), "{text}");
        assert!(text.contains("1 truncated"), "{text}");
        assert!(text.contains("GitNexus"), "{text}");
    }

    #[test]
    fn empty_session_still_points_at_devmap() {
        let report = json!({
            "query_count": 0,
            "truncated": 0,
            "walk_incomplete": 0,
            "empty": 0,
            "errors": 0,
            "gaps": []
        });
        assert!(brief(&report).contains("no queries"));
    }
}
