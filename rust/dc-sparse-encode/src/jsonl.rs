//! Schema 1 JSONL, the file `dcgrep index --sparse` imports.
//!
//! Written beside the target and renamed. A partial file must not be
//! importable as a whole one: an interrupted run leaves `*.partial`, which
//! the index will not be pointed at.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::post::{special_drop, terms_of};
use crate::tokenize::{document_ids, query_ids};
use crate::vocab;

pub struct Prepared<'a> {
    pub model_id: &'a str,
    pub tokens: &'a [String],
    pub weights: &'a [f64],
    pub vocab: &'a HashMap<String, u32>,
    pub max_positions: usize,
}

pub struct DocLine {
    pub path: String,
    pub total_terms: u32,
    pub terms: Vec<(u32, f64)>,
}

/// Header line. `query_ids` is this build's WordPiece with no specials, which
/// is what `dcgrep` will replay. Recording the model's tokenizer instead
/// would make the build refuse on `TABLE_SKEW` and pass everywhere else for
/// a reason that is not a regression.
pub fn header(prepared: &Prepared<'_>) -> Result<serde_json::Value, String> {
    if prepared.tokens.len() != prepared.weights.len() {
        return Err(format!(
            "vocabulary has {} tokens and {} query weights",
            prepared.tokens.len(),
            prepared.weights.len()
        ));
    }
    let parity: Vec<serde_json::Value> = vocab::parity_texts()
        .iter()
        .map(|text| {
            serde_json::json!({
                "text": text,
                "ids": query_ids(text, prepared.vocab),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "schema": 1,
        "model": prepared.model_id,
        "vocabulary": "wordpiece-30522",
        "vocab": prepared.tokens,
        "query_weights": prepared.weights,
        "parity": parity,
    }))
}

/// One batch of pooled rows, each `vocab` wide, in the same order as `paths`
/// and `texts`. A row that rounds to no terms is omitted, as the script
/// omitted it: an empty document line is not a document.
pub fn lines_from_pooled(
    prepared: &Prepared<'_>,
    root: &Path,
    paths: &[PathBuf],
    texts: &[String],
    pooled: &[f32],
) -> Result<Vec<DocLine>, String> {
    let width = prepared.tokens.len();
    if pooled.len() != paths.len() * width {
        return Err(format!(
            "pooled vector is {} floats for {} documents of width {width}",
            pooled.len(),
            paths.len()
        ));
    }
    if texts.len() != paths.len() {
        return Err(format!(
            "{} texts for {} paths",
            texts.len(),
            paths.len()
        ));
    }
    let drop = special_drop(prepared.tokens)?;
    let mut lines = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let row = &pooled[i * width..(i + 1) * width];
        let terms = terms_of(row, &drop);
        if terms.is_empty() {
            continue;
        }
        let ids = document_ids(&texts[i], prepared.vocab, prepared.max_positions)?;
        let rel = path.strip_prefix(root).unwrap_or(path);
        lines.push(DocLine {
            path: rel.to_string_lossy().replace('\\', "/"),
            total_terms: u32::try_from(ids.len()).unwrap_or(u32::MAX),
            terms,
        });
    }
    Ok(lines)
}

pub fn write_lines(out: &Path, header: &serde_json::Value, docs: &[DocLine]) -> Result<(), String> {
    let name = out
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("output path {} has no file name", out.display()))?;
    let staging = out.with_file_name(format!("{name}.partial"));
    let file = File::create(&staging)
        .map_err(|err| format!("could not write {}: {err}", staging.display()))?;
    let mut handle = BufWriter::new(file);
    writeln!(handle, "{}", serde_json::to_string(header).map_err(|e| e.to_string())?)
        .map_err(|err| format!("could not write {}: {err}", staging.display()))?;
    for doc in docs {
        let terms: Vec<serde_json::Value> = doc
            .terms
            .iter()
            .map(|(id, weight)| serde_json::json!([id, weight]))
            .collect();
        let line = serde_json::json!({
            "path": doc.path,
            "total_terms": doc.total_terms,
            "terms": terms,
        });
        writeln!(handle, "{}", serde_json::to_string(&line).map_err(|e| e.to_string())?)
            .map_err(|err| format!("could not write {}: {err}", staging.display()))?;
    }
    handle
        .flush()
        .map_err(|err| format!("could not write {}: {err}", staging.display()))?;
    std::fs::rename(&staging, out).map_err(|err| {
        format!(
            "could not move {} into place ({}): {err}",
            staging.display(),
            out.display()
        )
    })?;
    Ok(())
}

/// The wordpiece conformance fixture stays as it was recorded. Regenerating
/// it from this crate's tokeniser would compare the implementation to itself.
pub fn frozen_record_message() -> String {
    format!(
        "\
The wordpiece conformance fixture is frozen.

It was recorded from the model's Hugging Face tokeniser \
(opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini), \
14,810 pairs, and `dcgrep` checks it on every `cargo test`. Regenerating \
those pairs from this crate's WordPiece would be tautological: the test \
would compare the tokeniser to itself, and TABLE_SKEW — the 98 codepoints \
where this build's newer Unicode tables disagree with the model's — would \
disappear into a pass.

The neural reference (per-layer hidden states and pooled vectors) is not \
that fixture. Regenerate it, from the local cache and without downloading, with:

    uv venv /tmp/oracle && uv pip install --python /tmp/oracle torch transformers numpy
    /tmp/oracle/bin/python -I rust/dc-sparse-encode/tests/fixtures/record.py

scripts/encode-sparse.py is gone. --record does not rewrite either fixture."
    )
}


