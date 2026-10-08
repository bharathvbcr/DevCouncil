//! Offline doc-side sparse encoder for `dcgrep index --sparse`.
//!
//! The model runs here, on tessl, and nowhere in `dcgrep`. A query is scored
//! from the weight table this file's header carries, so the search binary
//! stays static and inference-free. tessl is Apple silicon only, and so is
//! the forward; `--self-test` and the frozen-fixture notice do not need it.
//!
//! The torch fixtures under `tests/fixtures/` are the parity reference.
//! `tests/fixtures/record.py` regenerates them from the local Hugging Face
//! cache and does not download.

mod bench;
mod fileset;
mod jsonl;
mod post;
mod selftest;
mod tokenize;
mod vocab;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod model;

pub use bench::{schedule, Side, MIN_REPEATS};
pub use fileset::{walk, Selection, DEFAULT_MAX_FILES};
pub use jsonl::{frozen_record_message, DocLine};
pub use post::{round4, special_drop, terms_of, MAX_TERMS_PER_DOCUMENT};
pub use tokenize::{document_ids, query_ids, QUERY_CAP};
pub use vocab::{densify, lookup, parity_texts, read_vocab, snapshot, DEFAULT_MODEL};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use model::Loaded;

/// Encode `root` to schema-1 JSONL at `out`.
///
/// `max_files_set` is whether the operator passed `--max-files`. The default
/// budget refusing a truncated listing, and an explicit budget warning about
/// one, are different requests: a prefix under the default is an index that
/// claims to be learned and is BM25 past the cut.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn encode_repository(
    root: &std::path::Path,
    out: &std::path::Path,
    model_id: &str,
    batch_size: usize,
    max_files: usize,
    max_files_set: bool,
    force_walk: bool,
) -> Result<usize, String> {
    if batch_size == 0 {
        return Err("--batch-size must be at least 1".into());
    }
    let root = root.canonicalize().map_err(|err| format!("{}: {err}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let selection = fileset::select(&root, max_files, force_walk)?;
    if selection.truncated {
        if !max_files_set {
            return Err(format!(
                "the file list was truncated at {max_files}. This encoding would cover a prefix \
                 of the repository and leave the rest ranking by BM25. Pass --max-files \
                 explicitly to encode a prefix on purpose."
            ));
        }
        eprintln!(
            "warning: encoding a prefix — the file list was truncated at the --max-files you \
             passed ({max_files}). Files past it will be searchable but ranked by BM25, and the \
             build will count them in lexical_unindexed."
        );
    }
    if selection.files.is_empty() {
        return Err(format!("no encodable files under {}", root.display()));
    }

    let loaded = model::Loaded::open(model_id)?;
    eprintln!(
        "encoding {} files with {model_id} on Metal",
        selection.files.len()
    );
    let prepared = jsonl::Prepared {
        model_id: &loaded.model_id,
        tokens: &loaded.tokens,
        weights: &loaded.weights,
        vocab: &loaded.vocab,
        max_positions: loaded.max_positions,
    };
    let header = jsonl::header(&prepared)?;
    let mut docs = Vec::new();
    let total = selection.files.len();
    for (index, chunk) in selection.files.chunks(batch_size).enumerate() {
        let mut paths = Vec::new();
        let mut texts = Vec::new();
        for path in chunk {
            match std::fs::read(path) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => {
                        paths.push(path.clone());
                        texts.push(text);
                    }
                    Err(_) => eprintln!("note: skipping {} (not utf-8)", path.display()),
                },
                Err(err) => eprintln!("note: skipping {}: {err}", path.display()),
            }
        }
        if !texts.is_empty() {
            let ids = texts
                .iter()
                .map(|text| document_ids(text, &loaded.vocab, loaded.max_positions))
                .collect::<Result<Vec<_>, _>>()?;
            let forward = loaded.forward(&ids, false)?;
            docs.extend(jsonl::lines_from_pooled(
                &prepared,
                &root,
                &paths,
                &texts,
                &forward.pooled,
            )?);
        }
        let finished = ((index + 1) * batch_size).min(total);
        eprintln!("  {finished}/{total}");
    }
    jsonl::write_lines(out, &header, &docs)?;
    eprintln!("wrote {} documents to {}", docs.len(), out.display());
    eprintln!();
    eprintln!("Build the index with it:");
    eprintln!(
        "    echo '{}' | dcgrep index",
        serde_json::json!({"root": root, "sparse": out})
    );
    Ok(docs.len())
}

pub fn self_test() -> Result<(), String> {
    selftest::self_test()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn bench(
    model_id: &str,
    files: &[std::path::PathBuf],
    repeats: usize,
) -> Result<bench::Report, String> {
    bench::run(model_id, files, repeats)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn format_bench(model_id: &str, report: &bench::Report) -> String {
    bench::format_report(model_id, report)
}
