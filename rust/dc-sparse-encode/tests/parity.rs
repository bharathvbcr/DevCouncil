//! The tessl forward against the recorded torch fp32 fixtures.
//!
//! Tokenisation is exact. Pooled weights are compared before `round(w, 4)`,
//! with the 1e-4 bound the rest of tessl uses for an f32 forward. Hidden
//! states are the embedding output and the residual after each layer, at the
//! positions the fixture kept; a wild miss there means a pooled match is luck.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

mod common;

use std::path::{Path, PathBuf};

use dc_grep::{IndexRequest, build_index};
use dc_sparse_encode::{document_ids, encode_repository, lookup, read_vocab, Loaded};

const MODELS: &[(&str, &str)] = &[
    (
        "doc-v2-mini",
        "opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini",
    ),
    (
        "doc-v3-distill",
        "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill",
    ),
];

const POOLED_ABS: f32 = 1e-4;
/// Six layers of f32. Wider than the pooled bound so a 1e-4 pooled match
/// cannot hide a layer that diverged and came back; still tight enough that
/// a wrong GELU or a missed residual fails.
const HIDDEN_ABS: f32 = 1e-3;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn the_tessl_forward_matches_the_torch_fixtures() {
    let tokens = read_vocab(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dc-grep/tests/fixtures/wordpiece-vocab.txt"),
    )
    .unwrap();
    let vocab = lookup(&tokens);

    for (short, model_id) in MODELS {
        let dir = fixtures().join(short);
        let names: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("documents.json")).unwrap())
                .unwrap();
        let loaded = Loaded::open(model_id).unwrap_or_else(|err| panic!("{model_id}: {err}"));
        let mut sequences = Vec::new();
        for name in &names {
            let text = std::fs::read_to_string(fixtures().join("corpus/src").join(name)).unwrap();
            let ids = document_ids(&text, &vocab, loaded.max_positions).unwrap();
            let recorded = common::i64(&dir.join(format!("{name}.ids.npy")));
            let want: Vec<u32> = recorded.data.iter().map(|id| *id as u32).collect();
            assert_eq!(ids, want, "{short} {name}");
            let forward = loaded.forward(std::slice::from_ref(&ids), true).unwrap();
            let pooled = common::f32(&dir.join(format!("{name}.pooled.npy")));
            assert_pooled(&forward.pooled, &pooled.data, &format!("{short} {name}"));
            let positions = common::i64(&dir.join(format!("{name}.positions.npy")));
            let hidden = common::f32(&dir.join(format!("{name}.hidden.npy")));
            assert_hidden(
                &forward.trace,
                &hidden,
                &positions.data,
                loaded.hidden,
                &format!("{short} {name}"),
            );
            sequences.push(ids);
        }
        let batch = loaded.forward(&sequences, false).unwrap();
        let recorded = common::f32(&dir.join("batch.pooled.npy"));
        assert_eq!(recorded.shape, vec![names.len(), loaded.vocab_size]);
        assert_pooled(&batch.pooled, &recorded.data, &format!("{short} batch"));
        drop(loaded);
    }
}

#[test]
fn the_encoder_jsonl_imports_through_dcgrep() {
    // Copy the corpus. build_index writes `.devcouncil` beside the root, and
    // the fixture tree is not a place for that cache.
    let room = std::env::temp_dir().join(format!(
        "dc-sparse-encode-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _clean = CleanDir(room.clone());
    let corpus = room.join("corpus");
    copy_dir(&fixtures().join("corpus"), &corpus).unwrap();
    let out = room.join("sparse.jsonl");
    encode_repository(
        &corpus,
        &out,
        "opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini",
        4,
        100,
        true,
        true,
    )
    .unwrap_or_else(|err| panic!("{err}"));
    let built = build_index(&IndexRequest {
        root: corpus,
        max_files: 0,
        sparse: Some(out),
    })
    .unwrap_or_else(|err| panic!("{err}"));
    assert!(built.ok);
    assert_eq!(built.lexical_vocabulary, "wordpiece-30522");
    assert_eq!(
        built.lexical_model.as_deref(),
        Some("opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini")
    );
    assert_eq!(built.lexical_files, 4, "unmatched {}", built.lexical_unmatched);
    assert_eq!(built.lexical_unmatched, 0);
}

struct CleanDir(PathBuf);

impl Drop for CleanDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

fn assert_pooled(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}");
    let mut worst = 0.0f32;
    let mut at = 0usize;
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let diff = (g - w).abs();
        if diff > worst {
            worst = diff;
            at = i;
        }
    }
    assert!(
        worst <= POOLED_ABS,
        "{what}: max |rust-torch| = {worst} at vocab {at} (rust {}, torch {}), bound {POOLED_ABS}",
        got[at], want[at]
    );
}

fn assert_hidden(
    trace: &[Vec<f32>],
    hidden: &common::NpyF32,
    positions: &[i64],
    width: usize,
    what: &str,
) {
    let layers = hidden.shape[0];
    let npos = hidden.shape[1];
    assert_eq!(hidden.shape[2], width, "{what}");
    assert_eq!(positions.len(), npos, "{what}");
    assert_eq!(trace.len(), layers, "{what} trace layers");
    let mut worst = 0.0f32;
    let mut where_ = (0usize, 0usize, 0usize);
    for (p_index, &pos) in positions.iter().enumerate() {
        let pos = pos as usize;
        for layer in 0..layers {
            let row = &trace[layer];
            assert!(
                (pos + 1) * width <= row.len(),
                "{what}: layer {layer} has {} floats, position {pos} needs {width}",
                row.len()
            );
            let got = &row[pos * width..(pos + 1) * width];
            let base = (layer * npos + p_index) * width;
            let want = &hidden.data[base..base + width];
            for (d, (g, w)) in got.iter().zip(want).enumerate() {
                let diff = (g - w).abs();
                if diff > worst {
                    worst = diff;
                    where_ = (layer, pos, d);
                }
            }
        }
    }
    assert!(
        worst <= HIDDEN_ABS,
        "{what}: max |hidden rust-torch| = {worst} at layer {} position {} dim {}, bound {HIDDEN_ABS}",
        where_.0, where_.1, where_.2
    );
}
