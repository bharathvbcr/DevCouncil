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
    fs::create_dir_all(&dir)?;
    let json_path = dir.join(format!("{stamp}.json"));
    let md_path = dir.join(format!("{stamp}.md"));
    let rendered = serde_json::to_vec_pretty(&report)?;
    fs::write(&json_path, rendered)?;
    fs::write(&md_path, render_markdown(&report))?;
    session_log::rotate_live(db, &stamp);
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
    let dir = session_log::sessions_dir(db);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(None);
    };
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("gaps.json") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
            newest = Some((modified, path));
        }
    }
    let Some((_, path)) = newest else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&text)?))
}

fn build_report(db: &Path, session_id: Option<&str>) -> anyhow::Result<Value> {
    let queries = session_log::read_live(db);
    let gaps = read_gaps(db);
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

fn read_gaps(db: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(gaps_path(db)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
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
