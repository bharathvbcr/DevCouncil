//! The tessl forward. Apple silicon only: tessl's kernels are Metal.
//!
//! `encode` refuses a runtime whose relaxed precision is on. `new_inference`
//! is exact f32 and skips the counter-heap timestamps, which are a host tax
//! this offline pass does not read.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tessl::bert::{BertConfig, BertSparseModel};
use tessl::safetensors::SafeTensors;
use tessl::GpuRuntime;

use crate::post::special_drop;
use crate::vocab::{self, query_weights, read_vocab};

pub struct Loaded {
    pub model_id: String,
    pub tokens: Vec<String>,
    pub vocab: HashMap<String, u32>,
    pub weights: Vec<f64>,
    pub drop: HashSet<u32>,
    pub max_positions: usize,
    pub hidden: usize,
    pub vocab_size: usize,
    runtime: Arc<GpuRuntime>,
    model: BertSparseModel,
}

pub struct Forward {
    /// `[batch, vocab]`, pre-round `max log1p(relu(logits))`.
    pub pooled: Vec<f32>,
    /// One sequence only, when requested: embedding output, then the residual
    /// after each layer. Each entry is `[tokens, hidden]`, row-major.
    pub trace: Vec<Vec<f32>>,
}

impl Loaded {
    /// Load `model_id` from the local hub cache. A missing file is an error.
    /// This does not download.
    pub fn open(model_id: &str) -> Result<Self, String> {
        if model_id.to_ascii_lowercase().contains("splade") {
            eprintln!(
                "note: {model_id} looks like a SPLADE model. SPLADE weights are\n\
                 CC BY-NC-SA 4.0 (non-commercial). DevCouncil is Apache-2.0\n\
                 and ships no weights; whether that licence suits your use\n\
                 is your call. Continuing."
            );
        }
        let dir = vocab::snapshot(model_id)?;
        let cfg = BertConfig::from_config_file(&dir.join("config.json"))?;
        let st = SafeTensors::open(&dir.join("model.safetensors"))?;
        let runtime = GpuRuntime::new_inference()?;
        let model = BertSparseModel::load(&runtime, &st, cfg.clone())?;
        let tokens = read_vocab(&dir.join("vocab.txt"))?;
        if tokens.len() != cfg.vocab as usize {
            return Err(format!(
                "{model_id}: vocab.txt has {} tokens, config.json says {}",
                tokens.len(),
                cfg.vocab
            ));
        }
        let idf = dir.join("idf.json");
        let weights = query_weights(Some(&idf), &tokens)?;
        let drop = special_drop(&tokens)?;
        let vocab = vocab::lookup(&tokens);
        Ok(Self {
            model_id: model_id.to_string(),
            tokens,
            vocab,
            weights,
            drop,
            max_positions: cfg.max_positions as usize,
            hidden: cfg.hidden as usize,
            vocab_size: cfg.vocab as usize,
            runtime,
            model,
        })
    }

    pub fn snapshot_dir(model_id: &str) -> Result<PathBuf, String> {
        vocab::snapshot(model_id)
    }

    /// Encode already-tokenised sequences. `trace` requires exactly one.
    /// The pooled row does not depend on the other sequences beyond GEMM
    /// tiling: padded keys are masked and padded rows are not pooled.
    pub fn forward(&self, sequences: &[Vec<u32>], trace: bool) -> Result<Forward, String> {
        let refs: Vec<&[u32]> = sequences.iter().map(Vec::as_slice).collect();
        let out = self.model.encode(&refs, trace)?;
        if out.pooled.len() != sequences.len() * self.vocab_size {
            return Err(format!(
                "forward returned {} floats, expected {} x {}",
                out.pooled.len(),
                sequences.len(),
                self.vocab_size
            ));
        }
        // The runtime stays borrowed so a dropped Loaded cannot outlive a
        // buffer the caller still reads. The vectors are host copies.
        let _ = &self.runtime;
        Ok(Forward {
            pooled: out.pooled,
            trace: out.trace,
        })
    }
}
