//! Real linked-worktree, watcher and IPC stress. Uses only disposable local data.
//!
//! ```text
//! cargo run -p devmap-cli --example worktree_stress -- \
//!   --binary /absolute/devmap --worktrees 128 --output /tmp/capacity.json
//! ```
//!
//! Results name actual concurrency, edits, query counts and cold-build
//! comparisons. A deadline, subprocess failure, missing result or truncated
//! response fails the run.

mod support;

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use support::{
    classify_startup, finalize_pass_flag, graph_proves_equivalence, require_ipc_probe,
    retry_file_operation, ProbeAdmission, StartBarrier, StartupDecision,
};

#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CTRL_BREAK_EVENT: u32 = 1;

#[derive(clap::Parser)]
#[command(about = "Real linked-worktree, watcher and IPC stress. Uses only disposable local data.")]
struct Args {
    #[arg(long)]
    binary: PathBuf,
    /// Native ipc_probe example; required on Windows.
    #[arg(long)]
    ipc_probe: Option<PathBuf>,
    #[arg(long, default_value_t = 128)]
    worktrees: usize,
    #[arg(long, default_value_t = 4)]
    rounds: usize,
    #[arg(long, default_value_t = 16)]
    build_workers: usize,
    /// Maximum native IPC measurement processes (1..32); editors remain independent.
    #[arg(long, default_value_t = 8)]
    probe_workers: usize,
    /// Additional indexed modules per worktree (0..8192).
    #[arg(long, default_value_t = 0)]
    fixture_files: usize,
    /// Functions per additional module (2..64).
    #[arg(long, default_value_t = 8)]
    functions_per_file: usize,
    #[arg(long)]
    output: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = <Args as clap::Parser>::parse();
    if !(1..=256).contains(&args.worktrees)
        || !(1..=100).contains(&args.rounds)
        || !(1..=32).contains(&args.build_workers)
    {
        bail!("worktrees must be 1..256, rounds 1..100, build-workers 1..32");
    }
    if !(0..=8192).contains(&args.fixture_files) || !(2..=64).contains(&args.functions_per_file) {
        bail!("fixture-files must be 0..8192, functions-per-file 2..64");
    }
    if !(1..=32).contains(&args.probe_workers) {
        bail!("probe-workers must be 1..32");
    }
    require_ipc_probe(cfg!(windows), args.ipc_probe.as_deref()).map_err(anyhow::Error::from)?;

    let probe = match &args.ipc_probe {
        Some(path) => Some(path.canonicalize().context("ipc-probe")?),
        None => None,
    };
    let probe_admission =
        ProbeAdmission::new(args.probe_workers, 30.0).map_err(anyhow::Error::msg)?;
    let binary = args.binary.canonicalize().context("binary")?;
    let binary_hash = sha256_file(&binary)?;
    let mut env: HashMap<OsString, OsString> = std::env::vars_os().collect();
    env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    env.insert("GIT_CONFIG_GLOBAL".into(), os_devnull().into());
    env.insert("DEVMAP_AUTOSPAWN".into(), "0".into());
    env.insert("DEVMAP_MAX_IDLE_SECS".into(), "0".into());
    env.insert("TOKIO_WORKER_THREADS".into(), "2".into());
    env.insert("RAYON_NUM_THREADS".into(), "2".into());
    env.remove(OsStr::new("DEVMAP_HOME"));

    let started = Instant::now();
    let mut report = json!({
        "worktrees": args.worktrees,
        "simultaneous_daemons": args.worktrees,
        "simultaneous_editors": 2 * args.worktrees,
        "build_workers": args.build_workers,
        "tokio_workers_per_process": 2,
        "rayon_workers_per_process": 2,
        "binary_sha256": binary_hash,
        "platform": platform_string(),
        "ipc_transport": if cfg!(windows) { "named_pipe" } else { "unix_socket" },
        "ipc_probe_process": probe.is_some(),
        "latency_includes_probe_startup": probe.is_some(),
        "latency_includes_probe_admission": probe.is_some(),
        "ipc_probe_workers": if probe.is_some() { args.probe_workers } else { 0 },
        "ipc_probe_admission_timeout_seconds": if probe.is_some() { 30 } else { 0 },
        "initial_source_files_per_worktree": 1 + args.fixture_files,
        "initial_symbols_per_worktree": 3 + args.fixture_files * (1 + args.functions_per_file),
        "initial_functions_per_worktree": 2 + args.fixture_files * args.functions_per_file,
        "initial_call_edges_per_worktree": 1 + args.fixture_files * (args.functions_per_file - 1),
        "fixture_functions_per_file": args.functions_per_file,
        "rounds": args.rounds,
        "edit_operations": 0,
        "ipc_queries": 0,
        "metadata_checks": 0,
        "cold_comparisons": 0,
        "kills": 0,
        "daemon_rss_kib_samples": [],
        "passed": false,
        "filesystem_retry_attempts": 0,
        "filesystem_retry_deadline_seconds": 5,
    });

