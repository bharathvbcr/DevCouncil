//! Best-effort MCP query log for session-end insights.
//!
//! A code index that does not record what it withheld cannot tell its author
//! where to improve it. Every `tools/call` appends one JSON line next to the
//! store; a failure to log never fails the call, because an agent's answer
//! matters more than our telemetry.
//!
//! Path: `<store-dir>/sessions/live.jsonl`.

#[cfg(test)]
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_extract::safe_fs::SafeFile;
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};

/// Directory that holds the live log and the rotated session reports.
pub fn sessions_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("sessions")
}

pub fn live_log_path(db_path: &Path) -> PathBuf {
    sessions_dir(db_path).join("live.jsonl")
}

/// Honesty flags extracted from a tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct Honesty {
    pub truncated: bool,
    pub walk_incomplete: Option<Value>,
    pub empty: bool,
}

/// Classify a structured tool payload.
///
/// `truncated` / `walk_incomplete` are searched a few levels down because
/// explore answers nest them per part. `empty` is a top-level judgement only:
/// a nested empty array inside a populated result is not "the tool found
/// nothing".
pub fn classify(structured: &Value) -> Honesty {
    Honesty {
        truncated: find_true(structured, "truncated", 4),
        walk_incomplete: find_truthy(structured, "walk_incomplete", 4),
        empty: looks_empty(structured),
    }
}

/// Append one query record. Never returns an error the caller should honour.
pub fn append_query(
    db_path: &Path,
    tool: &str,
    args: Option<&Value>,
    structured: Option<&Value>,
    error: Option<&str>,
    latency_ms: u64,
) {
    let honesty = structured.map(classify).unwrap_or(Honesty {
        truncated: false,
        walk_incomplete: None,
        empty: false,
    });
    let record = json!({
        "ts_ms": now_ms(),
        "tool": tool,
        "args": sanitize_args(args),
        "ok": error.is_none(),
        "error": error,
        "truncated": honesty.truncated,
        "walk_incomplete": honesty.walk_incomplete,
        "empty": honesty.empty && error.is_none(),
        "latency_ms": latency_ms,
    });
    let Ok(line) = serde_json::to_string(&record) else {
        return;
    };
    // Only log beside a store that actually exists. An unresolved slot's
    // preview path was `store_path(cwd)`, which under a host launched from
    // `$HOME` created `~/.devmap/.../sessions/live.jsonl`.
    if !db_path.is_file() {
        return;
    }
    let result = (|| -> std::io::Result<()> {
        use devmap_extract::safe_fs::{Access, Creation, SafeFile};
        // Inspect the raw database ancestry too: a linked state directory must
        // not become permission to create telemetry in its target.
        let _database = SafeFile::open(db_path, Access::Read, Creation::Never)?;
        let mut file =
            SafeFile::open(&live_log_path(db_path), Access::Append, Creation::IfMissing)?;
        if line.len() > 16 * 1024
            || file.metadata()?.len().saturating_add(line.len() as u64 + 1) > MAX_SESSION_BYTES
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session log limit reached; finish or rotate the session",
            ));
        }
        append_line(&mut file, &line)
    })();
    if let Err(error) = result {
        tracing::warn!("MCP query logging skipped: {error}");
    }
}

/// Append one record and its terminator in a single write.
///
/// `writeln!` sends the body and the newline as two writes, and `SafeFile`
/// derefs to an unbuffered `File`, so those are two `write` calls. `O_APPEND`
/// makes each one atomic against other appenders but says nothing about the
/// pair, so a second process lands in the gap and the two records share a
/// line. That is not theoretical: eight concurrent writers tore hundreds of
/// 2000 records, and the logs this machine had accumulated held sixteen such
/// lines across two repositories.
///
/// One `write_all` of body-plus-newline closes that window. A short write on a
/// full disk could still split a record, which is why the reader counts what
/// it cannot parse instead of trusting this to be perfect.
pub fn append_line(file: &mut SafeFile, line: &str) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    file.write_all(&bytes)
}

/// One ledger read: the records that parsed, and how many lines did not.
#[derive(Debug, Clone)]
pub struct Ledger<T> {
    pub records: Vec<T>,
    /// Lines that were present but did not parse.
    ///
    /// Carried rather than folded away because a read that lost records must
    /// never look like a read that found none. The count is what lets a report
    /// say "93 records, 4 unreadable" instead of quietly reporting 93.
    pub malformed: u64,
}

// Hand-written so an empty read needs nothing of `T`: `derive(Default)` would
// bound it on `T: Default` for no reason, since the empty ledger holds no `T`.
impl<T> Default for Ledger<T> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            malformed: 0,
        }
    }
}

/// Parse a JSON-lines ledger, skipping and counting what will not parse.
///
/// Failing closed on the first bad line is what made one torn record fatal:
/// `session-report` returned `Err`, so the rotation that would have retired
/// the bad line never ran, the live log grew instead, and at
/// [`MAX_SESSION_BYTES`] logging would have stopped altogether. The ledger
/// could not recover from a single tear without a human deleting the file.
///
/// Skipping is therefore the recovering choice. Counting is what keeps it
/// honest.
pub fn parse_jsonl<T: DeserializeOwned>(text: &str) -> Ledger<T> {
    let mut records = Vec::new();
    let mut malformed = 0u64;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(line) {
            Ok(record) => records.push(record),
            Err(_) => malformed = malformed.saturating_add(1),
        }
    }
    Ledger { records, malformed }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const ARGS_CAP: usize = 2048;
pub const MAX_SESSION_BYTES: u64 = 8 * 1024 * 1024;

