//! Query and run files, and the `dcgrep rank` runner.
//!
//! Queries: `{"id","query","relevant":[paths],"source"}` (`relevant` is absent
//! in an unlabelled questions file). Runs: `{"id","ranked":[paths]}`, one line
//! per query. Every arm, including the dense ones that are produced elsewhere,
//! writes the run format and is scored by the same code.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

pub struct QueryRow {
    pub id: String,
    pub query: String,
    pub relevant: Vec<String>,
}

pub fn read_queries(path: &Path) -> Result<Vec<QueryRow>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))?;
        let id = v["id"]
            .as_str()
            .ok_or_else(|| format!("{}:{}: no id", path.display(), n + 1))?;
        let query = v["query"]
            .as_str()
            .ok_or_else(|| format!("{}:{}: no query", path.display(), n + 1))?;
        let relevant = v["relevant"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.push(QueryRow {
            id: id.into(),
            query: query.into(),
            relevant,
        });
    }
    Ok(out)
}

pub fn read_run(path: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = BTreeMap::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))?;
        let id = v["id"]
            .as_str()
            .ok_or_else(|| format!("{}:{}: no id", path.display(), n + 1))?;
        let ranked = v["ranked"]
            .as_array()
            .ok_or_else(|| format!("{}:{}: no ranked list", path.display(), n + 1))?
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or("ranked entry is not a string")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if out.insert(id.to_string(), ranked).is_some() {
            return Err(format!("{}: duplicate id {id}", path.display()));
        }
    }
    Ok(out)
}

pub fn write_run(path: &Path, rows: &[(String, Vec<String>)]) -> Result<(), String> {
    let mut text = String::new();
    for (id, ranked) in rows {
        text.push_str(&json!({"id": id, "ranked": ranked}).to_string());
        text.push('\n');
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Run `dcgrep rank` for one query and return the ranked paths.
pub fn dcgrep_rank(
    bin: &Path,
    root: &Path,
    query: &str,
    max_results: usize,
) -> Result<Vec<String>, String> {
    let mut child = Command::new(bin)
        .arg("rank")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    let request = json!({"query": query, "root": root, "max_results": max_results}).to_string();
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(request.as_bytes())
        .map_err(|e| format!("write request: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("dcgrep rank output for {query:?}: {e}"))?;
    if v["ok"] != json!(true) {
        return Err(format!("dcgrep rank refused {query:?}: {v}"));
    }
    Ok(v["files"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f["path"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}
