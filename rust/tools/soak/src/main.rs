//! `soak <workdir> [cycles] [--daemon]` — see the library docs for what it
//! asserts.
//!
//! Every cycle is sampled (cycle, peak/resident RSS, store bytes) into a CSV,
//! and the plateau assertion compares the means of two halves taken after a
//! warm-up. Under 40 cycles the run is a smoke test: the digest must return
//! after every restore, and growth is reported but not asserted. `verify.sh`
//! runs five cycles and reads the verdict line.
//!
//! Environment: `DEVMAP_BIN` (default `$CARGO_TARGET_DIR/release/devmap`, else
//! the workspace's `target/release/devmap`), `SOAK_CSV`, `SOAK_TOLERANCE_PCT`
//! (10), `SOAK_SETTLE_SECONDS` (45, 1..300), `SOAK_SHUTDOWN_SECONDS` (10,
//! 1..30), `TMPDIR` for the daemon's socket.
//!
//! Exit codes: 0 the soak passed · 1 it failed, or a check could not run ·
//! 2 usage.

fn main() {
    std::process::exit(soak::run());
}

#[cfg(not(unix))]
mod soak {
    pub fn run() -> i32 {
        println!(
            "SOAK FAIL: the soak drives Unix sockets and process groups; run it on Linux or macOS"
        );
        1
    }
}

#[cfg(unix)]
mod soak {
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use devmap_soak::corpus::Corpus;
    use devmap_soak::daemon::{self, Daemon, Identity};
    use devmap_soak::query::{admit, Mode, Observation, Transport};
    use devmap_soak::samples::{assert_plateau, Sample, Samples};
    use devmap_soak::{ipc, rss, store, Cleanup};

    struct Config {
        root: PathBuf,
        cycles: u64,
        daemon: bool,
        mode_label: String,
        settle: Duration,
        shutdown: Duration,
        devmap: PathBuf,
        csv: PathBuf,
        tolerance: u64,
    }

    enum Refusal {
        Usage(String),
        Fail(String),
    }

