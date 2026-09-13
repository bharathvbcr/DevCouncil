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
