//! The dense embedder against the sentence-transformers reference.
//!
//! `tests/fixtures/minilm/fixture.json` holds, for six texts, the token ids the
//! reference tokenizer produced and its L2-normalised embedding (recorded by
//! `record.py` from the local Hugging Face cache; nothing is downloaded and
//! the test needs no Python). Token ids must match exactly; embeddings must
//! agree to cosine >= 0.999.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::path::PathBuf;

use dc_sparse_encode::dense::{DenseModel, cosine};
use serde_json::Value;

const MIN_COSINE: f32 = 0.999;

#[test]
fn the_tessl_dense_embedding_matches_sentence_transformers() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minilm/fixture.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let work = std::env::temp_dir().join(format!("dense-parity-{}", std::process::id()));
    let model = DenseModel::open(&work).unwrap_or_else(|err| panic!("{err}"));
    let rows = fixture["rows"].as_array().unwrap();
    assert!(rows.len() >= 6);
    for row in rows {
        let text = row["text"].as_str().unwrap();
        let label: String = text.chars().take(40).collect();
        let want_ids: Vec<u32> = row["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        // [CLS] = 101 and [SEP] = 102 in this vocabulary.
        assert_eq!(want_ids.first(), Some(&101));
        assert_eq!(want_ids.last(), Some(&102));
        assert_eq!(
            model.query_ids(text),
            want_ids[1..want_ids.len() - 1],
            "token ids for {label:?}"
        );

        let want: Vec<f32> = row["embedding"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        let got = model.embed_text(text).unwrap();
        assert_eq!(got.len(), want.len());
        let cos = cosine(&got, &want);
        eprintln!("cosine {cos:.7} for {label:?}");
        assert!(cos >= MIN_COSINE, "cosine {cos} for {label:?}");
        let norm = cosine(&got, &got);
        assert!((norm - 1.0).abs() < 1e-4, "not unit length: {norm}");
    }
    std::fs::remove_dir_all(&work).ok();
}

#[test]
fn a_different_text_is_not_the_same_embedding() {
    // Guards the parity test against a degenerate model: every text mapping
    // to one vector would pass a cosine check against a near-constant output.
    let work = std::env::temp_dir().join(format!("dense-distinct-{}", std::process::id()));
    let model = DenseModel::open(&work).unwrap_or_else(|err| panic!("{err}"));
    let a = model
        .embed_text("a lease keeps two agents off one task")
        .unwrap();
    let b = model
        .embed_text("the weather in spring is mild and wet")
        .unwrap();
    assert!(
        cosine(&a, &b) < 0.9,
        "unrelated texts embedded almost identically"
    );
    std::fs::remove_dir_all(&work).ok();
}
