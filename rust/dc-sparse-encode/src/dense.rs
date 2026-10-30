//! A dense sentence embedder on the same tessl BERT forward the sparse
//! encoder uses. A measurement tool for `docs/devmap/design/retrieval-eval.md`,
//! not part of the encoder's output and not wired into `dcgrep`.
//!
//! `BertSparseModel` loads a masked-LM checkpoint: `bert.*` tensor names and a
//! `cls.predictions` head. `sentence-transformers/all-MiniLM-L6-v2` is a bare
//! `BertModel` (unprefixed names, a pooler, no LM head). Rather than change
//! tessl, [`adapt_checkpoint`] writes a scratch safetensors file whose tensors
//! are the model's own, renamed, plus a synthetic head (identity transform,
//! zero decoder bias). The forward's trace is captured before the head, so
//! the head never touches the embedding; the trace's last entry is the
//! encoder's `last_hidden_state`, which [`mean_pool_l2`] pools the way the
//! model's `1_Pooling` and `2_Normalize` modules do (mean over every token,
//! `[CLS]` and `[SEP]` included, then L2).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

pub const DENSE_MODEL: &str = "sentence-transformers/all-MiniLM-L6-v2";
/// Content tokens per chunk: the model's 256-position `max_seq_length` less
/// `[CLS]` and `[SEP]`.
pub const WINDOW: usize = 254;
/// Chunk start-to-start distance. `WINDOW - STRIDE` = 62 tokens of overlap.
pub const STRIDE: usize = 192;

/// Chunk boundaries `(start, end)` over `len` content tokens. A tail that
/// lies wholly inside the previous chunk's overlap is not a chunk of its own.
pub fn windows(len: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < len {
        if start > 0 && len - start <= WINDOW - STRIDE {
            break;
        }
        out.push((start, (start + WINDOW).min(len)));
        start += STRIDE;
    }
    out
}

