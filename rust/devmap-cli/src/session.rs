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

/// Known questions an agent asks that the DevMap MCP server does not answer as
/// a tool: `(capability, the tool it would be, note)`.
///
/// Each entry names the tool so the list can be checked against the registry
/// the server publishes — `tests::no_missing_capability_is_a_served_tool`. The
/// list said `detect_changes`, `cypher` and `route_map` were missing while
/// `devmap_blast` already answered the first, and it cited a retired Python
/// `dev map --pdg` as the PDG's home for weeks after `devmap pdg` landed; a
/// list nothing checks drifts in exactly that direction.
///
/// What is still CLI-only, and why it stays so (decided 2026-10-06 with
/// `GAP-P7-DEVMAP-MCP-CLI`): `pdg` reads one Python file from disk rather than
/// the index and its sinks are a heuristic list — a security review's
/// question, not graph navigation; `shape-check` sweeps every route, which is
/// an audit pass, while the per-route answer an agent changing a route needs
/// is already in `devmap_api_impact`.
const MISSING_CAPABILITIES: &[(&str, &str, &str)] = &[
    (
        "rename",
        "devmap_rename",
        "graph-backed coordinated rename; workaround: `devmap_search` + `devmap_preview`",
    ),
    (
        "pdg_query",
        "devmap_pdg",
        "per-function control/data dependence graphs; CLI-only: `devmap pdg FILE` (Python files)",
    ),
    (
        "taint_explain",
        "devmap_pdg",
        "statements reaching a known sink; CLI-only: `devmap pdg FILE --taint` (Python, heuristic sinks)",
    ),
    (
        "shape_check",
        "devmap_shape_check",
        "repo-wide handler-vs-consumer key comparison; CLI-only: `devmap shape-check`; per route, \
         `devmap_api_impact` carries the mismatches",
    ),
    (
        "clusters_processes",
        "devmap_clusters",
        "precomputed execution-flow resources; workaround: subsystems in repo_map.json + `devmap_explore`",
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
    session_log::append_line(&mut file, &line)?;
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
    // Rotation is the recovery, so it must not sit behind the thing that
    // failed. A log that cannot be summarised — because it is over the read
    // limit, or not UTF-8 — would otherwise stay live forever: the appender
    // refuses to add to it past the cap and the reader refuses to read it, so
    // nothing retires it and telemetry stops for good. Retiring it still costs
    // nothing: rotation is a rename, and the bytes keep their own file.
    let report = match build_report(db, session_id) {
        Ok(report) => report,
        Err(error) => {
            let stamp = stamp_now();
            match session_log::rotate_live(db, &stamp) {
                Ok(true) => {
                    return Err(error.context(format!(
                        "live log could not be summarised; it was rotated to {stamp}.jsonl so the \
                         next session starts clean"
                    )))
                }
                Ok(false) => return Err(error),
                Err(rotate_error) => {
                    return Err(error.context(format!("and rotation failed too: {rotate_error}")))
                }
            }
        }
    };
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
    outln!("{}", brief(&report));
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
    outln!("{}", brief(&report));
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
    let live = session_log::read_live(db)?;
    let gap_ledger = read_gaps(db)?;
    let queries = live.records;
    let (gaps, resolved_gaps) = open_gaps(gap_ledger.records);
    // Records that were written but cannot be read back. Reported next to the
    // counts they are missing from, so a degraded report is visibly degraded.
    let unreadable = live.malformed.saturating_add(gap_ledger.malformed);
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
    // The live log is shared by every session in the repository, so
    // `query_count` is the log's, not the ending session's. Per-session counts
    // come from the id each row carries; a row from a host that names no
    // session is counted as unattributed rather than credited to anyone.
    let mut by_session: Map<String, Value> = Map::new();
    let mut unattributed = 0u64;
    for row in &queries {
        match row.get("session_id").and_then(Value::as_str) {
            Some(id) => bump(&mut by_session, id),
            None => unattributed += 1,
        }
    }
    let session_query_count =
        session_id.map(|id| by_session.get(id).and_then(Value::as_u64).unwrap_or(0));
    Ok(json!({
        "stamp": stamp,
        "session_id": session_id,
        "store": db.display().to_string(),
        "query_count": queries.len(),
        "session_query_count": session_query_count,
        "queries_by_session": by_session,
        "unattributed_query_count": unattributed,
        "unreadable_lines": unreadable,
        "unreadable_query_lines": live.malformed,
        "unreadable_gap_lines": gap_ledger.malformed,
        "truncated": truncated,
        "walk_incomplete": walk_incomplete,
        "empty": empty,
        "errors": errors,
        "tools": tools,
        "issues": issues,
        "gaps": gaps,
        "resolved_gaps": resolved_gaps,
        "missing_capabilities": MISSING_CAPABILITIES.iter().map(|(name, tool, note)| json!({
            "capability": name,
            "tool": tool,
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

/// The key a gap is opened and closed under: its `gap_id`.
///
/// A row with no usable `gap_id` (the writer refuses to write one, but a ledger
/// that was appended to by hand holds some) is keyed by when it was written, as
/// `unkeyed@<ts_ms>`, so it can be closed by recording that key as the
/// `--gap-id`. A row with no timestamp either is keyed by its position and
/// stays open: nothing can name it, so nothing can close it.
fn gap_key(row: &Value, index: usize) -> String {
    let id = row
        .get("gap_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if let Some(id) = id {
        return id.to_string();
    }
    match row.get("ts_ms").and_then(Value::as_u64) {
        Some(ts) => format!("unkeyed@{ts}"),
        None => format!("unkeyed#{index}"),
    }
}

/// Collapse the append-only gap ledger to the gaps that are still open.
///
/// The ledger is a history: a gap is recorded, later recorded again with
/// `resolved: true`, and may be recorded open again if it recurs. Reporting every
/// line made `session-report` list work that had been closed — the report said
/// "35 recorded gaps" of which a third were fixed — so the latest line for each
/// key decides, and a key whose latest line is `resolved` is counted, not shown.
/// Returns the open rows in the order their keys first appeared, each as its
/// latest line, and how many keys are closed.
fn open_gaps(rows: Vec<Value>) -> (Vec<Value>, usize) {
    use std::collections::HashMap;
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, Value> = HashMap::new();
    for (index, mut row) in rows.into_iter().enumerate() {
        let key = gap_key(&row, index);
        if key.starts_with("unkeyed") {
            if let Some(object) = row.as_object_mut() {
                // Shown so a person can close it: the key is not in the row.
                object.insert("gap_key".into(), json!(key));
            }
        }
        if latest.insert(key.clone(), row).is_none() {
            order.push(key);
        }
    }
    let mut open = Vec::new();
    let mut resolved = 0usize;
    for key in order {
        let Some(row) = latest.remove(&key) else {
            continue;
        };
        if row.get("resolved") == Some(&Value::Bool(true)) {
            resolved += 1;
        } else {
            open.push(row);
        }
    }
    (open, resolved)
}

/// Read the gap ledger, counting rather than failing on an unreadable line.
///
/// Several agents append here concurrently, so this ledger can tear the same
/// way the live log did; one bad line must not cost the report the other
/// entries. See [`session_log::parse_jsonl`].
fn read_gaps(db: &Path) -> anyhow::Result<session_log::Ledger<Value>> {
    use devmap_extract::safe_fs::{Access, Creation, SafeFile};
    let mut file = match SafeFile::open(&gaps_path(db), Access::Read, Creation::Never) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(session_log::Ledger::default())
        }
        Err(error) => return Err(error.into()),
    };
    let text = file.read_text_prefix(session_log::MAX_SESSION_BYTES)?;
    Ok(session_log::parse_jsonl(&text))
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
    let unreadable = report
        .get("unreadable_lines")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // Stays inside the first sentence: the SessionStart hook keeps only that
    // much, and a report that dropped records has to say so where it is read.
    let lost = if unreadable == 0 {
        String::new()
    } else {
        format!(", {unreadable} unreadable lines")
    };
    if queries == 0 && gaps == 0 {
        if unreadable > 0 {
            return format!(
                "DevMap session: 0 readable queries{lost}. The log was written but could not be \
parsed back; it rotates with this report, so the next session starts clean."
            );
        }
        return "DevMap session: no queries logged. Prefer `devmap_*` MCP tools over GitNexus."
            .to_string();
    }
    format!(
        "DevMap session: {queries} queries, {truncated} truncated, {incomplete} walk_incomplete, \
{empty} empty, {errors} errors, {gaps} open gaps{lost}. Read truncated/walk_incomplete before \
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
    out.push_str("## Open agent-recorded gaps\n\n");
    match report.get("gaps").and_then(Value::as_array) {
        Some(gaps) if !gaps.is_empty() => {
            for gap in gaps {
                out.push_str(&format!("- {}\n", gap));
            }
            out.push('\n');
        }
        _ => out.push_str("None open.\n\n"),
    }
    if let Some(resolved) = report
        .get("resolved_gaps")
        .and_then(Value::as_u64)
        .filter(|count| *count > 0)
    {
        out.push_str(&format!("{resolved} recorded gaps are resolved.\n\n"));
    }
    out.push_str("## Questions DevMap answers only on the CLI, or not yet\n\n");
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

    /// The list of missing capabilities is checked against the registry the
    /// MCP server publishes, in both directions that have gone wrong.
    ///
    /// No entry may name a tool the server serves — `detect_changes` sat here
    /// beside a live `devmap_blast`. And the capabilities this list used to
    /// carry and the server now answers must stay answered, by the tool the
    /// session report's readers were told to wait for: removing one of those
    /// tools would otherwise leave nothing saying the gap reopened.
    #[test]
    fn no_missing_capability_is_a_served_tool() {
        let served = devmap_serve::mcp::TOOL_NAMES;
        for (capability, tool, _) in MISSING_CAPABILITIES {
            assert!(
                tool.starts_with("devmap_"),
                "{capability}: name the MCP tool it would be, not a CLI command: {tool}"
            );
            assert!(
                !served.contains(tool),
                "{capability} is listed as missing, but the MCP server serves {tool}"
            );
        }
        for (capability, tool) in [
            ("detect_changes", "devmap_blast"),
            ("cypher", "devmap_cypher"),
            ("route_map", "devmap_routes"),
            ("api_impact", "devmap_api_impact"),
        ] {
            assert!(
                served.contains(&tool),
                "{capability} left this list because {tool} answers it; {tool} is gone, so \
                 the capability is missing again and belongs back in MISSING_CAPABILITIES"
            );
        }
    }

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
        let ledger = read_gaps(&db).unwrap();
        let rows = &ledger.records;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(ledger.malformed, 0, "a fresh entry is readable: {rows:?}");
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

    fn open_ids(db: &Path) -> Vec<String> {
        let report = build_report(db, None).unwrap();
        report["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| gap_key(row, 0))
            .collect()
    }

    /// The ledger is a history; the report is what is still open. Before this
    /// the report listed every line, so a gap closed with `--resolved` kept
    /// showing, and a gap recorded twice (opened, then resolved) showed twice.
    #[test]
    fn a_resolved_gap_is_counted_and_not_listed() {
        let db = scratch_db("resolved");
        record_gap(&db, "t", "GAP-OPEN", "still broken", None, false).unwrap();
        record_gap(&db, "t", "GAP-DONE", "was broken", None, false).unwrap();
        record_gap(&db, "t", "GAP-DONE", "fixed in abc123", None, true).unwrap();

        let report = build_report(&db, None).unwrap();
        assert_eq!(open_ids(&db), vec!["GAP-OPEN".to_string()], "{report}");
        assert_eq!(report["resolved_gaps"], json!(1), "{report}");
        let text = brief(&report);
        assert!(text.contains("1 open gaps"), "{text}");
        let markdown = render_markdown(&report);
        assert!(!markdown.contains("GAP-DONE"), "{markdown}");
        assert!(
            markdown.contains("1 recorded gaps are resolved"),
            "{markdown}"
        );
        let _ = fs::remove_dir_all(gaps_path(&db).parent().unwrap().parent().unwrap());
    }

    /// The live log is shared by every session in the repository. The report
    /// for the session that ended counts that session's queries, and does not
    /// credit a row whose host named no session to anyone.
    #[test]
    fn the_report_counts_the_ending_sessions_queries_apart_from_the_logs() {
        let db = scratch_db("by-session");
        fs::write(&db, b"store-present").unwrap();
        for session in [Some("ended"), Some("ended"), Some("other"), None] {
            session_log::append_query(&db, session, "devmap_search", None, None, None, 1);
        }
        let report = build_report(&db, Some("ended")).unwrap();
        assert_eq!(report["session_id"], json!("ended"), "{report}");
        assert_eq!(report["query_count"], json!(4), "{report}");
        assert_eq!(report["session_query_count"], json!(2), "{report}");
        assert_eq!(report["queries_by_session"]["other"], json!(1), "{report}");
        assert_eq!(report["unattributed_query_count"], json!(1), "{report}");
        let _ = fs::remove_dir_all(db.parent().unwrap());
    }

    /// The latest line decides, so a gap that recurs after being closed is open
    /// again, and the recurrence carries the new reason.
    #[test]
    fn a_gap_recorded_again_after_resolution_is_open_with_its_new_reason() {
        let db = scratch_db("reopen");
        record_gap(&db, "t", "GAP-R", "first", None, false).unwrap();
        record_gap(&db, "t", "GAP-R", "fixed", None, true).unwrap();
        record_gap(&db, "t", "GAP-R", "it came back", None, false).unwrap();
        let report = build_report(&db, None).unwrap();
        let gaps = report["gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 1, "{report}");
        assert_eq!(gaps[0]["reason"], json!("it came back"));
        assert_eq!(report["resolved_gaps"], json!(0), "{report}");
        let _ = fs::remove_dir_all(gaps_path(&db).parent().unwrap().parent().unwrap());
    }

    /// A line with no `gap_id` — only a hand-written ledger has one, since the
    /// writer refuses them — stays visible as open rather than being dropped,
    /// names the key that closes it, and is closed by recording that key.
    #[test]
    fn a_line_without_a_gap_id_is_shown_and_can_be_closed_by_its_key() {
        let db = scratch_db("unkeyed");
        let path = gaps_path(&db);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"ts_ms\":1789109200000,\"tool\":\"devmap_status\",\"reason\":\"old\"}\n\
             {\"tool\":\"t\",\"gap_id\":\"  \",\"reason\":\"blank id, no timestamp\"}\n",
        )
        .unwrap();

        let report = build_report(&db, None).unwrap();
        let gaps = report["gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 2, "an id-less line must not vanish: {report}");
        assert_eq!(gaps[0]["gap_key"], json!("unkeyed@1789109200000"));
        assert_eq!(gaps[1]["gap_key"], json!("unkeyed#1"));

        record_gap(&db, "t", "unkeyed@1789109200000", "stale", None, true).unwrap();
        let report = build_report(&db, None).unwrap();
        assert_eq!(report["gaps"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(report["resolved_gaps"], json!(1), "{report}");
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
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

    /// The table is read by agents as a list of what to work around, so a note
    /// that names a retired Python command, or lists a capability another
    /// command covers, sends them to something that is not there.
    #[test]
    fn the_capability_table_names_no_python_surface_and_omits_what_blast_covers() {
        for (name, _tool, note) in MISSING_CAPABILITIES {
            assert_ne!(*name, "detect_changes", "`devmap blast` covers it");
            assert!(!note.contains("Python `dev"), "{name}: {note}");
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
