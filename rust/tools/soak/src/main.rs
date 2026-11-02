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
//! 1..30), `SOAK_IDLE_SECONDS` (35, 1..600: the daemon-mode idle stretch its
//! disk writes are judged over), `TMPDIR` for the daemon's socket.
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

    use std::sync::atomic::{AtomicBool, Ordering};

    use devmap_soak::args::Mode as RunMode;
    use devmap_soak::corpus::Corpus;
    use devmap_soak::daemon::{self, Daemon, Identity};
    use devmap_soak::query::{admit, Mode, Observation, Transport};
    use devmap_soak::readers::{self, ReaderStats, WriterTally};
    use devmap_soak::samples::{assert_plateau, Sample, Samples};
    use devmap_soak::{diskio, ipc, rss, stats, store, Cleanup};

    struct Config {
        root: PathBuf,
        cycles: u64,
        daemon: bool,
        mode: RunMode,
        mode_label: String,
        settle: Duration,
        shutdown: Duration,
        devmap: PathBuf,
        csv: PathBuf,
        tolerance: u64,
        idle: Duration,
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
        let parsed = devmap_soak::args::parse(&args).map_err(|refusal| match refusal {
            devmap_soak::args::Refusal::Usage(msg) => Refusal::Usage(msg),
            devmap_soak::args::Refusal::Fail(msg) => Refusal::Fail(msg),
        })?;
        let root = &parsed.root;
        let cycles = parsed.cycles;
        let mode_label = parsed.label.clone();
        let daemon = parsed.mode == RunMode::Daemon;
        let settle = bounded("SOAK_SETTLE_SECONDS", 45, 1, 300)?;
        let shutdown = bounded("SOAK_SHUTDOWN_SECONDS", 10, 1, 30)?;
        let tolerance = bounded("SOAK_TOLERANCE_PCT", 10, 0, 1000)?;
        let idle = bounded("SOAK_IDLE_SECONDS", 35, 1, 600)?;
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
            mode: parsed.mode,
            root,
            cycles,
            daemon,
            mode_label,
            settle: Duration::from_secs(settle),
            shutdown: Duration::from_secs(shutdown),
            devmap,
            csv,
            tolerance,
            idle: Duration::from_secs(idle),
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
                // HUP too: a closed terminal or a dropped ssh session ends
                // the soak, and the shell harness's EXIT trap ran for it.
                let (Ok(mut interrupt), Ok(mut terminate), Ok(mut hangup)) = (
                    signal(SignalKind::interrupt()),
                    signal(SignalKind::terminate()),
                    signal(SignalKind::hangup()),
                ) else {
                    return None;
                };
                tokio::select! {
                    _ = interrupt.recv() => Some(130),
                    _ = terminate.recv() => Some(143),
                    _ = hangup.recv() => Some(129),
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
        // A panic must still clean up. The interrupt thread holds a clone of
        // `cleanup`, so unwinding never drops it and the daemon's Drop never
        // runs; catching here is what stops a panic orphaning the daemon.
        let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            soak(&cfg, time_flag, &corpus, &cleanup, &mut out)
        }))
        .unwrap_or(1);
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
        // The `/usr/bin/time` report lives inside the corpus, so it is
        // registered for removal before anything writes it.
        let timeout_log = cfg.root.join(format!(".soak_time.{}", std::process::id()));
        lock(cleanup).timer_log = Some(timeout_log.clone());
        let load_start = load_average();
        let cold = match rss::measure_timed(time_flag, &timeout_log, devmap, &["build", "."]) {
            Ok(measured) => measured,
            Err(_) => {
                let _ = writeln!(out, "SOAK FAIL: initial build");
                return 1;
            }
        };
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
        let _ = writeln!(
            out,
            "soak initial build: wall {} ms, peak rss {} bytes (cold only if the workdir had no store); load average {load_start}",
            cold.wall.as_millis(),
            cold.peak
        );
        if let RunMode::Readers {
            readers,
            duration_secs,
        } = cfg.mode
        {
            let failed = drive_readers(
                cfg,
                time_flag,
                corpus,
                cleanup,
                &store_path,
                &base_digest,
                &samples,
                &timeout_log,
                readers,
                duration_secs,
                cold,
                &load_start,
                out,
            );
            let _ = std::fs::remove_file(&timeout_log);
            return i32::from(failed);
        }
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
                cleanup,
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
        // Ahead of the verdict on purpose: `verify.sh` reads the last two
        // lines for the verdict word.
        if !cfg.daemon {
            report_builds(&samples, cold, &load_start, out);
        }

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

    /// The 1, 5 and 15 minute load averages, or `unavailable`. The host is
    /// shared with an agent swarm, so every timing is reported beside it.
    fn load_average() -> String {
        let text = Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .or_else(|| std::fs::read_to_string("/proc/loadavg").ok());
        text.as_deref()
            .and_then(stats::parse_loadavg)
            .unwrap_or_else(|| "unavailable".to_owned())
    }

    /// The target summaries for a build-mode run: one-file incremental build
    /// wall time and peak RSS, from the samples file. The edited build and the
    /// restored build are both one-file incremental builds; they are reported
    /// separately and pooled, and the pooled line is the one a reader should
    /// quote (N is twice the cycle count).
    fn report_builds(
        samples: &Samples,
        cold: rss::Measured,
        load_start: &str,
        out: &mut dyn Write,
    ) {
        let rows = match samples.read() {
            Ok(rows) => rows,
            Err(e) => {
                let _ = writeln!(
                    out,
                    "soak summary NOT RUN: the samples could not be read: {e}"
                );
                return;
            }
        };
        let restored: Vec<u64> = rows.iter().filter_map(|r| r.build_ms).collect();
        let dirty: Vec<u64> = rows.iter().filter_map(|r| r.dirty_build_ms).collect();
        let pooled: Vec<u64> = restored.iter().chain(&dirty).copied().collect();
        let restored_rss: Vec<u64> = rows
            .iter()
            .filter(|r| r.build_ms.is_some())
            .map(|r| r.rss)
            .collect();
        let dirty_rss: Vec<u64> = rows.iter().filter_map(|r| r.dirty_rss).collect();
        let pooled_rss: Vec<u64> = restored_rss.iter().chain(&dirty_rss).copied().collect();
        let _ = writeln!(
            out,
            "soak targets (wall time includes process start-up and the writer-lock wait; p95 is nearest-rank):"
        );
        let _ = writeln!(out, "  cold build wall {} ms", cold.wall.as_millis());
        let _ = writeln!(out, "  {}", stats::build_line("edited (one file)", &dirty));
        let _ = writeln!(
            out,
            "  {}",
            stats::build_line("restored (one file)", &restored)
        );
        let _ = writeln!(
            out,
            "  {}",
            stats::build_line("all one-file incremental", &pooled)
        );
        let _ = writeln!(out, "  {}", stats::rss_line("cold build", &[cold.peak]));
        let _ = writeln!(
            out,
            "  {}",
            stats::rss_line("incremental edited", &dirty_rss)
        );
        let _ = writeln!(
            out,
            "  {}",
            stats::rss_line("incremental restored", &restored_rss)
        );
        let _ = writeln!(out, "  {}", stats::rss_line("all incremental", &pooled_rss));
        let _ = writeln!(
            out,
            "  load average (1/5/15 min): start {load_start}, end {}",
            load_average()
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn drive_builds(
        cfg: &Config,
        time_flag: &str,
        corpus: &Corpus,
        cleanup: &Arc<Mutex<Cleanup>>,
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
            if append(cleanup, corpus, i).is_err() {
                let _ = writeln!(out, "SOAK FAIL: build (dirty) cycle {i}");
                return true;
            }
            let dirty = match rss::measure_timed(time_flag, timeout_log, devmap, &["build", "."]) {
                Ok(dirty) => dirty,
                Err(_) => {
                    let _ = writeln!(out, "SOAK FAIL: build (dirty) cycle {i}");
                    return true;
                }
            };
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
            if restore(cleanup, corpus).is_err() {
                let _ = writeln!(out, "SOAK FAIL: restore cycle {i}");
                return true;
            }
            let restored = match rss::measure_timed(time_flag, timeout_log, devmap, &["build", "."])
            {
                Ok(restored) => restored,
                Err(_) => {
                    let _ = writeln!(out, "SOAK FAIL: build (restored) cycle {i}");
                    return true;
                }
            };
            let rss = restored.peak;
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
            let sample = Sample {
                cycle: i,
                rss,
                db,
                build_ms: Some(restored.wall.as_millis() as u64),
                dirty_build_ms: Some(dirty.wall.as_millis() as u64),
                dirty_rss: Some(dirty.peak),
            };
            if samples.append(sample).is_err() {
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

    /// Hold the daemon to its disk-write budget: an idle stretch with no edits
    /// must write (almost) nothing, and no window of the whole run may average
    /// more than the sustained rate. Prints every number it judged; true when
    /// the daemon stayed within both.
    fn judge_disk_writes(
        sampler: diskio::Sampler,
        counters: &dyn Fn() -> Option<diskio::WriteCounters>,
        cycle_bytes: &[u64],
        idle: Duration,
        store_bytes: u64,
        out: &mut dyn Write,
    ) -> bool {
        let mut ok = true;
        if let Some(max) = cycle_bytes.iter().max() {
            let mean = cycle_bytes.iter().sum::<u64>() / cycle_bytes.len() as u64;
            let _ = writeln!(
                out,
                "daemon disk writes per edit cycle (two drains): mean {mean} max {max} bytes; \
                 store {store_bytes} bytes"
            );
        }
        let before = counters();
        std::thread::sleep(idle);
        let after = counters();
        match diskio::judge_idle(before, after) {
            diskio::IdleVerdict::Within(bytes) => {
                let _ = writeln!(
                    out,
                    "daemon idle disk writes: {bytes} bytes over {idle:?} (bound {} bytes)",
                    diskio::IDLE_MAX_BYTES
                );
            }
            diskio::IdleVerdict::Over(bytes) => {
                let _ = writeln!(
                    out,
                    "SOAK FAIL: the idle daemon wrote {bytes} bytes over {idle:?} with no edit to \
                     index, over the {} byte bound",
                    diskio::IDLE_MAX_BYTES
                );
                ok = false;
            }
            diskio::IdleVerdict::Unreadable => {
                let _ = writeln!(
                    out,
                    "SOAK FAIL: the daemon's disk-write counters could not be read across the \
                     idle stretch"
                );
                ok = false;
            }
        }
        let (readings, ended) = sampler.finish();
        if let Some(why) = ended {
            let _ = writeln!(out, "SOAK FAIL: disk-write sampling ended early: {why}");
            ok = false;
        }
        let budget = diskio::Budget::default();
        match diskio::judge(&readings, budget) {
            diskio::Verdict::Within(w) => {
                let _ = writeln!(
                    out,
                    "daemon disk-write rate: worst {:?} window {:.2} MiB/s, budget {} MiB/s",
                    budget.window,
                    diskio::mib_per_s(w.rate),
                    budget.rate >> 20
                );
            }
            diskio::Verdict::Over(w) => {
                let _ = writeln!(
                    out,
                    "SOAK FAIL: the daemon wrote {} bytes in {:.0} s ({:.2} MiB/s), over the {} \
                     MiB/s sustained budget",
                    w.bytes,
                    (w.to - w.from).as_secs_f64(),
                    diskio::mib_per_s(w.rate),
                    budget.rate >> 20
                );
                ok = false;
            }
            diskio::Verdict::NotRun(why) => {
                let _ = writeln!(out, "SOAK FAIL: disk-write rate NOT RUN: {why}");
                ok = false;
            }
        }
        ok
    }

    /// Pick the impact target and prove every query kind answers on the quiet
    /// store, so that an error under load is never a query that was always
    /// going to fail. `file::symbol` is the id the kernel treats as
    /// unambiguous; the candidates are search rows whose pair is unique.
    fn readers_preflight(
        devmap: &Path,
        store_path: &Path,
        scratch: &Path,
        tag: u32,
    ) -> Result<String, String> {
        let out = scratch.join(format!("devmap-soak-{tag}-probe.out"));
        let err = scratch.join(format!("devmap-soak-{tag}-probe.err"));
        let db = store_path.to_string_lossy().into_owned();
        let args = |rest: &[&str]| -> Vec<String> {
            ["--json", "--db", db.as_str()]
                .iter()
                .chain(rest)
                .map(|a| (*a).to_owned())
                .collect()
        };
        let result = (|| {
            let mut target = None;
            for term in ["build", "parse", "store", "config"] {
                let (outcome, _) = readers::run_query(devmap, &args(&["search", term]), &out, &err);
                if outcome != readers::Outcome::Ok {
                    return Err(format!(
                        "search {term:?} failed on the quiet store: {outcome:?}"
                    ));
                }
                let reply = std::fs::read_to_string(&out).unwrap_or_default();
                for candidate in readers::targets_from_search(&reply).into_iter().take(10) {
                    let (outcome, _) = readers::run_query(
                        devmap,
                        &args(&["impact", candidate.as_str()]),
                        &out,
                        &err,
                    );
                    if outcome == readers::Outcome::Ok {
                        target = Some(candidate);
                        break;
                    }
                }
                if target.is_some() {
                    break;
                }
            }
            let target = target.ok_or("no search row gave an impact target the kernel answers")?;
            let (outcome, _) = readers::run_query(
                devmap,
                &args(&["explore", "build", "--limit", "5"]),
                &out,
                &err,
            );
            if outcome != readers::Outcome::Ok {
                return Err(format!("explore failed on the quiet store: {outcome:?}"));
            }
            Ok(target)
        })();
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&err);
        result
    }

    /// The writer's failed build: SQLite's busy report, or any other error.
    fn writer_failure(timeout_log: &Path, tally: &mut WriterTally, what: &str) -> String {
        let text = std::fs::read_to_string(timeout_log).unwrap_or_default();
        if readers::is_busy(&text) {
            tally.busy += 1;
        }
        let first = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("(no output)");
        format!("{what}: {first}")
    }

    /// One writer cycling edit -> incremental build -> restore -> incremental
    /// build for `duration_secs` while `readers` threads query the same store.
    /// Returns true when the run failed. See [`devmap_soak::readers`] for what
    /// the readers do and what fails the verdict.
    #[allow(clippy::too_many_arguments)]
    fn drive_readers(
        cfg: &Config,
        time_flag: &str,
        corpus: &Corpus,
        cleanup: &Arc<Mutex<Cleanup>>,
        store_path: &Path,
        base_digest: &str,
        samples: &Samples,
        timeout_log: &Path,
        readers: u64,
        duration_secs: u64,
        cold: rss::Measured,
        load_start: &str,
        out: &mut dyn Write,
    ) -> bool {
        let devmap = cfg.devmap.as_path();
        let scratch = std::env::temp_dir();
        let tag = std::process::id();
        let impact_target = match readers_preflight(devmap, store_path, &scratch, tag) {
            Ok(target) => target,
            Err(why) => {
                let _ = writeln!(out, "SOAK FAIL: readers preflight: {why}");
                return true;
            }
        };
        let _ = writeln!(
            out,
            "soak readers: {readers} reader threads, {duration_secs} s, one `devmap --json --db <store>` subprocess per query (search/impact/explore, impact target {impact_target}), {} s per-query timeout",
            readers::QUERY_TIMEOUT.as_secs()
        );
        let setup = readers::Setup {
            devmap,
            db: store_path,
            impact_target: &impact_target,
            scratch_dir: &scratch,
            tag,
        };
        let stop = AtomicBool::new(false);
        let building = AtomicBool::new(false);
        let started = Instant::now();
        let mut tally = WriterTally::default();
        let mut failure: Option<String> = None;
        let (mut edited_rss, mut restored_rss) = (Vec::new(), Vec::new());
        let reader_stats: Vec<ReaderStats> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..readers as usize)
                .map(|id| {
                    let (setup, stop, building) = (&setup, &stop, &building);
                    scope.spawn(move || readers::reader_loop(id, setup, stop, building))
                })
                .collect();
            let timed = |building: &AtomicBool| {
                building.store(true, Ordering::Release);
                let measured = rss::measure_timed(time_flag, timeout_log, devmap, &["build", "."]);
                building.store(false, Ordering::Release);
                measured
            };
            let mut cycle = 0u64;
            while started.elapsed() < Duration::from_secs(duration_secs) {
                cycle += 1;
                if append(cleanup, corpus, cycle).is_err() {
                    failure = Some(format!("cannot edit the corpus at cycle {cycle}"));
                    break;
                }
                let dirty = match timed(&building) {
                    Ok(m) => m,
                    Err(_) => {
                        failure = Some(writer_failure(
                            timeout_log,
                            &mut tally,
                            &format!("build (edited) cycle {cycle}"),
                        ));
                        break;
                    }
                };
                if restore(cleanup, corpus).is_err() {
                    failure = Some(format!("cannot restore the corpus at cycle {cycle}"));
                    break;
                }
                let restored = match timed(&building) {
                    Ok(m) => m,
                    Err(_) => {
                        failure = Some(writer_failure(
                            timeout_log,
                            &mut tally,
                            &format!("build (restored) cycle {cycle}"),
                        ));
                        break;
                    }
                };
                let (dirty_ms, restored_ms) = (
                    dirty.wall.as_millis() as u64,
                    restored.wall.as_millis() as u64,
                );
                tally.builds_ms.extend([dirty_ms, restored_ms]);
                edited_rss.push(dirty.peak);
                restored_rss.push(restored.peak);
                tally.cycles += 1;
                let sample = Sample {
                    cycle,
                    rss: restored.peak,
                    db: store::bytes(store_path),
                    build_ms: Some(restored_ms),
                    dirty_build_ms: Some(dirty_ms),
                    dirty_rss: Some(dirty.peak),
                };
                if samples.append(sample).is_err() {
                    failure = Some(format!("cannot append CSV {}", samples.path().display()));
                    break;
                }
            }
            stop.store(true, Ordering::Release);
            // Each reader's last query is bounded by the per-query timeout, so
            // this join is too.
            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| ReaderStats {
                        other: 1,
                        first_other: Some("reader thread panicked".into()),
                        ..ReaderStats::default()
                    })
                })
                .collect()
        });
        let ran = started.elapsed();
        let _ = writeln!(
            out,
            "readers ran {:.1} s of the requested {duration_secs} s (the last writer cycle always finishes); writer completed {} cycles ({} builds)",
            ran.as_secs_f64(),
            tally.cycles,
            tally.builds_ms.len()
        );
        for (i, r) in reader_stats.iter().enumerate() {
            let _ = writeln!(
                out,
                "  reader {i}: ok {} busy {} other {} (timeouts {}) overlapped-a-build {}",
                r.ok, r.busy, r.other, r.timeouts, r.overlapped
            );
        }
        let total: u64 = reader_stats.iter().map(ReaderStats::total).sum();
        let sum = |f: fn(&ReaderStats) -> u64| reader_stats.iter().map(f).sum::<u64>();
        let latencies: Vec<u64> = reader_stats
            .iter()
            .flat_map(|r| r.latencies_ms.iter().copied())
            .collect();
        let _ = writeln!(
            out,
            "readers total: {total} queries ({:.1}/s), ok {}, SQLITE_BUSY/locked {}, other errors {}, overlapped a build {}",
            total as f64 / ran.as_secs_f64().max(0.001),
            sum(|r| r.ok),
            sum(|r| r.busy),
            sum(|r| r.other),
            sum(|r| r.overlapped)
        );
        if let Some(s) = stats::summarize(&latencies) {
            let _ = writeln!(
                out,
                "reader query latency ms: N={} min {} median {} p95 {} max {}",
                s.n, s.min, s.median, s.p95, s.max
            );
        }
        let _ = writeln!(
            out,
            "  {}",
            stats::build_line("under 1 writer + readers", &tally.builds_ms)
        );
        let _ = writeln!(
            out,
            "  {}",
            stats::rss_line("incremental edited under readers", &edited_rss)
        );
        let _ = writeln!(
            out,
            "  {}",
            stats::rss_line("incremental restored under readers", &restored_rss)
        );
        let _ = writeln!(out, "  {}", stats::rss_line("cold build", &[cold.peak]));
        let _ = writeln!(
            out,
            "  load average (1/5/15 min): start {load_start}, end {}",
            load_average()
        );
        let mut fails = readers::failures(&reader_stats, &tally);
        if let Some(why) = failure {
            fails.push(why);
        }
        match store::digest(store_path) {
            Ok(d) if d == base_digest => {}
            Ok(_) => fails.push("the graph differs from the baseline after the restore".into()),
            Err(e) => fails.push(format!("the graph could not be measured: {e}")),
        }
        if fails.is_empty() {
            let _ = writeln!(
                out,
                "SOAK READERS OK ({readers} readers, {:.0} s, {total} queries, 0 SQLITE_BUSY, 0 other reader errors, {} writer cycles); samples in {}",
                ran.as_secs_f64(),
                tally.cycles,
                samples.path().display()
            );
            false
        } else {
            for line in &fails {
                let _ = writeln!(out, "SOAK FAIL: readers: {line}");
            }
            true
        }
    }

    fn lock(cleanup: &Arc<Mutex<Cleanup>>) -> std::sync::MutexGuard<'_, Cleanup> {
        cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // Corpus edits happen under the cleanup lock. An interrupt's cleanup takes
    // the same lock, restores the corpus and exits still holding it, so no
    // edit can land after the restore that was meant to undo it.
    fn append(cleanup: &Arc<Mutex<Cleanup>>, corpus: &Corpus, cycle: u64) -> io::Result<()> {
        let _held = lock(cleanup);
        corpus.append_cycle(cycle)
    }

    fn restore(cleanup: &Arc<Mutex<Cleanup>>, corpus: &Corpus) -> io::Result<()> {
        let _held = lock(cleanup);
        corpus.restore()
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

        // The daemon's disk writes, sampled for the whole run. On a platform
        // with a counter, a sampler that cannot start is a failed soak; only a
        // platform with none skips the budget, and says so.
        let disk = match diskio::Sampler::start(serve_pid) {
            Ok(sampler) => Some(sampler),
            Err(why) if diskio::PLATFORM_SAMPLES => {
                let _ = writeln!(out, "SOAK FAIL: {why}");
                return Err(1);
            }
            Err(why) => {
                let _ = writeln!(out, "SOAK DISK-WRITES NOT RUN: {why}");
                None
            }
        };
        let counters = || dc_proc::diskio::write_counters(serve_pid).ok();
        let mut cycle_bytes: Vec<u64> = Vec::new();

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
            let cycle_start = counters();
            // An OK envelope does not prove the edit was indexed. Keep the
            // edited bytes in place until the exact symbol/file and freshness
            // are observed.
            if append(cleanup, corpus, i).is_err()
                || !await_symbol(&name, Mode::Present, "present", out)
            {
                let _ = writeln!(out, "SOAK FAIL: inserted symbol cycle {i}");
                fails = true;
                break;
            }
            if ask(&corpus.impact_frame(i), Mode::Validate, "") != Observation::Observed {
                let _ = writeln!(out, "SOAK FAIL: impact cycle {i}");
                fails = true;
                break;
            }
            if restore(cleanup, corpus).is_err()
                || !await_symbol(&name, Mode::Absent, "absent", out)
            {
                let _ = writeln!(out, "SOAK FAIL: removed symbol cycle {i}");
                fails = true;
                break;
            }
            if let (Some(from), Some(to)) = (cycle_start, counters()) {
                cycle_bytes.push(to.logical_bytes.saturating_sub(from.logical_bytes));
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
                    ..Sample::default()
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
        if let Some(sampler) = disk {
            fails |= !judge_disk_writes(
                sampler,
                &counters,
                &cycle_bytes,
                cfg.idle,
                store::bytes(store_path),
                out,
            );
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