    let scratch_parent = scratch_parent();
    fs::create_dir_all(&scratch_parent).ok();
    let scratch = scratch_parent.join(format!(
        "dm-wt-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).context("scratch")?;

    let mut active: Vec<Mutex<Child>> = Vec::new();
    let mut log_paths: Vec<PathBuf> = Vec::new();
    let retries = AtomicU64::new(0);
    let query_ms: Mutex<Vec<f64>> = Mutex::new(Vec::new());
    let rss_samples: Mutex<Vec<i64>> = Mutex::new(Vec::new());
    let outcome = execute(
        &args,
        &binary,
        probe.as_deref(),
        &probe_admission,
        &env,
        &scratch,
        &mut report,
        &mut active,
        &mut log_paths,
        &retries,
        &query_ms,
        &rss_samples,
    );

    let mut cleanup_errors = Vec::new();
    for child in &active {
        let mut child = child.lock().expect("child");
        let id = child.id();
        if let Err(error) = stop(&mut child, true) {
            cleanup_errors.push(format!("child {id}: {error}"));
        }
    }
    if !report["passed"].as_bool().unwrap_or(false) {
        let mut samples = Vec::new();
        for path in log_paths.iter().take(4) {
            if let Ok(mut log) = File::open(path) {
                let len = log.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = log.seek(SeekFrom::Start(len.saturating_sub(8192)));
                let mut buf = Vec::new();
                let _ = log.read_to_end(&mut buf);
                samples.push(String::from_utf8_lossy(&buf).into_owned());
            }
        }
        report["daemon_log_samples"] = json!(samples);
    }
    if let Err(error) = fs::remove_dir_all(&scratch) {
        cleanup_errors.push(format!("scratch cleanup: {error}"));
        report["passed"] = json!(false);
    }
    report["elapsed_seconds"] = json!((started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0);
    report["ipc_probe_peak_active"] = json!(probe_admission.peak());
    report["filesystem_retry_attempts"] = json!(retries.load(Ordering::Relaxed));
    report["daemon_rss_kib_samples"] = json!(&*rss_samples.lock().expect("rss"));
    let measured = {
        let mut ms = query_ms.lock().expect("query_ms").clone();
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ms
    };
    if !measured.is_empty() {
        let p95_idx = ((measured.len() as f64) * 0.95) as usize;
        report["query_latency_ms"] = json!({
            "samples": measured.len(),
            "p50": (measured[measured.len() / 2] * 1000.0).round() / 1000.0,
            "p95": (measured[p95_idx.min(measured.len() - 1)] * 1000.0).round() / 1000.0,
            "max": (measured[measured.len() - 1] * 1000.0).round() / 1000.0,
        });
    }
    if !cleanup_errors.is_empty() {
        report["cleanup_errors"] = json!(cleanup_errors);
        report["passed"] = json!(false);
    }
    if let Err(error) = &outcome {
        report["error"] = json!(format!("{error:#}"));
    }
    report["passed"] = json!(finalize_pass_flag(
        report["passed"].as_bool().unwrap_or(false),
        outcome.is_err(),
        !cleanup_errors.is_empty(),
    ));
    let encoded = serde_json::to_string_pretty(&report)? + "\n";
    fs::write(&args.output, &encoded).context("write report")?;
    println!("{}", serde_json::to_string(&report)?);
    if !cleanup_errors.is_empty() {
        bail!("harness cleanup failed: {cleanup_errors:?}");
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
fn execute(
    args: &Args,
    binary: &Path,
    probe: Option<&Path>,
    probe_admission: &ProbeAdmission,
    env: &HashMap<OsString, OsString>,
    scratch: &Path,
    report: &mut Value,
    active: &mut Vec<Mutex<Child>>,
    log_paths: &mut Vec<PathBuf>,
    retries: &AtomicU64,
    query_ms: &Mutex<Vec<f64>>,
    rss_samples: &Mutex<Vec<i64>>,
) -> Result<()> {
    let main_tree = scratch.join("main");
    fs::create_dir_all(&main_tree)?;
    let hooks = scratch.join("empty-hooks");
    fs::create_dir_all(&hooks)?;
    run_cmd(
        &[
            OsString::from("git"),
            "init".into(),
            "-q".into(),
            os(&main_tree),
        ],
        None,
        env,
        scratch,
    )?;
    run_cmd(
        &[
            OsString::from("git"),
            "config".into(),
            "core.autocrlf".into(),
            "false".into(),
        ],
        Some(&main_tree),
        env,
        scratch,
    )?;
    fs::write(
        main_tree.join(".gitignore"),
        ".devmap/\n.devcouncil/\nAGENTS.md\n",
    )?;
    fs::write(
        main_tree.join("common.py"),
        "def stable_helper():\n    return 1\ndef stable_caller():\n    return stable_helper()\n",
    )?;
    for module in 0..args.fixture_files {
        let mut body = String::new();
        for leaf in 0..args.functions_per_file {
            let expression = if leaf == 0 {
                module.to_string()
            } else {
                format!("fixture_{}_{}()", module, leaf - 1)
            };
            body.push_str(&format!(
                "def fixture_{module}_{leaf}():\n    return {expression}\n"
            ));
        }
        fs::write(main_tree.join(format!("fixture_{module}.py")), body)?;
    }
    run_cmd(
        &[OsString::from("git"), "add".into(), ".".into()],
        Some(&main_tree),
        env,
        scratch,
    )?;
    git_commit(&main_tree, &hooks, "fixture", false, env, scratch)?;

    let roots: Vec<PathBuf> = (0..args.worktrees)
        .map(|i| scratch.join(format!("w{i}")))
        .collect();
    for root in &roots {
        run_cmd(
            &[
                OsString::from("git"),
                "worktree".into(),
                "add".into(),
                "-q".into(),
                "--detach".into(),
                os(root),
                "HEAD".into(),
            ],
            Some(&main_tree),
            env,
            scratch,
        )?;
    }
    let exclude = main_tree.join(".git/info/exclude");
    fs::write(&exclude, "excluded.py\n")?;
    for (i, root) in roots.iter().enumerate() {
        fs::write(
            root.join("excluded.py"),
            format!("def excluded_w{i}():\n    return 1\n"),
        )?;
    }
    println!("created {} real linked worktrees", roots.len());

    let binary_os = os(binary);
    parallel(args.build_workers, roots.clone(), |root| {
        run_cmd(
            &[
                binary_os.clone(),
                "build".into(),
                os(&root),
                "--json".into(),
            ],
            None,
            env,
            scratch,
        )
        .map(|_| ())
    })?;

    let dbs: Vec<PathBuf> = roots
        .iter()
        .map(|root| root.join(".devmap/codeintel/devmap.sqlite"))
        .collect();
    let mut initial_edges = None;
    let expected_symbols = report["initial_symbols_per_worktree"].as_u64().unwrap() as usize;
    for db in &dbs {
        let snap = snapshot(db)?;
        ensure!(
            snap.nodes.len() == expected_symbols,
            "fixture symbol coverage mismatch: {}, sample={:?}",
            snap.nodes.len(),
            &snap.nodes[..snap.nodes.len().min(6)]
        );
        let files = snap.nodes.iter().filter(|node| node.2 == "File").count();
        let functions = snap
            .nodes
            .iter()
            .filter(|node| node.2 == "Function")
            .count();
        let calls = snap.edges.iter().filter(|edge| edge.2 == "Calls").count();
        ensure!(files == 1 + args.fixture_files);
        ensure!(functions == 2 + args.fixture_files * args.functions_per_file);
        ensure!(
            calls == 1 + args.fixture_files * (args.functions_per_file.saturating_sub(1)),
            "fixture edge coverage mismatch"
        );
        if let Some(expected) = initial_edges {
            ensure!(snap.edges.len() == expected);
        }
        initial_edges = Some(snap.edges.len());
        ensure!(snap.pending == 0);
    }
    report["initial_edges_per_worktree"] = json!(initial_edges.unwrap());

    let endpoints: Vec<PathBuf> = if cfg!(windows) {
        (0..args.worktrees)
            .map(|i| {
                PathBuf::from(format!(
                    r"\\.\pipe\devmap-capacity-{}-{i}",
                    std::process::id()
                ))
            })
            .collect()
    } else {
        (0..args.worktrees)
            .map(|i| scratch.join(format!("s{i}")))
            .collect()
    };

    for i in 0..roots.len() {
        active.push(Mutex::new(start_daemon(
            binary,
            &roots[i],
            &endpoints[i],
            scratch,
            i,
            env,
            log_paths,
        )?));
    }

    let n = roots.len();
    for i in 0..n {
        let mut child = active[i].lock().expect("child");
        ready_one(
            i,
            &mut child,
            probe,
            probe_admission,
            env,
            scratch,
            &endpoints[i],
        )?;
    }
    println!("{n} daemons answering IPC concurrently");

    let wait_metadata = |i: usize, present: Option<bool>, head: Option<&str>| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            let snap = snapshot(&dbs[i])?;
            let found = snap
                .nodes
                .iter()
                .any(|node| node.1.ends_with(&format!("::excluded_w{i}")));
            let stored = stored_head(&dbs[i])?;
            if snap.pending == 0
                && present.is_none_or(|want| found == want)
                && head.is_none_or(|want| stored == want)
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!("metadata-only change did not converge in worktree {i}: present={present:?}, head={head:?}")
    };
    parallel(args.build_workers, (0..n).collect(), |i| {
        wait_metadata(i, Some(false), None)
    })?;
    fs::write(&exclude, "")?;
    parallel(args.build_workers, (0..n).collect(), |i| {
        wait_metadata(i, Some(true), None)
    })?;
    fs::write(&exclude, "excluded.py\n")?;
    parallel(args.build_workers, (0..n).collect(), |i| {
        wait_metadata(i, Some(false), None)
    })?;

    let heads = parallel(args.build_workers, roots.clone(), |root| {
        git_commit(
            &root,
            &hooks,
            &format!(
                "metadata-only edit for {}",
                root.file_name().unwrap().to_string_lossy()
            ),
            true,
            env,
            scratch,
        )?;
        let out = run_cmd(
            &[OsString::from("git"), "rev-parse".into(), "HEAD".into()],
            Some(&root),
            env,
            scratch,
        )?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    })?;
    ensure!(
        heads.iter().collect::<std::collections::HashSet<_>>().len() == n,
        "private HEAD edits must be distinct in every worktree"
    );
    let heads_for_wait = heads.clone();
    parallel(args.build_workers, (0..n).collect(), |i| {
        wait_metadata(i, None, Some(heads_for_wait[i].as_str()))
    })?;
    report["metadata_checks"] = json!(4 * n);
    report["distinct_private_heads"] =
        json!(heads.iter().collect::<std::collections::HashSet<_>>().len());
    println!("shared exclude changes and private HEAD-only commits converged");

    let windows = cfg!(windows);
    let mut edit_operations = 0u64;
    let mut ipc_queries = 0u64;
    let mut kills = 0u64;
    for cycle in 0..args.rounds {
        let barrier = Arc::new(StartBarrier::new(2 * n, 30.0).expect("start barrier"));
        let results = thread::scope(|scope| {
            let mut joins = Vec::new();
            for session in 0..(2 * n) {
                let barrier = Arc::clone(&barrier);
                let root = &roots[session / 2];
                let endpoint = &endpoints[session / 2];
                joins.push(scope.spawn(move || -> Result<(u64, f64)> {
                    let i = session / 2;
                    let agent = session % 2;
                    let path = root.join(format!("agent{agent}.py"));
                    barrier.wait()?;
                    for revision in 0..8 {
                        let stage = root.join(format!("agent{agent}.tmp"));
                        let body =
                            format!("def owner_w{i}_a{agent}_r{cycle}():\n    return {revision}\n");
                        mutate(&stage, retries, windows, || fs::write(&stage, &body))?;
                        mutate(&path, retries, windows, || fs::rename(&stage, &path))?;
                    }
                    let renamed = root.join(format!("renamed{agent}.py"));
                    mutate(&renamed, retries, windows, || fs::rename(&path, &renamed))?;
                    mutate(&path, retries, windows, || fs::rename(&renamed, &path))?;
                    let transient = root.join(format!("deleted{agent}.py"));
                    mutate(&transient, retries, windows, || {
                        fs::write(&transient, "def must_disappear():\n    return 1\n")
                    })?;
                    mutate(&transient, retries, windows, || fs::remove_file(&transient))?;
                    let query_start = Instant::now();
                    let result = ipc(
                        probe,
                        probe_admission,
                        env,
                        scratch,
                        endpoint,
                        json!({"cmd": "search", "query": "stable_helper"}),
                    )?;
                    let elapsed = query_start.elapsed().as_secs_f64() * 1000.0;
                    ensure!(result["total"] == json!(1), "{result}");
                    query_ms.lock().expect("query_ms").push(elapsed);
                    Ok((12, elapsed))
                }));
            }
            joins
                .into_iter()
                .map(|j| j.join().expect("editor"))
                .collect::<Vec<_>>()
        });
        for item in results {
            let (ops, _) = item?;
            edit_operations += ops;
        }
        ipc_queries += 2 * n as u64;

        let victims: Vec<usize> = (cycle % 4..n).step_by(4).collect();
        for &i in &victims {
            {
                let mut child = active[i].lock().expect("child");
                stop(&mut child, true)?;
                *child =
                    start_daemon(binary, &roots[i], &endpoints[i], scratch, i, env, log_paths)?;
            }
            kills += 1;
            let mut child = active[i].lock().expect("child");
            ready_one(
                i,
                &mut child,
                probe,
                probe_admission,
                env,
                scratch,
                &endpoints[i],
            )?;
        }

        let daemons: &[Mutex<Child>] = active;
        parallel(args.build_workers, (0..n).collect(), |i| {
            let deadline = Instant::now() + Duration::from_secs(120);
            while Instant::now() < deadline {
                if daemons[i]
                    .lock()
                    .expect("child")
                    .try_wait()?
                    .is_some()
                {
                    bail!("daemon {i} exited during resync");
                }
                let snap = snapshot(&dbs[i])?;
                let names: std::collections::HashSet<String> = snap
                    .nodes
                    .iter()
                    .map(|node| node.1.rsplit("::").next().unwrap_or(&node.1).to_string())
                    .collect();
                let expected: std::collections::HashSet<String> = (0..2)
                    .map(|a| format!("owner_w{i}_a{a}_r{cycle}"))
                    .collect();
                if snap.pending == 0
                    && expected.is_subset(&names)
                    && !names.contains("must_disappear")
                {
                    ensure!(!names
                        .iter()
                        .any(|name| { name.starts_with("owner_w") && !expected.contains(name) }));
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(100));
            }
            bail!("worktree {i} did not converge after round {cycle}")
        })?;

        let pids: Vec<String> = active
            .iter()
            .map(|c| c.lock().expect("child").id().to_string())
            .collect();
        let rss = if cfg!(windows) {
            let joined = pids.join(",");
            let out = run_cmd(
                &[
                    OsString::from("powershell.exe"),
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    format!(
                        "$ErrorActionPreference='Stop'; Get-Process -Id {joined} | ForEach-Object {{ [math]::Ceiling($_.WorkingSet64 / 1024) }}"
                    )
                    .into(),
                ],
                None,
                env,
                scratch,
            )?;
            String::from_utf8_lossy(&out)
                .split_whitespace()
                .map(|s| s.parse::<i64>())
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut argv = vec![
                OsString::from("ps"),
                "-o".into(),
                "rss=".into(),
                "-p".into(),
            ];
            argv.push(OsString::from(pids.join(",")));
            let out = run_cmd(&argv, None, env, scratch)?;
            String::from_utf8_lossy(&out)
                .split_whitespace()
                .map(|s| s.parse::<i64>())
                .collect::<Result<Vec<_>, _>>()?
        };
        ensure!(rss.len() == active.len(), "RSS sample missed a daemon");
        rss_samples.lock().expect("rss").push(rss.into_iter().sum());
        println!(
            "round {}: {} editors converged; {} crash/restarts",
            cycle + 1,
            2 * n,
            victims.len()
        );
    }

    for child in active.iter() {
        stop(&mut child.lock().expect("child"), false)?;
    }
    parallel(args.build_workers, (0..n).collect(), |i| {
        let cold = scratch.join(format!("cold{i}.sqlite"));
        run_cmd(
            &[
                os(binary),
                "--db".into(),
                os(&cold),
                "build".into(),
                os(&roots[i]),
                "--json".into(),
            ],
            None,
            env,
            scratch,
        )?;
        let incremental = snapshot(&dbs[i])?;
        let expected = snapshot(&cold)?;
        ensure!(
            incremental == expected,
            "incremental/cold mismatch in worktree {i}"
        );
        if cfg!(windows) {
            match ipc(
                probe,
                probe_admission,
                env,
                scratch,
                &endpoints[i],
                json!({"cmd": "status"}),
            ) {
                Err(error) if is_refused(&error) => {}
                Ok(_) => bail!("endpoint still serves after shutdown: {i}"),
                Err(error) => return Err(error),
            }
        } else {
            ensure!(!endpoints[i].exists(), "stale endpoint after shutdown: {i}");
        }
        Ok(())
    })?;
    report["edit_operations"] = json!(edit_operations);
    report["ipc_queries"] = json!(ipc_queries);
    report["kills"] = json!(kills);
    report["cold_comparisons"] = json!(n);
    report["passed"] = json!(true);
    Ok(())
}

fn ready_one(
    i: usize,
    child: &mut Child,
    probe: Option<&Path>,
    probe_admission: &ProbeAdmission,
    env: &HashMap<OsString, OsString>,
    scratch: &Path,
    endpoint: &Path,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let exited = child.try_wait()?.is_some();
        let can_attempt = cfg!(windows) || endpoint.exists();
        let mut ipc_ok = None;
        if !exited && can_attempt {
            match ipc(
                probe,
                probe_admission,
                env,
                scratch,
                endpoint,
                json!({"cmd": "status"}),
            ) {
                Ok(_) => ipc_ok = Some(true),
                Err(error) if is_refused(&error) => ipc_ok = Some(false),
                Err(error) => return Err(error),
            }
        }
        match classify_startup(exited, can_attempt, ipc_ok) {
            StartupDecision::Ready => return Ok(()),
            StartupDecision::Exited => bail!("daemon {i} exited during startup"),
            StartupDecision::Retry => thread::sleep(Duration::from_millis(50)),
        }
    }
    bail!("daemon {i} failed to bind")
}

fn start_daemon(
    binary: &Path,
    root: &Path,
    endpoint: &Path,
    scratch: &Path,
    i: usize,
    env: &HashMap<OsString, OsString>,
    log_paths: &mut Vec<PathBuf>,
) -> Result<Child> {
    let log_path = scratch.join(format!(
        "daemon-{i}-{}.log",
        RUN_TAG.fetch_add(1, Ordering::Relaxed)
    ));
    let log = File::create(&log_path)?;
    let log_err = log.try_clone()?;
    log_paths.push(log_path);
    let mut cmd = Command::new(binary);
    cmd.arg("serve").arg(root).arg("--socket").arg(endpoint);
    configure(&mut cmd, env);
    cmd.stdin(Stdio::null()).stdout(log).stderr(log_err);
    cmd.spawn().context("serve")
}

fn mutate(
    _path: &Path,
    retries: &AtomicU64,
    windows: bool,
    operation: impl FnMut() -> io::Result<()>,
) -> Result<()> {
    retry_file_operation(operation, windows, 5.0, || {
        retries.fetch_add(1, Ordering::Relaxed);
    })
    .map_err(anyhow::Error::from)
}

fn os(path: &Path) -> OsString {
    path.as_os_str().to_os_string()
}

fn os_devnull() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

fn scratch_parent() -> PathBuf {
    if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    }
}

fn platform_string() -> String {
    Command::new("uname")
        .arg("-a")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH))
}

