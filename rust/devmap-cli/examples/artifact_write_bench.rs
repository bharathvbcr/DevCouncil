//! Interleaved A/B, min-of-N: what the consumer-artifact write costs a one-file
//! build.
//!
//! Arm `manifest` is `devmap build --manifest` (writes `repo_map.json` and
//! `code_graph.json`); arm `plain` is `devmap build` (writes neither). Both
//! arms run after the same kind of one-line edit, alternating, so host-load
//! drift lands on both. Per round the bench records wall time, the
//! `writing consumer artifacts` stage and its sub-phases from the build's own
//! timings, the artifacts' size, and whether the artifact file was replaced
//! (inode or mtime moved) -- bytes written is load-independent, wall time is
//! not.
//!
//! ```text
//! cargo build --release -p devmap-cli --bin devmap
//! cargo run --release -p devmap-cli --example artifact_write_bench -- \
//!     <scratch-repo> <edit-file-relative-to-repo> [rounds=7]
//! ```
//!
//! Point it only at a scratch clone (`git clone --shared`): it edits the named
//! file and rebuilds that clone's store. The `devmap` binary is the one next
//! to this example's profile directory, or `DEVMAP_BIN`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn devmap_bin() -> PathBuf {
    if let Some(explicit) = std::env::var_os("DEVMAP_BIN") {
        return PathBuf::from(explicit);
    }
    let exe = std::env::current_exe().expect("current exe");
    exe.parent()
        .and_then(Path::parent)
        .expect("profile directory")
        .join("devmap")
}

fn stamp_of(path: &Path) -> Option<(u64, u64, std::time::SystemTime)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.ino(), meta.len(), meta.modified().ok()?))
}

struct Round {
    wall_ms: f64,
    artifact_stage_ms: Option<f64>,
    phases: Vec<(String, f64)>,
    map_bytes: u64,
    graph_bytes: u64,
    replaced: bool,
}

/// `repo_map.json` and `code_graph.json` where `devmap paths` says this
/// repository keeps them.
fn artifact_paths(bin: &Path, repo: &Path) -> [PathBuf; 2] {
    let output = Command::new(bin)
        .args(["paths", "--json"])
        .current_dir(repo)
        .output()
        .expect("devmap paths runs");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("devmap paths prints JSON");
    ["repo_map", "code_graph"].map(|key| {
        PathBuf::from(
            json[key]
                .as_str()
                .unwrap_or_else(|| panic!("paths has no {key}")),
        )
    })
}

/// A comment line the file's language accepts, so the edit changes the
/// extraction (a line the parser ignores would leave the build "up to date").
fn comment(edit: &Path, text: &str) -> String {
    match edit.extension().and_then(|e| e.to_str()) {
        Some("rs" | "ts" | "tsx" | "js" | "go" | "java" | "swift" | "kt" | "c" | "cpp") => {
            format!("// {text}")
        }
        _ => format!("# {text}"),
    }
}

fn run(bin: &Path, repo: &Path, manifest: bool) -> Round {
    let state = artifact_paths(bin, repo).map(Some);
    let before = state.clone().map(|path| path.as_deref().and_then(stamp_of));
    let mut command = Command::new(bin);
    command.args(["--json", "build", "."]).current_dir(repo);
    if manifest {
        command.arg("--manifest");
    }
    let started = Instant::now();
    let output = command.output().expect("devmap build runs");
    let wall_ms = started.elapsed().as_secs_f64() * 1e3;
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(text.lines().next_back().expect("build prints a JSON line"))
            .expect("build JSON");
    let stage = json["timings"]["stages"].as_array().and_then(|stages| {
        stages
            .iter()
            .find(|stage| stage["stage"] == "writing consumer artifacts")
    });
    let ms = |value: &serde_json::Value| value["seconds"].as_f64().map(|secs| secs * 1e3);
    let phases = stage
        .and_then(|stage| stage["sub"].as_array())
        .map(|subs| {
            subs.iter()
                .filter_map(|sub| Some((sub["stage"].as_str()?.to_string(), ms(sub)?)))
                .collect()
        })
        .unwrap_or_default();
    let after = state.map(|path| path.as_deref().and_then(stamp_of));
    Round {
        wall_ms,
        artifact_stage_ms: stage.and_then(ms),
        phases,
        map_bytes: after[0].map_or(0, |s| s.1),
        graph_bytes: after[1].map_or(0, |s| s.1),
        replaced: before != after,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let repo = PathBuf::from(
        args.next()
            .expect("usage: <scratch-repo> <edit-file> [rounds]"),
    );
    let edit = repo.join(
        args.next()
            .expect("usage: <scratch-repo> <edit-file> [rounds]"),
    );
    let rounds: usize = args.next().map_or(7, |n| n.parse().expect("rounds"));
    let bin = devmap_bin();

    // One build outside the measurement so both arms see a warm store.
    run(&bin, &repo, true);

    let mut manifest_rounds = Vec::new();
    let mut plain_rounds = Vec::new();
    for round in 0..rounds {
        // Alternate which arm goes first so neither always follows the other.
        let order = if round % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        };
        for manifest in order {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&edit)
                .expect("edit file opens");
            writeln!(
                file,
                "{}",
                comment(
                    &edit,
                    &format!("artifact_write_bench edit {round} {manifest}")
                )
            )
            .expect("edit");
            drop(file);
            let result = run(&bin, &repo, manifest);
            if manifest {
                manifest_rounds.push(result);
            } else {
                plain_rounds.push(result);
            }
        }
    }

    for (label, set) in [("manifest", &manifest_rounds), ("plain", &plain_rounds)] {
        let walls: Vec<f64> = set.iter().map(|r| r.wall_ms).collect();
        let min = walls.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = walls.iter().cloned().fold(0.0, f64::max);
        let mut sorted = walls.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        println!(
            "{label:9} n={} wall_ms min={min:.0} median={:.0} max={max:.0} (ambient load, not a quiet-host result)",
            set.len(),
            sorted[sorted.len() / 2]
        );
        println!(
            "          artifact files replaced in {}/{} rounds; repo_map {} B, code_graph {} B",
            set.iter().filter(|r| r.replaced).count(),
            set.len(),
            set.last().map_or(0, |r| r.map_bytes),
            set.last().map_or(0, |r| r.graph_bytes),
        );
        if let Some(stage_min) = set
            .iter()
            .filter_map(|r| r.artifact_stage_ms)
            .reduce(f64::min)
        {
            println!("          writing consumer artifacts stage: min {stage_min:.0} ms");
        }
        let mut names: Vec<&str> = set
            .iter()
            .flat_map(|r| r.phases.iter().map(|(n, _)| n.as_str()))
            .collect();
        names.dedup();
        names.sort();
        names.dedup();
        for name in names {
            let samples: Vec<f64> = set
                .iter()
                .filter_map(|r| r.phases.iter().find(|(n, _)| n == name).map(|(_, v)| *v))
                .collect();
            let lo = samples.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = samples.iter().cloned().fold(0.0, f64::max);
            println!(
                "            {name:26} min {lo:.0} max {hi:.0} ms (n={})",
                samples.len()
            );
        }
    }
}
