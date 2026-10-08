//! The post-processing and the tokeniser, judged against the recorded torch
//! fixtures. No GPU: a wrong `round` or a dropped `[UNK]` fails here before
//! a forward is launched.

mod common;

use std::collections::HashSet;
use std::path::PathBuf;

use dc_sparse_encode::{document_ids, lookup, query_ids, read_vocab, special_drop, terms_of};

const MODELS: &[&str] = &["doc-v2-mini", "doc-v3-distill"];

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn vocab() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dc-grep/tests/fixtures/wordpiece-vocab.txt");
    read_vocab(&path).expect("wordpiece vocab fixture")
}

#[test]
fn recorded_ids_match_document_tokenisation_and_pooled_rows_round_to_the_jsonl() {
    let tokens = vocab();
    assert_eq!(tokens.len(), 30522);
    let vocab_map = lookup(&tokens);
    let drop = special_drop(&tokens).unwrap();
    assert!(drop.contains(&0) && drop.contains(&101) && drop.contains(&102) && drop.contains(&103));
    assert!(!drop.contains(&100), "[UNK] is a document signal and is kept");

    for model in MODELS {
        let dir = fixtures().join(model);
        let expected = std::fs::read_to_string(dir.join("expected.jsonl")).unwrap();
        let mut lines = expected.lines();
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["schema"], 1);
        assert_eq!(header["vocabulary"], "wordpiece-30522");
        let header_vocab = header["vocab"].as_array().unwrap();
        assert_eq!(header_vocab.len(), tokens.len());
        for (i, token) in tokens.iter().enumerate() {
            assert_eq!(header_vocab[i].as_str().unwrap(), token, "{model} vocab id {i}");
        }
        let parity = header["parity"].as_array().unwrap();
        assert!(!parity.is_empty());
        for sample in parity {
            let text = sample["text"].as_str().unwrap();
            let want: Vec<u32> = sample["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_u64().unwrap() as u32)
                .collect();
            let got = query_ids(text, &vocab_map);
            assert_eq!(got, want, "{model} parity {text:?}");
        }

        let mut expected_docs = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            expected_docs.push(serde_json::from_str::<serde_json::Value>(line).unwrap());
        }
        let names: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(dir.join("documents.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(expected_docs.len(), names.len(), "{model}");

        for (i, (name, document)) in names.iter().zip(&expected_docs).enumerate() {
            let text = std::fs::read_to_string(fixtures().join("corpus/src").join(name)).unwrap();
            let ids = document_ids(&text, &vocab_map, 512).unwrap();
            let recorded = common::i64(&dir.join(format!("{name}.ids.npy")));
            let want: Vec<u32> = recorded.data.iter().map(|id| *id as u32).collect();
            assert_eq!(ids, want, "{model} {name} tokenisation");
            assert_eq!(
                document["total_terms"].as_u64().unwrap(),
                ids.len() as u64,
                "{model} {name} total_terms"
            );
            assert_eq!(document["path"], format!("src/{name}"));

            // expected.jsonl was written from the padded batch, not the
            // single-document forward. Those two agree to ~1e-6, which is
            // inside the parity bound and still enough to flip a 4th decimal
            // that lands on the rounding boundary (short.txt id 3642).
            let batch = common::f32(&dir.join("batch.pooled.npy"));
            assert_eq!(batch.shape, vec![names.len(), 30522]);
            let width = 30522;
            let row = &batch.data[i * width..(i + 1) * width];
            let got = terms_of(row, &drop);
            let want_terms = document["terms"].as_array().unwrap();
            assert_eq!(got.len(), want_terms.len(), "{model} {name} term count");
            for (got, want) in got.iter().zip(want_terms) {
                let pair = want.as_array().unwrap();
                let id = pair[0].as_u64().unwrap() as u32;
                let weight = pair[1].as_f64().unwrap();
                assert_eq!(got.0, id, "{model} {name}");
                assert!(
                    (got.1 - weight).abs() < 1e-9,
                    "{model} {name} id {id}: round gave {}, json has {weight}",
                    got.1
                );
            }
            let _ = HashSet::<u32>::new();
        }
    }
}