fn sha256_file(path: &Path) -> Result<String> {
    for (cmd, extra) in [("shasum", &["-a", "256"][..]), ("sha256sum", &[])] {
        if let Ok(out) = Command::new(cmd).args(extra).arg(path).output() {
            if out.status.success() {
                if let Some(hash) = String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .next()
                {
                    return Ok(hash.to_string());
                }
            }
        }
    }
    #[cfg(windows)]
    {
        let out = Command::new("certutil")
            .args(["-hashfile"])
            .arg(path)
            .arg("SHA256")
            .output()
            .context("certutil")?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(hash) = text.lines().nth(1) {
                return Ok(hash.split_whitespace().collect());
            }
        }
    }
    bail!("no sha256 tool (shasum, sha256sum, or certutil)")
}

fn configure(cmd: &mut Command, env: &HashMap<OsString, OsString>) {
    cmd.env_clear().envs(env);
    cmd.stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
}

static RUN_TAG: AtomicU64 = AtomicU64::new(0);

enum CmdOutcome {
    Ok(Vec<u8>),
    Failed { status: ExitStatus, stderr: Vec<u8> },
}

fn run_cmd_outcome(
    argv: &[OsString],
    cwd: Option<&Path>,
    env: &HashMap<OsString, OsString>,
    scratch: &Path,
) -> Result<CmdOutcome> {
    let tag = RUN_TAG.fetch_add(1, Ordering::Relaxed);
    let stdout_path = scratch.join(format!("run-{tag}.out"));
    let stderr_path = scratch.join(format!("run-{tag}.err"));
    let stdout = File::create(&stdout_path)?;
    let stderr = File::create(&stderr_path)?;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    configure(&mut cmd, env);
    cmd.stdout(stdout).stderr(stderr);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {:?}", argv[0]))?;
    match wait_timeout(&mut child, Duration::from_secs(90))? {
        Some(status) if status.success() => {
            let out = fs::read(&stdout_path)?;
            ensure!(
                out.len() <= 2_000_000,
                "subprocess output exceeded harness bound"
            );
            Ok(CmdOutcome::Ok(out))
        }
        Some(status) => {
            let err = fs::read(&stderr_path).unwrap_or_default();
            ensure!(
                err.len() <= 2_000_000,
                "subprocess output exceeded harness bound"
            );
            Ok(CmdOutcome::Failed {
                status,
                stderr: err,
            })
        }
        None => {
            let _ = stop(&mut child, true);
            bail!("command {argv:?} timed out");
        }
    }
}

