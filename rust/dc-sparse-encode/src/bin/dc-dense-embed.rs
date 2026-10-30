//! `dc-dense-embed`: dense file ranking for `docs/devmap/design/retrieval-eval.md`.
//!
//!     dc-dense-embed index --root DIR --corpus corpus.txt --work DIR --out DIR
//!     dc-dense-embed query --index DIR --queries q.jsonl --work DIR --out run.jsonl [--repeats N]
//!
//! A measurement tool. Apple silicon only (tessl's Metal kernels), reads the
//! model from the local Hugging Face cache, never downloads, and is not linked
//! into `dcgrep`.

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dc-dense-embed: {err}");
            ExitCode::from(2)
        }
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn run() -> Result<(), String> {
    Err("the dense embedder runs on tessl, which is Apple silicon only".into())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run() -> Result<(), String> {
    imp::run(std::env::args().skip(1).collect())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod imp {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use dc_sparse_encode::dense::{DenseModel, WINDOW, cosine, windows};
    use serde_json::{Value, json};

    fn flags(argv: &[String]) -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        let mut it = argv.iter();
        while let Some(a) = it.next() {
            let name = a
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument {a:?}"))?;
            let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            out.insert(name.to_string(), v.clone());
        }
        Ok(out)
    }

    fn need<'a>(f: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
        f.get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("--{name} is required"))
    }

    pub fn run(argv: Vec<String>) -> Result<(), String> {
        let (cmd, rest) = argv
            .split_first()
            .ok_or("usage: dc-dense-embed index|query ...")?;
        let f = flags(rest)?;
        match cmd.as_str() {
            "index" => index(&f),
            "query" => query(&f),
            other => Err(format!("unknown command {other:?}")),
        }
    }

    fn index(f: &BTreeMap<String, String>) -> Result<(), String> {
        let root = PathBuf::from(need(f, "root")?);
        let out = PathBuf::from(need(f, "out")?);
        let paths: Vec<String> = std::fs::read_to_string(need(f, "corpus")?)
            .map_err(|e| e.to_string())?
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        let limit: usize = f
            .get("max-files")
            .map_or(Ok(usize::MAX), |v| v.parse().map_err(|_| "--max-files"))?;
        let model = DenseModel::open(Path::new(need(f, "work")?))?;
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;

        let started = Instant::now();
        let mut vectors: Vec<u8> = Vec::new();
        let mut table = String::new();
        let (mut chunks, mut tokens, mut skipped) = (0usize, 0usize, 0usize);
        for (n, path) in paths.iter().take(limit).enumerate() {
            let text = match std::fs::read(root.join(path))
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
            {
                Some(t) => t,
                None => {
                    skipped += 1;
                    table.push_str(&format!("{path}\t{chunks}\t0\n"));
                    continue;
                }
            };
            let ids = model.content_ids(&text, usize::MAX);
            tokens += ids.len();
            let first = chunks;
            for (a, b) in windows(ids.len()) {
                for v in model.embed_ids(&ids[a..b])? {
                    vectors.extend_from_slice(&v.to_le_bytes());
                }
                chunks += 1;
            }
            table.push_str(&format!("{path}\t{first}\t{}\n", chunks - first));
            if (n + 1) % 250 == 0 {
                eprintln!(
                    "  {}/{} files, {chunks} chunks, {:.0}s",
                    n + 1,
                    paths.len(),
                    started.elapsed().as_secs_f64()
                );
            }
        }
        std::fs::write(out.join("vectors.f32"), &vectors).map_err(|e| e.to_string())?;
        std::fs::write(out.join("files.tsv"), table).map_err(|e| e.to_string())?;
        let stats = json!({
            "model": dc_sparse_encode::dense::DENSE_MODEL, "window": WINDOW,
            "stride": dc_sparse_encode::dense::STRIDE, "dim": model.dim,
            "files": paths.len().min(limit), "files_not_utf8_or_unreadable": skipped,
            "chunks": chunks, "content_tokens": tokens, "vector_bytes": vectors.len(),
            "encode_seconds": started.elapsed().as_secs_f64(),
        });
        std::fs::write(
            out.join("stats.json"),
            serde_json::to_string_pretty(&stats).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        eprintln!("{stats}");
        Ok(())
    }

    fn query(f: &BTreeMap<String, String>) -> Result<(), String> {
        let dir = PathBuf::from(need(f, "index")?);
        let repeats: usize = f
            .get("repeats")
            .map_or(Ok(7), |v| v.parse().map_err(|_| "--repeats"))?;
        if repeats == 0 {
            return Err("--repeats must be at least 1".into());
        }
        let model = DenseModel::open(Path::new(need(f, "work")?))?;
        let raw = std::fs::read(dir.join("vectors.f32")).map_err(|e| e.to_string())?;
        let dim = model.dim;
        if raw.len() % (4 * dim) != 0 {
            return Err("vectors.f32 is not a whole number of vectors".into());
        }
        let vectors: Vec<f32> = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let mut files: Vec<(String, usize, usize)> = Vec::new();
        for line in std::fs::read_to_string(dir.join("files.tsv"))
            .map_err(|e| e.to_string())?
            .lines()
        {
            let mut p = line.split('\t');
            let (path, first, n) = (
                p.next().ok_or("files.tsv")?,
                p.next().ok_or("files.tsv")?,
                p.next().ok_or("files.tsv")?,
            );
            files.push((
                path.to_string(),
                first.parse().map_err(|_| "files.tsv")?,
                n.parse().map_err(|_| "files.tsv")?,
            ));
        }
        let mut rows = String::new();
        let mut embed_ms: Vec<f64> = Vec::new();
        for line in std::fs::read_to_string(need(f, "queries")?)
            .map_err(|e| e.to_string())?
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            let (id, text) = (
                v["id"].as_str().ok_or("query without id")?,
                v["query"].as_str().ok_or("query without text")?,
            );
            let mut best = f64::INFINITY;
            let mut q = Vec::new();
            for _ in 0..repeats {
                let t = Instant::now();
                q = model.embed_text(text)?;
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            embed_ms.push(best);
            let mut scored: Vec<(f32, &str)> = files
                .iter()
                .filter(|(_, _, n)| *n > 0)
                .map(|(path, first, n)| {
                    let top = (*first..first + n)
                        .map(|c| cosine(&q, &vectors[c * dim..(c + 1) * dim]))
                        .fold(f32::NEG_INFINITY, f32::max);
                    (top, path.as_str())
                })
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            let ranked: Vec<&str> = scored.into_iter().map(|(_, p)| p).collect();
            rows.push_str(&json!({"id": id, "ranked": ranked}).to_string());
            rows.push('\n');
        }
        std::fs::write(need(f, "out")?, rows).map_err(|e| e.to_string())?;
        embed_ms.sort_by(f64::total_cmp);
        let at = |p: f64| embed_ms[((embed_ms.len() - 1) as f64 * p).round() as usize];
        eprintln!(
            "{}",
            json!({"queries": embed_ms.len(), "repeats_per_query": repeats,
                   "embed_ms_per_query_min_of_n": {"min": at(0.0), "median": at(0.5), "p90": at(0.9), "max": at(1.0)}})
        );
        Ok(())
    }
}