    fn bounded(name: &str, default: u64, lo: u64, hi: u64) -> Result<u64, Refusal> {
        let raw = std::env::var(name).unwrap_or_else(|_| default.to_string());
        let refuse = || Refusal::Fail(format!("SOAK FAIL: {name} must be {lo}..{hi}"));
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(refuse());
        }
        let value: u64 = raw.parse().map_err(|_| refuse())?;
        if value < lo || value > hi {
            return Err(refuse());
        }
        Ok(value)
    }

    fn absolute(path: PathBuf) -> PathBuf {
        if path.is_absolute() {
            path
        } else {
            std::env::current_dir().map_or(path.clone(), |cwd| cwd.join(path))
        }
    }

    /// Honour CARGO_TARGET_DIR: lanes build into their own target directory,
    /// and a soak that silently measured a stale `rust/target/release/devmap`
    /// would report the previous kernel's numbers as this one's.
    fn default_devmap() -> PathBuf {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
                    .join("target")
            });
        target.join("release").join("devmap")
    }

    fn config() -> Result<Config, Refusal> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let usage = || Refusal::Usage("usage: soak <workdir> [cycles] [--daemon]".into());
        let root = args.first().filter(|r| !r.is_empty()).ok_or_else(usage)?;
        if args.len() > 3 {
            return Err(usage());
        }
        let cycles = match args.get(1) {
            None => 40,
            Some(c) => c.parse::<u64>().ok().filter(|c| *c >= 1).ok_or_else(|| {
                Refusal::Fail(format!(
                    "SOAK FAIL: cycles must be a positive integer, got {c:?}"
                ))
            })?,
        };
        let mode_label = args.get(2).cloned().unwrap_or_else(|| "build".into());
        let daemon = match mode_label.as_str() {
            "build" => false,
            "--daemon" => true,
            other => {
                return Err(Refusal::Usage(format!(
                    "usage: soak <workdir> [cycles] [--daemon]; unknown mode {other:?}"
                )))
            }
        };
        let settle = bounded("SOAK_SETTLE_SECONDS", 45, 1, 300)?;
        let shutdown = bounded("SOAK_SHUTDOWN_SECONDS", 10, 1, 30)?;
        let tolerance = bounded("SOAK_TOLERANCE_PCT", 10, 0, 1000)?;
        let devmap = absolute(
            std::env::var_os("DEVMAP_BIN")
                .filter(|v| !v.is_empty())
                .map_or_else(default_devmap, PathBuf::from),
        );
        let root = std::fs::canonicalize(root)
            .map_err(|e| Refusal::Fail(format!("SOAK FAIL: cannot enter {root}: {e}")))?;
        let csv = absolute(
            std::env::var_os("SOAK_CSV")
                .filter(|v| !v.is_empty())
                .map_or_else(|| root.join("soak_samples.csv"), PathBuf::from),
        );
        Ok(Config {
            root,
            cycles,
            daemon,
            mode_label,
            settle: Duration::from_secs(settle),
            shutdown: Duration::from_secs(shutdown),
            devmap,
            csv,
            tolerance,
        })
    }

    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }

    fn quiet(devmap: &Path, args: &[&str]) -> bool {
        Command::new(devmap)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn check_cli_symbol(devmap: &Path, name: &str, mode: Mode, relative: &str) -> Observation {
        let Ok(output) = Command::new(devmap)
            .args(["--json", "search", name])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
        else {
            return Observation::Unexaminable;
        };
        if !output.status.success() {
            return Observation::Unexaminable;
        }
        admit(
            Transport::Cli,
            mode,
            name,
            relative,
            &String::from_utf8_lossy(&output.stdout),
        )
    }

    fn short(digest: &str) -> &str {
        &digest[..digest.len().min(12)]
    }

    /// SIGINT and SIGTERM run the same cleanup as a normal exit, then exit
    /// 130 / 143. The daemon leads its own process group, so a terminal's
    /// Ctrl-C never reaches it: without this an interrupted soak would leave
    /// it serving the corpus with its idle timeout disabled.
    fn on_interrupt(cleanup: Arc<Mutex<Cleanup>>) {
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let code = runtime.block_on(async {
                use tokio::signal::unix::{signal, SignalKind};
                let (Ok(mut interrupt), Ok(mut terminate)) = (
                    signal(SignalKind::interrupt()),
                    signal(SignalKind::terminate()),
                ) else {
                    return None;
                };
                tokio::select! {
                    _ = interrupt.recv() => Some(130),
                    _ = terminate.recv() => Some(143),
                }
            });
            if let Some(code) = code {
                let mut guard = cleanup
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.run(&mut io::stdout());
                std::process::exit(code);
            }
        });
    }

    pub fn run() -> i32 {
        let mut out = io::stdout();
        let cfg = match config() {
            Ok(cfg) => cfg,
            Err(Refusal::Usage(msg)) => {
                eprintln!("{msg}");
                return 2;
            }
            Err(Refusal::Fail(msg)) => {
                println!("{msg}");
                return 1;
            }
        };
        if !is_executable(&cfg.devmap) {
            println!("SOAK FAIL: no devmap binary at {}", cfg.devmap.display());
            return 1;
        }
        let Some(time_flag) = rss::time_flag() else {
            println!("SOAK FAIL: no /usr/bin/time supporting -l or -v");
            return 1;
        };
        if let Err(e) = std::env::set_current_dir(&cfg.root) {
            println!("SOAK FAIL: cannot enter {}: {e}", cfg.root.display());
            return 1;
        }
        let corpus = match Corpus::open(&cfg.root, std::process::id()) {
            Ok(corpus) => corpus,
            Err(e) => {
                println!("SOAK FAIL: {e}");
                return 1;
            }
        };
        let cleanup = Arc::new(Mutex::new(Cleanup {
            corpus: Some(corpus.clone()),
            ..Cleanup::default()
        }));
        on_interrupt(Arc::clone(&cleanup));
        let code = soak(&cfg, time_flag, &corpus, &cleanup, &mut out);
        let mut guard = cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cleaned = guard.run(&mut out);
        let _ = out.flush();
        if cleaned {
            code
        } else {
            1
        }
    }

    fn soak(
        cfg: &Config,
        time_flag: &str,
        corpus: &Corpus,
        cleanup: &Arc<Mutex<Cleanup>>,
        out: &mut dyn Write,
    ) -> i32 {
        let devmap = cfg.devmap.as_path();
        if !quiet(devmap, &["build", "."]) {
            let _ = writeln!(out, "SOAK FAIL: initial build");
            return 1;
        }
        let Ok(store_path) = store::resolve(devmap, &cfg.root) else {
            let _ = writeln!(
                out,
                "SOAK FAIL: the kernel did not report a db_path in `devmap --json status`"
            );
            return 1;
        };
        if !store_path.is_file() {
            let _ = writeln!(
                out,
                "SOAK FAIL: the kernel reports its store at {}, which does not exist after the initial build",
                store_path.display()
            );
            return 1;
        }
        let base_digest = match store::digest(&store_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("SOAK FAIL: {e}");
                let _ = writeln!(out, "SOAK FAIL: baseline graph could not be measured");
                return 1;
            }
        };
        let base_db = store::bytes(&store_path);
        let _ = writeln!(
            out,
            "soak baseline: mode={} digest={} db={base_db} tolerance={}%",
            cfg.mode_label,
            short(&base_digest),
            cfg.tolerance
        );
        let samples = match Samples::create(&cfg.csv) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("SOAK FAIL: cannot write CSV {}", cfg.csv.display());
                return 1;
            }
        };
        let timeout_log = cfg.root.join(format!(".soak_time.{}", std::process::id()));
        let mut fails = if cfg.daemon {
            match drive_daemon(
                cfg,
                time_flag,
                corpus,
                cleanup,
                &store_path,
                &base_digest,
                &samples,
                &timeout_log,
                out,
            ) {
                Ok(failed) => failed,
                Err(code) => return code,
            }
        } else {
            drive_builds(
                cfg,
                time_flag,
                corpus,
                &store_path,
                &base_digest,
                &samples,
                &timeout_log,
                out,
            )
        };
        let _ = std::fs::remove_file(&timeout_log);

        let end_db = store::bytes(&store_path);
        let _ = writeln!(out, "soak end: db={end_db} (baseline {base_db})");

        if !fails && cfg.cycles >= 40 {
            let plateau = samples.read().and_then(|rows| {
                assert_plateau(&rows, cfg.cycles, cfg.daemon, base_db, cfg.tolerance, out)
            });
            match plateau {
                Ok(true) => {
                    let _ = writeln!(
                        out,
                        "SOAK OK ({} cycles, digest stable, growth bounded); samples in {}",
                        cfg.cycles,
                        samples.path().display()
                    );
                }
                Ok(false) => fails = true,
                Err(e) => {
                    let _ = writeln!(
                        out,
                        "SOAK FAIL: the samples in {} could not be read: {e}",
                        samples.path().display()
                    );
                    fails = true;
                }
            }
        } else if !fails {
            // Two verdict words on purpose. `verify.sh` runs five cycles for
            // incremental equivalence and reads this one; a single "SOAK OK"
            // was once printed here too, for a check that had not run.
            let _ = writeln!(
                out,
                "SOAK SMOKE OK ({} cycles, digest stable; growth NOT asserted — under the 40-cycle minimum); samples in {}",
                cfg.cycles,
                samples.path().display()
            );
        }
        i32::from(fails)
    }

    #[allow(clippy::too_many_arguments)]
    fn drive_builds(
        cfg: &Config,
        time_flag: &str,
        corpus: &Corpus,
        store_path: &Path,
        base_digest: &str,
        samples: &Samples,
        timeout_log: &Path,
        out: &mut dyn Write,
    ) -> bool {
        let devmap = cfg.devmap.as_path();
        for i in 1..=cfg.cycles {
            let name = format!("_soak_{i}");
            // Perturb, rebuild, then restore and rebuild: the digest must return.
            if corpus.append_cycle(i).is_err() || !quiet(devmap, &["build", "."]) {
                let _ = writeln!(out, "SOAK FAIL: build (dirty) cycle {i}");
                return true;
            }
            if check_cli_symbol(devmap, &name, Mode::Present, &corpus.relative)
                != Observation::Observed
            {
                let _ = writeln!(out, "SOAK FAIL: inserted symbol cycle {i}");
                return true;
            }
            for (q, args) in [
                ("search", &["search", "soak"][..]),
                ("dead", &["dead"][..]),
                ("status", &["status"][..]),
            ] {
                if !quiet(devmap, args) {
                    let _ = writeln!(out, "SOAK FAIL: {q} query cycle {i}");
                    return true;
                }
            }
            if corpus.restore().is_err() {
                let _ = writeln!(out, "SOAK FAIL: restore cycle {i}");
                return true;
            }
            let rss = match rss::measure(time_flag, timeout_log, devmap, &["build", "."]) {
                Ok(rss) => rss,
                Err(_) => {
                    let _ = writeln!(out, "SOAK FAIL: build (restored) cycle {i}");
                    return true;
                }
            };
            if check_cli_symbol(devmap, &name, Mode::Absent, &corpus.relative)
                != Observation::Observed
            {
                let _ = writeln!(out, "SOAK FAIL: removed symbol cycle {i}");
                return true;
            }
            let digest = match store::digest(store_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("SOAK FAIL: {e}");
                    let _ = writeln!(out, "SOAK FAIL: graph read cycle {i}");
                    return true;
                }
            };
            if digest != base_digest {
                let _ = writeln!(
                    out,
                    "SOAK FAIL: digest drift at cycle {i} ({} != {})",
                    short(&digest),
                    short(base_digest)
                );
                return true;
            }
            let db = store::bytes(store_path);
            if samples.append(Sample { cycle: i, rss, db }).is_err() {
                eprintln!(
                    "SOAK FAIL: cannot append CSV {} at cycle {i}",
                    samples.path().display()
                );
                return true;
            }
            if i % 10 == 0 {
                let _ = writeln!(out, "  cycle {i} ok rss={rss} db={db}");
            }
        }
        false
    }

    fn lock(cleanup: &Arc<Mutex<Cleanup>>) -> std::sync::MutexGuard<'_, Cleanup> {
        cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Ok(true) when the soak failed in a way that still reaches the verdict
    /// lines; Err(code) when it must stop before them.
    #[allow(clippy::too_many_arguments)]
    fn drive_daemon(
        cfg: &Config,
        time_flag: &str,
        corpus: &Corpus,
        cleanup: &Arc<Mutex<Cleanup>>,
        store_path: &Path,
        base_digest: &str,
        samples: &Samples,
        timeout_log: &Path,
        out: &mut dyn Write,
    ) -> Result<bool, i32> {
        use std::os::unix::fs::FileTypeExt;
        let devmap = cfg.devmap.as_path();
        // A socket of this soak's own: the default endpoint is shared with
        // whatever daemon the developer's checkout already runs, and this one
        // lets the soak observe its own startup. Under TMPDIR, not the
        // workdir: `sun_path` is 104 bytes and a scratch workdir is routinely
        // longer, so a socket beside the corpus is silently never bound.
        let tmp = std::env::var_os("TMPDIR")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        let endpoint = tmp.join(format!("devmap-soak-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&endpoint);
        let endpoint_arg = endpoint.to_string_lossy().into_owned();
        let spawned = Daemon::spawn(
            time_flag,
            devmap,
            &["serve", ".", "--socket", &endpoint_arg],
            &[("DEVMAP_MAX_IDLE_SECS", "0")],
            &cfg.root,
            timeout_log,
            cfg.shutdown,
        );
        let daemon = match spawned {
            Ok(d) => d,
            Err(e) => {
                let _ = writeln!(out, "SOAK FAIL: cannot start the daemon: {e}");
                return Err(1);
            }
        };
        {
            let mut guard = lock(cleanup);
            guard.daemon = Some(daemon);
            guard.endpoint = Some(endpoint.clone());
        }
        let bound =
            || std::fs::symlink_metadata(&endpoint).is_ok_and(|m| m.file_type().is_socket());
        let mut serve_pid = None;
        for _ in 0..150 {
            let mut guard = lock(cleanup);
            let d = guard.daemon.as_mut().expect("the daemon was just stored");
            match d.identify() {
                Identity::Found(pid) => serve_pid = Some(pid),
                Identity::NotYet => {}
                Identity::Ambiguous => {
                    let _ = writeln!(out, "SOAK FAIL: ambiguous daemon child identity");
                    return Err(1);
                }
                Identity::Unexaminable => {
                    let _ = writeln!(out, "SOAK FAIL: cannot examine daemon child identity");
                    return Err(1);
                }
            }
            if bound() && serve_pid.is_some() {
                break;
            }
            if !d.wrapper_running() {
                break;
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(200));
        }
        if !bound() {
            let tail: Vec<String> = std::fs::read_to_string(timeout_log)
                .unwrap_or_default()
                .lines()
                .rev()
                .take(5)
                .map(str::to_owned)
                .collect();
            let tail: Vec<String> = tail.into_iter().rev().collect();
            let _ = writeln!(
                out,
                "SOAK FAIL: the daemon never bound {}",
                endpoint.display()
            );
            let _ = writeln!(out, "  its output was: {}", tail.join("\n"));
            return Err(1);
        }
        let Some(serve_pid) = serve_pid else {
            let _ = writeln!(out, "SOAK FAIL: cannot identify the daemon process");
            return Err(1);
        };

        let relative = corpus.relative.as_str();
        let ask = |frame: &str, mode: Mode, name: &str| match ipc::exchange(&endpoint, frame) {
            Ok(reply) => admit(Transport::Ipc, mode, name, relative, &reply),
            Err(_) => Observation::Unexaminable,
        };
        let status_frame = r#"{"version":1,"cmd":"status"}"#;
        let await_symbol = |name: &str, mode: Mode, label: &str, out: &mut dyn Write| {
            let search =
                format!(r#"{{"version":1,"cmd":"search","query":"{name}","budget":2000}}"#);
            daemon::await_symbol(
                name,
                label,
                cfg.settle,
                || ask(&search, mode, name),
                || ask(status_frame, Mode::Fresh, ""),
                out,
            )
        };

        let mut fails = false;
        for i in 1..=cfg.cycles {
            let name = format!("_soak_{i}");
            // An OK envelope does not prove the edit was indexed. Keep the
            // edited bytes in place until the exact symbol/file and freshness
            // are observed.
            if corpus.append_cycle(i).is_err()
                || !await_symbol(&name, Mode::Present, "present", out)
            {
                let _ = writeln!(out, "SOAK FAIL: inserted symbol cycle {i}");
                fails = true;
                break;
            }
            if ask(
                r#"{"version":1,"cmd":"impact","target":"main"}"#,
                Mode::Validate,
                "",
            ) != Observation::Observed
            {
                let _ = writeln!(out, "SOAK FAIL: impact cycle {i}");
                fails = true;
                break;
            }
            if corpus.restore().is_err() || !await_symbol(&name, Mode::Absent, "absent", out) {
                let _ = writeln!(out, "SOAK FAIL: removed symbol cycle {i}");
                fails = true;
                break;
            }
            let rss = Command::new("ps")
                .args(["-o", "rss=", "-p", &serve_pid.to_string()])
                .stderr(Stdio::null())
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .and_then(|s| s.parse::<u64>().ok());
            let Some(rss_kib) = rss else {
                let _ = writeln!(out, "SOAK FAIL: the daemon died at cycle {i}");
                fails = true;
                break;
            };
            let db = store::bytes(store_path);
            if samples
                .append(Sample {
                    cycle: i,
                    rss: rss_kib * 1024,
                    db,
                })
                .is_err()
            {
                eprintln!(
                    "SOAK FAIL: cannot append CSV {} at cycle {i}",
                    samples.path().display()
                );
                fails = true;
                break;
            }
            if i % 10 == 0 {
                let _ = writeln!(out, "  cycle {i} ok rss={} db={db}", rss_kib * 1024);
            }
        }
        if !fails {
            // The last restore is another edit. Keep the daemon alive until
            // that edit is committed, then compare the graph actually measured
            // with the baseline. Watcher shutdown flushes its in-memory
            // debounce buffer, so freshness must hold beyond its 2 s quiet
            // window before the daemon is stopped.
            let deadline = Instant::now() + Duration::from_secs(90);
            let mut stable_since: Option<Instant> = None;
            let mut restored = false;
            while Instant::now() < deadline {
                match ask(status_frame, Mode::Fresh, "") {
                    Observation::Observed => {
                        let since = *stable_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= Duration::from_secs(3) {
                            restored = true;
                            break;
                        }
                    }
                    Observation::NotYet => stable_since = None,
                    Observation::Unexaminable => {
                        let _ = writeln!(out, "SOAK FAIL: final status query");
                        fails = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if !restored && !fails {
                let _ = writeln!(out, "SOAK FAIL: final restore did not become current");
                fails = true;
            }
        }
        if !fails {
            match store::digest(store_path) {
                Ok(d) if d == base_digest => {}
                Ok(_) => {
                    let _ = writeln!(out, "SOAK FAIL: final daemon graph differs from baseline");
                    fails = true;
                }
                Err(e) => {
                    eprintln!("SOAK FAIL: {e}");
                    let _ = writeln!(out, "SOAK FAIL: final graph could not be measured");
                    fails = true;
                }
            }
        }
        {
            let mut guard = lock(cleanup);
            if let Some(d) = guard.daemon.as_mut() {
                fails |= !d.stop(out);
            }
        }
        let _ = std::fs::remove_file(&endpoint);
        if !fails {
            // Verify the stopped state too: shutdown can flush a late watcher batch.
            let fresh = Command::new(devmap)
                .args(["--json", "status"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| {
                    admit(
                        Transport::Cli,
                        Mode::Fresh,
                        "",
                        "",
                        &String::from_utf8_lossy(&o.stdout),
                    )
                });
            if fresh != Some(Observation::Observed) {
                let _ = writeln!(out, "SOAK FAIL: stopped daemon left pending or stale work");
                fails = true;
            }
        }
        let peak = std::fs::read_to_string(timeout_log)
            .ok()
            .and_then(|r| rss::parse_peak(&r));
        let _ = writeln!(
            out,
            "daemon peak rss: {} bytes over {} cycles",
            peak.map_or_else(|| "unavailable".to_owned(), |p| p.to_string()),
            cfg.cycles
        );
        Ok(fails)
    }
}