fn sanitize_args(args: Option<&Value>) -> Value {
    let Some(value) = args else {
        return Value::Null;
    };
    let mut cloned = value.clone();
    if let Some(obj) = cloned.as_object_mut() {
        if let Some(content) = obj.remove("content") {
            obj.insert(
                "content".into(),
                json!({"_omitted": true, "bytes": content.to_string().len()}),
            );
        }
    }
    match serde_json::to_string(&cloned) {
        Ok(text) if text.len() > ARGS_CAP => json!({
            "_truncated_args": true,
            "preview": text.chars().take(ARGS_CAP).collect::<String>(),
        }),
        Ok(_) => cloned,
        Err(_) => Value::Null,
    }
}

fn looks_empty(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    for key in [
        "items",
        "definitions",
        "paths",
        "groups",
        "hits",
        "results",
        "clones",
    ] {
        if let Some(Value::Array(items)) = obj.get(key) {
            return items.is_empty();
        }
    }
    false
}

fn find_true(value: &Value, key: &str, depth: u8) -> bool {
    matches!(find_flag(value, key, depth), Some(Value::Bool(true)))
}

fn find_truthy(value: &Value, key: &str, depth: u8) -> Option<Value> {
    match find_flag(value, key, depth) {
        Some(Value::Bool(false)) | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        other => other,
    }
}

fn find_flag(value: &Value, key: &str, depth: u8) -> Option<Value> {
    if depth == 0 {
        return None;
    }
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                if !found.is_null() {
                    return Some(found.clone());
                }
            }
            for nested in map.values() {
                if let Some(found) = find_flag(nested, key, depth - 1) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for nested in items.iter().take(8) {
                if let Some(found) = find_flag(nested, key, depth - 1) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Load the live log as parsed records. Missing file is an empty session.
///
/// Unreadable lines are reported in [`Ledger::malformed`], not returned as an
/// error: see [`parse_jsonl`] for why one torn record must not cost the whole
/// session.
pub fn read_live(db_path: &Path) -> std::io::Result<Ledger<Map<String, Value>>> {
    use devmap_extract::safe_fs::{Access, Creation};
    let mut file = match SafeFile::open(&live_log_path(db_path), Access::Read, Creation::Never) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Ledger::default()),
        Err(error) => return Err(error),
    };
    let text = file.read_text_prefix(MAX_SESSION_BYTES)?;
    Ok(parse_jsonl(&text))
}

/// Rotate within the checked session directory. Only a missing log is a no-op.
pub fn rotate_live(db_path: &Path, stamp: &str) -> std::io::Result<bool> {
    use devmap_extract::safe_fs::{Access, Creation, PinnedDir};
    let directory = match PinnedDir::open(&sessions_dir(db_path), false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match directory.open_file(
        std::ffi::OsStr::new("live.jsonl"),
        Access::Read,
        Creation::Never,
    ) {
        Ok(file) => file.require_owned()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    let name = format!("{stamp}.jsonl");
    match directory.open_file(std::ffi::OsStr::new(&name), Access::Read, Creation::Never) {
        Ok(file) => file.require_owned()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // rename_child validates both names as single path components.
    directory.rename_child(
        std::ffi::OsStr::new("live.jsonl"),
        std::ffi::OsStr::new(&name),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classify_reads_nested_honesty_flags() {
        let value = json!({
            "definitions": [{"name": "foo"}],
            "callers": {"items": [], "truncated": true, "walk_incomplete": "depth cap"}
        });
        let honesty = classify(&value);
        assert!(honesty.truncated);
        assert_eq!(
            honesty.walk_incomplete,
            Some(Value::String("depth cap".into()))
        );
        assert!(!honesty.empty);
    }

    #[test]
    fn empty_is_top_level_only() {
        let populated = json!({"items": [{"a": 1}], "nested": {"items": []}});
        assert!(!classify(&populated).empty);
        let vacant = json!({"items": []});
        assert!(classify(&vacant).empty);
    }

    #[test]
    fn append_then_read_round_trips() {
        let dir = std::env::temp_dir().join(format!("devmap-session-log-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let db = dir.join("devmap.sqlite");
        fs::write(&db, b"store-present").unwrap();
        append_query(
            &db,
            "devmap_search",
            Some(&json!({"query": "Foo"})),
            Some(&json!({"items": [], "truncated": false})),
            None,
            4,
        );
        let ledger = read_live(&db).unwrap();
        let rows = &ledger.records;
        assert_eq!(rows.len(), 1);
        assert_eq!(ledger.malformed, 0);
        assert_eq!(rows[0]["tool"], "devmap_search");
        assert_eq!(rows[0]["empty"], true);
        assert_eq!(rows[0]["ok"], true);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_content_is_omitted() {
        let args = json!({"file": "a.rs", "content": "fn huge() {}"});
        let sanitized = sanitize_args(Some(&args));
        assert!(sanitized["content"]["_omitted"].as_bool().unwrap());
        assert!(sanitized.get("file").is_some());
    }

    #[test]
    fn append_does_not_create_a_log_when_the_store_file_is_absent() {
        let dir =
            std::env::temp_dir().join(format!("devmap-session-log-absent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("codeintel").join("devmap.sqlite");
        append_query(&db, "devmap_status", None, None, Some("no store"), 1);
        assert!(
            !live_log_path(&db).exists(),
            "an unresolved slot must not create {} from a missing store",
            live_log_path(&db).display()
        );
        assert!(
            !sessions_dir(&db).exists(),
            "must not create the sessions directory for a store that was never opened"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