fn run_cmd(
    argv: &[OsString],
    cwd: Option<&Path>,
    env: &HashMap<OsString, OsString>,
    scratch: &Path,
) -> Result<Vec<u8>> {
    match run_cmd_outcome(argv, cwd, env, scratch)? {
        CmdOutcome::Ok(out) => Ok(out),
        CmdOutcome::Failed { status, stderr } => {
            let tail = String::from_utf8_lossy(&stderr[..stderr.len().min(8192)]);
            bail!("command {argv:?} failed ({status:?}): {tail}");
        }
    }
}

fn git_commit(
    cwd: &Path,
    hooks: &Path,
    message: &str,
    allow_empty: bool,
    env: &HashMap<OsString, OsString>,
    scratch: &Path,
) -> Result<Vec<u8>> {
    let mut hooks_arg = OsString::from("core.hooksPath=");
    hooks_arg.push(hooks);
    let mut argv = vec![
        OsString::from("git"),
        "-c".into(),
        "user.name=DevMap test".into(),
        "-c".into(),
        "user.email=devmap-test@invalid".into(),
        "-c".into(),
        hooks_arg,
        "commit".into(),
    ];
    if allow_empty {
        argv.push("--allow-empty".into());
    }
    argv.push("-qm".into());
    argv.push(message.into());
    run_cmd(&argv, Some(cwd), env, scratch)
}

