//! Interleaved A/B against the torch oracle, min of N.
//!
//! A single before/after measurement is not a comparison: the machine warms
//! up, and whichever side ran second looks faster. Each repeat runs both,
//! and which one goes first alternates. The reported number is the minimum
//! of that side's samples, not the mean and not the first one.
//!
//! Measured 2026-10-08, release, four fixture documents, N=5, doc-v2-mini:
//! rust minimum 0.041532s, torch (CPU) minimum 0.273876s.
//!
//! The torch process is started once and kept. Each side times tokenisation,
//! the forward and the pool, and neither times model load. Torch stays on
//! CPU, which is the script's default device; both sides on the GPU would
//! be timing each other's contention.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Instant;

use crate::tokenize::document_ids;

/// Fewer than this and the minimum is one sample with a warm-up mixed in.
pub const MIN_REPEATS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Rust,
    Torch,
}

/// Pair `i` starts with Rust when `i` is even and with torch when `i` is odd.
pub fn schedule(repeats: usize) -> Vec<[Side; 2]> {
    (0..repeats)
        .map(|i| {
            if i % 2 == 0 {
                [Side::Rust, Side::Torch]
            } else {
                [Side::Torch, Side::Rust]
            }
        })
        .collect()
}

pub fn min_of(samples: &[f64]) -> Option<f64> {
    samples
        .iter()
        .copied()
        .filter(|sample| sample.is_finite())
        .reduce(f64::min)
}

pub struct Report {
    pub repeats: usize,
    pub documents: usize,
    pub rust_seconds: Vec<f64>,
    pub torch_seconds: Vec<f64>,
    pub rust_min: f64,
    pub torch_min: f64,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn run(model_id: &str, files: &[std::path::PathBuf], repeats: usize) -> Result<Report, String> {
    if repeats < MIN_REPEATS {
        return Err(format!(
            "--repeats must be at least {MIN_REPEATS} (got {repeats}); a shorter run is one sample, not a min-of-N"
        ));
    }
    if files.is_empty() {
        return Err("bench: no documents".into());
    }
    let mut texts = Vec::with_capacity(files.len());
    for path in files {
        let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{} is not utf-8", path.display()))?;
        texts.push(text);
    }

    let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/oracle_worker.py");
    if !worker.is_file() {
        return Err(format!("torch oracle is missing at {}", worker.display()));
    }
    let mut child = Command::new("python3")
        .arg(&worker)
        .arg(model_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| format!("could not start the torch oracle ({err})"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("torch oracle stdin was not piped")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("torch oracle stdout was not piped")?;
    let mut reader = BufReader::new(stdout);
    let mut ready = String::new();
    reader
        .read_line(&mut ready)
        .map_err(|err| format!("torch oracle closed before READY ({err})"))?;
    if ready.trim() != "READY" {
        let _ = child.kill();
        return Err(format!("torch oracle said {ready:?}, want READY"));
    }

    let loaded = crate::model::Loaded::open(model_id)?;

    let mut rust_seconds = Vec::with_capacity(repeats);
    let mut torch_seconds = Vec::with_capacity(repeats);
    for pair in schedule(repeats) {
        for side in pair {
            match side {
                Side::Rust => rust_seconds.push(time_rust(&loaded, &texts)?),
                Side::Torch => torch_seconds.push(time_torch(&mut stdin, &mut reader, &mut child, &texts)?),
            }
        }
    }
    let _ = stdin.write_all(b"QUIT\n");
    let _ = child.wait();

    let rust_min = min_of(&rust_seconds).ok_or("no rust samples")?;
    let torch_min = min_of(&torch_seconds).ok_or("no torch samples")?;
    Ok(Report {
        repeats,
        documents: texts.len(),
        rust_seconds,
        torch_seconds,
        rust_min,
        torch_min,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn time_rust(loaded: &crate::model::Loaded, texts: &[String]) -> Result<f64, String> {
    let started = Instant::now();
    let mut ids = Vec::with_capacity(texts.len());
    for text in texts {
        ids.push(document_ids(text, &loaded.vocab, loaded.max_positions)?);
    }
    let _ = loaded.forward(&ids, false)?;
    Ok(started.elapsed().as_secs_f64())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn time_torch(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<std::process::ChildStdout>,
    child: &mut Child,
    texts: &[String],
) -> Result<f64, String> {
    let blob = texts.join("\0");
    let bytes = blob.as_bytes();
    writeln!(stdin, "ENCODE {}", bytes.len()).map_err(|err| format!("torch oracle stdin: {err}"))?;
    stdin
        .write_all(bytes)
        .map_err(|err| format!("torch oracle stdin: {err}"))?;
    stdin.flush().map_err(|err| format!("torch oracle stdin: {err}"))?;
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|err| format!("torch oracle stdout: {err}"))?;
    if read == 0 {
        let _ = child.kill();
        return Err("torch oracle exited during a measurement".into());
    }
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("OK"), Some(seconds)) => seconds
            .parse::<f64>()
            .map_err(|_| format!("torch oracle timing {line:?} is not a number")),
        _ => {
            let _ = child.kill();
            Err(format!("torch oracle said {line:?}, want OK <seconds>"))
        }
    }
}

pub fn format_report(model_id: &str, report: &Report) -> String {
    format!(
        "bench: {model_id}, {} documents, N={}, torch on CPU, tessl on Metal\n\
         rust  min {:.6}s  samples {:?}\n\
         torch min {:.6}s  samples {:?}\n\
         min-of-N ratio (torch / rust) {:.3}",
        report.documents,
        report.repeats,
        report.rust_min,
        report.rust_seconds,
        report.torch_min,
        report.torch_seconds,
        report.torch_min / report.rust_min
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schedule_alternates_who_goes_first() {
        let pairs = schedule(MIN_REPEATS);
        assert_eq!(pairs.len(), MIN_REPEATS);
        assert_eq!(pairs[0], [Side::Rust, Side::Torch]);
        assert_eq!(pairs[1], [Side::Torch, Side::Rust]);
        let rust = pairs
            .iter()
            .filter(|pair| pair[0] == Side::Rust || pair[1] == Side::Rust)
            .count();
        let torch = pairs
            .iter()
            .filter(|pair| pair[0] == Side::Torch || pair[1] == Side::Torch)
            .count();
        assert_eq!(rust, MIN_REPEATS);
        assert_eq!(torch, MIN_REPEATS);
    }

    #[test]
    fn min_of_n_is_the_minimum() {
        assert_eq!(min_of(&[0.4, 0.2, 0.9, 0.3, 0.5]), Some(0.2));
        assert_eq!(min_of(&[]), None);
    }
}