/// Mean over `tokens` rows of `[tokens, dim]`, then L2 normalised.
pub fn mean_pool_l2(hidden: &[f32], tokens: usize, dim: usize) -> Result<Vec<f32>, String> {
    if tokens == 0 || hidden.len() != tokens * dim {
        return Err(format!(
            "hidden state has {} floats, expected {tokens} x {dim}",
            hidden.len()
        ));
    }
    let mut mean = vec![0f32; dim];
    for row in hidden.chunks_exact(dim) {
        for (m, v) in mean.iter_mut().zip(row) {
            *m += v;
        }
    }
    for m in &mut mean {
        *m /= tokens as f32;
    }
    let norm = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !(norm.is_finite() && norm > 0.0) {
        return Err("pooled embedding has no finite norm".into());
    }
    for m in &mut mean {
        *m /= norm;
    }
    Ok(mean)
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Write `bert.*`-named safetensors from a bare-BertModel checkpoint, with a
/// synthetic LM head. Only F32 tensors are accepted; `pooler.*` and
/// `position_ids` are dropped (they are not read by the forward).
pub fn adapt_checkpoint(src: &Path, dst: &Path, hidden: usize, vocab: usize) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let n = bytes
        .get(..8)
        .ok_or("safetensors: truncated length")
        .map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")) as usize)?;
    let header: Value =
        serde_json::from_slice(bytes.get(8..8 + n).ok_or("safetensors: truncated header")?)
            .map_err(|e| format!("safetensors header: {e}"))?;
    let data = &bytes[8 + n..];
    let mut tensors: BTreeMap<String, (Vec<usize>, Vec<u8>)> = BTreeMap::new();
    for (name, info) in header
        .as_object()
        .ok_or("safetensors header is not an object")?
    {
        if name == "__metadata__"
            || name == "embeddings.position_ids"
            || name.starts_with("pooler.")
        {
            continue;
        }
        if info["dtype"] != json!("F32") {
            return Err(format!("{name}: dtype {} is not F32", info["dtype"]));
        }
        let off = info["data_offsets"].as_array().ok_or("no data_offsets")?;
        let (a, b) = (
            off[0].as_u64().ok_or("bad offset")? as usize,
            off[1].as_u64().ok_or("bad offset")? as usize,
        );
        let shape: Vec<usize> = info["shape"]
            .as_array()
            .ok_or("no shape")?
            .iter()
            .map(|v| v.as_u64().map(|x| x as usize).ok_or("bad shape"))
            .collect::<Result<_, _>>()?;
        let raw = data
            .get(a..b)
            .ok_or_else(|| format!("{name}: data out of range"))?;
        if raw.len() != 4 * shape.iter().product::<usize>() {
            return Err(format!("{name}: byte length does not match its shape"));
        }
        tensors.insert(format!("bert.{name}"), (shape, raw.to_vec()));
    }
    let f32_bytes = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut identity = vec![0f32; hidden * hidden];
    for i in 0..hidden {
        identity[i * hidden + i] = 1.0;
    }
    let head: [(&str, Vec<usize>, Vec<f32>); 5] = [
        (
            "cls.predictions.transform.dense.weight",
            vec![hidden, hidden],
            identity,
        ),
        (
            "cls.predictions.transform.dense.bias",
            vec![hidden],
            vec![0.0; hidden],
        ),
        (
            "cls.predictions.transform.LayerNorm.weight",
            vec![hidden],
            vec![1.0; hidden],
        ),
        (
            "cls.predictions.transform.LayerNorm.bias",
            vec![hidden],
            vec![0.0; hidden],
        ),
        ("cls.predictions.bias", vec![vocab], vec![0.0; vocab]),
    ];
    for (name, shape, values) in head {
        tensors.insert(name.to_string(), (shape, f32_bytes(&values)));
    }
    let mut entries = serde_json::Map::new();
    let mut offset = 0usize;
    for (name, (shape, raw)) in &tensors {
        entries.insert(
            name.clone(),
            json!({"dtype": "F32", "shape": shape, "data_offsets": [offset, offset + raw.len()]}),
        );
        offset += raw.len();
    }
    let mut head_json = serde_json::to_vec(&Value::Object(entries)).map_err(|e| e.to_string())?;
    while !head_json.len().is_multiple_of(8) {
        head_json.push(b' ');
    }
    let mut out = Vec::with_capacity(8 + head_json.len() + offset);
    out.extend_from_slice(&(head_json.len() as u64).to_le_bytes());
    out.extend_from_slice(&head_json);
    for (_, raw) in tensors.values() {
        out.extend_from_slice(raw);
    }
    std::fs::write(dst, out).map_err(|e| format!("{}: {e}", dst.display()))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use gpu::DenseModel;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod gpu {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    use tessl::GpuRuntime;
    use tessl::bert::{BertConfig, BertSparseModel};
    use tessl::safetensors::SafeTensors;

    use super::{DENSE_MODEL, WINDOW, adapt_checkpoint, mean_pool_l2};
    use crate::{lookup, read_vocab, snapshot};

    pub struct DenseModel {
        vocab: HashMap<String, u32>,
        cls: u32,
        sep: u32,
        pub dim: usize,
        _runtime: Arc<GpuRuntime>,
        model: BertSparseModel,
    }

    impl DenseModel {
        /// Load from the local Hugging Face cache; never downloads. `work` is a
        /// scratch directory that receives the adapted checkpoint.
        pub fn open(work: &Path) -> Result<Self, String> {
            let dir = snapshot(DENSE_MODEL)?;
            let cfg = BertConfig::from_config_file(&dir.join("config.json"))?;
            std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
            let adapted = work.join("all-MiniLM-L6-v2.bert-head.safetensors");
            adapt_checkpoint(
                &dir.join("model.safetensors"),
                &adapted,
                cfg.hidden as usize,
                cfg.vocab as usize,
            )?;
            let st = SafeTensors::open(&adapted)?;
            let runtime = GpuRuntime::new_inference()?;
            let dim = cfg.hidden as usize;
            let model = BertSparseModel::load(&runtime, &st, cfg)?;
            let tokens = read_vocab(&dir.join("vocab.txt"))?;
            let vocab = lookup(&tokens);
            let cls = *vocab.get("[CLS]").ok_or("vocabulary has no [CLS]")?;
            let sep = *vocab.get("[SEP]").ok_or("vocabulary has no [SEP]")?;
            Ok(Self {
                vocab,
                cls,
                sep,
                dim,
                _runtime: runtime,
                model,
            })
        }

        /// Content token ids for `text` (no `[CLS]`/`[SEP]`, not truncated).
        pub fn content_ids(&self, text: &str, cap: usize) -> Vec<u32> {
            dc_grep::wordpiece_ids(text, &self.vocab, cap)
        }

        /// Query ids, truncated like the reference (`max_seq_length` 256).
        pub fn query_ids(&self, text: &str) -> Vec<u32> {
            self.content_ids(text, WINDOW)
        }

        /// Embedding of one already-tokenised chunk of content ids.
        pub fn embed_ids(&self, content: &[u32]) -> Result<Vec<f32>, String> {
            if content.is_empty() || content.len() > WINDOW {
                return Err(format!(
                    "chunk of {} tokens is outside 1..={WINDOW}",
                    content.len()
                ));
            }
            let mut ids = Vec::with_capacity(content.len() + 2);
            ids.push(self.cls);
            ids.extend_from_slice(content);
            ids.push(self.sep);
            let out = self.model.encode(&[ids.as_slice()], true)?;
            let last = out.trace.last().ok_or("forward returned no trace")?;
            mean_pool_l2(last, ids.len(), self.dim)
        }

        /// Embedding of `text`, truncated to the first window.
        pub fn embed_text(&self, text: &str) -> Result<Vec<f32>, String> {
            self.embed_ids(&self.query_ids(text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_cover_every_token_with_the_stated_overlap() {
        assert_eq!(windows(0), vec![]);
        assert_eq!(windows(1), vec![(0, 1)]);
        assert_eq!(windows(254), vec![(0, 254)]);
        // 255 tokens: token 254 is past the first window, so a second window
        // starts at 192; with 254 tokens it would lie wholly inside the first.
        assert_eq!(windows(255), vec![(0, 254), (192, 255)]);
        assert_eq!(windows(300), vec![(0, 254), (192, 300)]);
        for len in [1usize, 100, 253, 254, 255, 300, 446, 447, 1000, 5000] {
            let w = windows(len);
            let mut covered = vec![false; len];
            for (a, b) in &w {
                assert!(b - a <= WINDOW && a < b && *b <= len);
                for c in &mut covered[*a..*b] {
                    *c = true;
                }
            }
            assert!(
                covered.iter().all(|c| *c),
                "len {len} left a token uncovered"
            );
        }
    }

    #[test]
    fn pooling_is_mean_then_l2() {
        let v = mean_pool_l2(&[1.0, 0.0, 3.0, 0.0], 2, 2).unwrap();
        assert!((v[0] - 1.0).abs() < 1e-6 && v[1] == 0.0);
        let u = mean_pool_l2(&[3.0, 4.0], 1, 2).unwrap();
        assert!((cosine(&u, &u) - 1.0).abs() < 1e-6);
        assert!(mean_pool_l2(&[0.0, 0.0], 1, 2).is_err());
        assert!(mean_pool_l2(&[1.0, 2.0, 3.0], 2, 2).is_err());
    }

    #[test]
    fn the_adapted_checkpoint_carries_prefixed_tensors_and_a_head() {
        let dir = std::env::temp_dir().join(format!("dense-adapt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.safetensors");
        let hdr = br#"{"embeddings.word_embeddings.weight":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]},"pooler.dense.bias":{"dtype":"F32","shape":[1],"data_offsets":[16,20]},"embeddings.position_ids":{"dtype":"I64","shape":[1],"data_offsets":[20,28]}}"#;
        let mut padded = hdr.to_vec();
        while !padded.len().is_multiple_of(8) {
            padded.push(b' ');
        }
        let mut file = (padded.len() as u64).to_le_bytes().to_vec();
        file.extend(&padded);
        file.extend((0..7u32).flat_map(|i| (i as f32).to_le_bytes()));
        file.extend([0u8; 8]);
        std::fs::write(&src, file).unwrap();
        let dst = dir.join("dst.safetensors");
        adapt_checkpoint(&src, &dst, 2, 2).unwrap();
        let out = std::fs::read(&dst).unwrap();
        let n = u64::from_le_bytes(out[..8].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&out[8..8 + n]).unwrap();
        let names: Vec<&str> = header
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(names.contains(&"bert.embeddings.word_embeddings.weight"));
        assert!(names.contains(&"cls.predictions.bias"));
        assert!(
            !names
                .iter()
                .any(|n| n.contains("pooler") || n.contains("position_ids"))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_f32_tensor_is_refused() {
        let dir = std::env::temp_dir().join(format!("dense-adapt-f16-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.safetensors");
        let hdr = br#"{"encoder.x":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}      "#;
        let mut file = (hdr.len() as u64).to_le_bytes().to_vec();
        file.extend(hdr.iter());
        file.extend([0u8; 2]);
        std::fs::write(&src, file).unwrap();
        let err = adapt_checkpoint(&src, &dir.join("d"), 2, 2).unwrap_err();
        assert!(err.contains("not F32"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