fn wait_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if start.elapsed() >= timeout {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop(child: &mut Child, crash: bool) -> Result<()> {
    if let Some(status) = child.try_wait()? {
        if !crash && !status.success() {
            bail!("daemon graceful shutdown failed: {status:?}");
        }
        return Ok(());
    }
    let pid = child.id();
    if crash {
        let _ = child.kill();
        #[cfg(unix)]
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    } else {
        #[cfg(unix)]
        unsafe {
            libc::killpg(pid as i32, libc::SIGTERM);
        }
        #[cfg(windows)]
        unsafe {
            GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid);
        }
    }
    match wait_timeout(child, Duration::from_secs(15))? {
        Some(status) => {
            if !crash && !status.success() {
                bail!("daemon graceful shutdown failed: {status:?}");
            }
            Ok(())
        }
        None => {
            let _ = child.kill();
            #[cfg(unix)]
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
            let _ = wait_timeout(child, Duration::from_secs(10))?;
            if !crash {
                bail!("daemon did not shut down within 15 seconds");
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
extern "system" {
    fn GenerateConsoleCtrlEvent(dw_ctrl_event: u32, dw_process_group_id: u32) -> i32;
}

fn parallel<T, R>(
    workers: usize,
    items: Vec<T>,
    f: impl Fn(T) -> Result<R> + Sync + Send,
) -> Result<Vec<R>>
where
    T: Send,
    R: Send,
{
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
        .build()
        .context("thread pool")?;
    pool.install(|| items.into_par_iter().map(f).collect())
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    nodes: Vec<(String, String, String, i64, i64)>,
    edges: Vec<(String, String, String, f64, String)>,
    pending: i64,
}

fn snapshot(db: &Path) -> Result<Snapshot> {
    let conn = open_readonly(db)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    ensure!(integrity == "ok", "integrity_check: {integrity}");
    let fk: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    ensure!(fk.is_empty(), "foreign_key_check: {fk:?}");
    let generation: i64 = conn
        .query_row("SELECT max(id) FROM generations", [], |row| row.get(0))
        .context("no generation")?;
    let mut nodes = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT p.path,n.qualified_name,n.kind,n.span_start,n.span_end \
             FROM generation_nodes n JOIN paths p ON p.id=n.file_id \
             WHERE n.generation_id=?1 ORDER BY 1,2,3,4,5",
        )?;
        let rows = stmt.query_map([generation], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?;
        for row in rows {
            nodes.push(row?);
        }
    }
    let mut edges = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT source_symbol,target_symbol,edge_kind,confidence,resolution \
             FROM generation_edges WHERE generation_id=?1 ORDER BY 1,2,3,4,5",
        )?;
        let rows = stmt.query_map([generation], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            ))
        })?;
        for row in rows {
            edges.push(row?);
        }
    }
    let pending: i64 =
        conn.query_row("SELECT count(*) FROM pending_paths", [], |row| row.get(0))?;
    ensure!(
        graph_proves_equivalence(nodes.len(), edges.len()),
        "empty graph cannot prove equivalence"
    );
    Ok(Snapshot {
        nodes,
        edges,
        pending,
    })
}

fn open_readonly(db: &Path) -> Result<Connection> {
    let uri = format!(
        "file:{}?mode=ro",
        db.display().to_string().replace('\\', "/")
    );
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("open {}", db.display()))?;
    conn.busy_timeout(Duration::from_secs(10))?;
    Ok(conn)
}

fn stored_head(db: &Path) -> Result<String> {
    let conn = open_readonly(db)?;
    conn.query_row(
        "SELECT head_sha FROM generations ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .context("head_sha")
}

fn ipc(
    probe: Option<&Path>,
    admission: &ProbeAdmission,
    env: &HashMap<OsString, OsString>,
    scratch: &Path,
    endpoint: &Path,
    command: Value,
) -> Result<Value> {
    let mut body = command;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("version".into(), json!(1));
    }
    if let Some(probe) = probe {
        let frame = serde_json::to_string(&body)?;
        let endpoint_s = endpoint.as_os_str().to_os_string();
        let probe_s = probe.as_os_str().to_os_string();
        let data = admission.run(|| -> Result<Vec<u8>> {
            match run_cmd_outcome(
                &[probe_s.clone(), endpoint_s.clone(), OsString::from(&frame)],
                None,
                env,
                scratch,
            )? {
                CmdOutcome::Ok(data) => Ok(data),
                CmdOutcome::Failed { status, .. } if status.code() == Some(2) => {
                    Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        endpoint.display().to_string(),
                    )
                    .into())
                }
                CmdOutcome::Failed { status, stderr } => {
                    let tail = String::from_utf8_lossy(&stderr[..stderr.len().min(8192)]);
                    bail!("ipc probe failed ({status:?}): {tail}")
                }
            }
        })?;
        let result: Value = serde_json::from_slice(&data)?;
        ensure!(
            result.get("ok") == Some(&json!(true)),
            "IPC error: {result}"
        );
        return Ok(result["result"].clone());
    }
    ipc_unix(endpoint, &body)
}

#[cfg(unix)]
fn ipc_unix(endpoint: &Path, command: &Value) -> Result<Value> {
    use std::os::unix::net::UnixStream;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = UnixStream::connect(endpoint)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut frame = serde_json::to_vec(command)?;
    frame.push(b'\n');
    stream.write_all(&frame)?;
    let mut data = Vec::new();
    loop {
        if Instant::now() >= deadline {
            bail!("IPC exchange exceeded 30 seconds");
        }
        let mut buf = [0u8; 65536];
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                ensure!(
                    data.len() <= 2_000_000,
                    "IPC response exceeded harness bound"
                );
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                bail!("IPC exchange exceeded 30 seconds");
            }
            Err(error) => return Err(error.into()),
        }
    }
    let result: Value = serde_json::from_slice(&data)?;
    ensure!(
        result.get("ok") == Some(&json!(true)),
        "IPC error: {result}"
    );
    Ok(result["result"].clone())
}

#[cfg(windows)]
fn ipc_unix(_endpoint: &Path, _command: &Value) -> Result<Value> {
    bail!("Windows requires --ipc-probe")
}

fn is_refused(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(ioe) = cause.downcast_ref::<io::Error>() {
            matches!(
                ioe.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            )
        } else {
            let text = cause.to_string();
            text.contains("Connection refused") || text.contains("exit 2")
        }
    })
}
