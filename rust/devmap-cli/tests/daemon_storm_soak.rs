//! Storm, kill, restart — and then check the store against a cold build.
//!
//! The existing daemon coverage proves single steps: a drain completes, an
//! endpoint is reclaimed, a generation is written. This proves the *composition*
//! survives repetition, which is the only thing that catches the failures that
//! need a sequence — a queue that grows a little each cycle, a generation
//! written half-way, an endpoint reclaimed 49 times out of 50.
//!
//! Three checks, with distinct scopes:
//!
//! 1. **Each restarted daemon empties its queue.** After all cycles, the final
//!    store is compared with a cold build for node/edge counts and symbol
//!    membership by file, name and kind. Exact edge tuples, confidence and
//!    resolution are not compared, and there is no per-cycle cold comparison.
//! 2. **RSS across restarted daemons stays within a tolerance.** Two half-means
//!    are compared after discarding a warm-up quarter. These are samples of
//!    separate processes handling different storm shapes, not a measurement
//!    of one continuously running daemon's memory growth.
//! 3. **Nothing is left behind.** No socket, no endpoint lock, after the last
//!    daemon exits — and, at every restart in between, the *next* daemon binds
//!    with no manual cleanup. `serve_stress.rs`'s kill test removes the stale
//!    socket itself before restarting (`let _ = std::fs::remove_file(&socket)`),
//!    so reclamation after an abrupt death is the one thing it cannot see.
//!
//! Ignored by default because it runs for minutes. It is meant to be run, and
//! the numbers reported; `DEVMAP_SOAK_CYCLES` sets the length.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// The binary under test, built by cargo for this integration test.
const DEVMAP: &str = env!("CARGO_BIN_EXE_devmap");

fn cycles() -> usize {
    std::env::var("DEVMAP_SOAK_CYCLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12)
}

/// A short directory under `/tmp`.
///
/// Not the system temp dir: on macOS `TMPDIR` is a ~50-byte per-user path, and
/// `UnixIpcServer::bind` refuses any socket path over 100 bytes — correctly, and
/// it is the portable `sockaddr_un` limit. A soak whose endpoint could not bind
/// would report the harness's mistake as the daemon's.
fn short_scratch(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = PathBuf::from("/tmp").join(format!("dm-soak-{tag}-{}-{stamp}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn devmap(args: &[&str]) -> std::process::Output {
    Command::new(DEVMAP)
        .args(args)
        .env("DEVMAP_AUTOSPAWN", "0")
        .output()
        .expect("devmap runs")
}

fn json(args: &[&str]) -> serde_json::Value {
    let out = devmap(args);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "devmap {args:?} did not answer JSON ({err}): stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// What a generation actually holds: every symbol, by file, name and kind.
///
/// Symbol membership supplements node/edge counts; this does not compare the
/// edge tuples or confidence values of two generations.
///
/// `ast` is budgeted like every other answer here, and its default page is 100.
/// A digest built from that page would be a capped sample presented as all
/// symbols, which is the one thing this file must not do, so the limit is raised
/// past the corpus and `hidden` is asserted to be zero. `corpus_symbols` is the
/// exact total the command reports, and it is what the limit is checked
/// against — not a number chosen here.
fn symbol_digest(db: &Path) -> BTreeSet<String> {
    let db = db.display().to_string();
    let value = json(&["--db", &db, "--json", "ast", "--limit", "1000000", ""]);
    let hidden = value["hidden"].as_i64().unwrap_or(-1);
    let total = value["corpus_symbols"].as_i64().unwrap_or(-1);
    assert_eq!(
        hidden, 0,
        "the digest must include all symbols and not a page of them: {total} symbols \
         in the corpus and {hidden} withheld. Raise the limit."
    );
    let matches = value["matches"]
        .as_array()
        .unwrap_or_else(|| panic!("ast --json must enumerate symbols, got: {value}"));
    let digest: BTreeSet<String> = matches
        .iter()
        .map(|item| {
            format!(
                "{}::{}::{}",
                item["path"].as_str().unwrap_or("?"),
                item["qualified_name"]
                    .as_str()
                    .or_else(|| item["name"].as_str())
                    .unwrap_or("?"),
                item["kind"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    assert!(
        !digest.is_empty(),
        "an empty digest would make every comparison below hold trivially"
    );
    digest
}

fn counts(db: &Path) -> (i64, i64, i64) {
    let s = json(&["--db", &db.display().to_string(), "--json", "status"]);
    (
        s["node_count"].as_i64().unwrap_or(-1),
        s["edge_count"].as_i64().unwrap_or(-1),
        s["pending_count"].as_i64().unwrap_or(-1),
    )
}

fn seed(tree: &Path, count: usize) {
    for i in 0..count {
        std::fs::write(
            tree.join(format!("seed_{i}.py")),
            format!("def seed_{i}(x):\n    return helper_{i}(x)\n\n\ndef helper_{i}(x):\n    return x + {i}\n"),
        )
        .unwrap();
    }
}

/// One storm. Four shapes, rotated, each a different way for a watcher to lose
/// track of what the tree contains.
fn storm(tree: &Path, cycle: usize) -> &'static str {
    fn checked<T>(result: std::io::Result<T>, cycle: usize, operation: &str, path: &Path) -> T {
        result.unwrap_or_else(|error| {
            panic!(
                "storm cycle {cycle}: {operation} at {} failed: {error}",
                path.display()
            )
        })
    }

    let churn = tree.join("churn");
    match cycle % 4 {
        // Ten thousand creations in one directory, as fast as the filesystem
        // will take them.
        0 => {
            // Later rounds retain the previous reborn files and cycle_a.py.
            checked(
                std::fs::create_dir_all(&churn),
                cycle,
                "create directory",
                &churn,
            );
            for i in 0..10_000 {
                let path = churn.join(format!("burst_{i}.py"));
                checked(
                    std::fs::write(&path, format!("def burst_{i}():\n    return {i}\n")),
                    cycle,
                    "write burst source",
                    &path,
                );
            }
            "10,000 creations in one directory"
        }
        // Rename every one of them. No content changes at all, so a watcher
        // that keys on content sees nothing while every path it knows is wrong.
        1 => {
            let entries = checked(
                std::fs::read_dir(&churn),
                cycle,
                "enumerate churn directory",
                &churn,
            );
            // Finish enumeration before mutating the directory. A live
            // iterator can revisit renamed entries and rename them repeatedly.
            let entries = checked(
                entries.collect::<std::io::Result<Vec<_>>>(),
                cycle,
                "read churn directory entry",
                &churn,
            );
            for entry in entries {
                let from = entry.path();
                let mut name = std::ffi::OsString::from("moved_");
                name.push(entry.file_name());
                checked(
                    std::fs::rename(&from, churn.join(name)),
                    cycle,
                    "rename churn entry",
                    &from,
                );
            }
            "mass rename of the whole directory"
        }
        // Delete the directory and recreate it with different content. Every
        // path that existed is gone and every path that exists is new.
        2 => {
            checked(
                std::fs::remove_dir_all(&churn),
                cycle,
                "remove churn directory",
                &churn,
            );
            checked(
                std::fs::create_dir_all(&churn),
                cycle,
                "recreate churn directory",
                &churn,
            );
            for i in 0..500 {
                let path = churn.join(format!("reborn_{i}.py"));
                checked(
                    std::fs::write(&path, format!("def reborn_{i}(v):\n    return v * {i}\n")),
                    cycle,
                    "write reborn source",
                    &path,
                );
            }
            "directory deleted and recreated with different content"
        }
        // A rename cycle: a -> b -> c -> a, a thousand times. Every file ends
        // exactly where it started, so the tree is unchanged and the event
        // stream is anything but.
        _ => {
            let a = churn.join("cycle_a.py");
            let b = churn.join("cycle_b.py");
            let c = churn.join("cycle_c.py");
            checked(
                std::fs::create_dir_all(&churn),
                cycle,
                "create directory",
                &churn,
            );
            checked(
                std::fs::write(&a, "def cycled():\n    return 1\n"),
                cycle,
                "write rename-cycle source",
                &a,
            );
            for _ in 0..1_000 {
                checked(std::fs::rename(&a, &b), cycle, "rename a to b", &a);
                checked(std::fs::rename(&b, &c), cycle, "rename b to c", &b);
                checked(std::fs::rename(&c, &a), cycle, "rename c to a", &c);
            }
            "1,000 a->b->c->a rename cycles"
        }
    }
}

#[test]
fn every_storm_shape_refuses_a_file_in_place_of_its_directory() {
    let work = short_scratch("blocked-churn");
    std::fs::write(work.join("churn"), "directory creation is blocked\n").unwrap();
    for cycle in 0..4 {
        let attempted = std::panic::catch_unwind(|| storm(&work, cycle));
        assert!(
            attempted.is_err(),
            "storm shape {cycle} reported success although churn is a regular file"
        );
    }
    std::fs::remove_dir_all(work).unwrap();
}

#[test]
fn creation_storm_refuses_an_unwritable_file_destination() {
    let work = short_scratch("blocked-write");
    std::fs::create_dir_all(work.join("churn/burst_0.py")).unwrap();
    let attempted = std::panic::catch_unwind(|| storm(&work, 0));
    assert!(
        attempted.is_err(),
        "the creation storm reported success when its first destination was a directory"
    );
    std::fs::remove_dir_all(work).unwrap();
}

#[test]
fn rename_cycle_refuses_a_blocked_intermediate_destination() {
    let work = short_scratch("blocked-rename");
    std::fs::create_dir_all(work.join("churn/cycle_b.py")).unwrap();
    std::fs::write(
        work.join("churn/cycle_b.py/marker"),
        "cannot replace this directory\n",
    )
    .unwrap();
    let attempted = std::panic::catch_unwind(|| storm(&work, 3));
    assert!(
        attempted.is_err(),
        "the rename storm reported success when the a-to-b rename could not run"
    );
    std::fs::remove_dir_all(work).unwrap();
}

#[test]
fn storm_shapes_produce_the_requested_files_in_first_and_later_rounds() {
    let work = short_scratch("shape-controls");
    let churn = work.join("churn");
    for cycle in 0..8 {
        storm(&work, cycle);
        let names: BTreeSet<_> = std::fs::read_dir(&churn)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                assert!(entry.file_type().unwrap().is_file());
                entry.file_name().into_string().unwrap()
            })
            .collect();
        match cycle % 4 {
            0 | 1 => {
                let prefix = if cycle % 4 == 0 { "" } else { "moved_" };
                assert_eq!(names.len(), if cycle < 4 { 10_000 } else { 10_501 });
                for index in 0..10_000 {
                    assert!(names.contains(&format!("{prefix}burst_{index}.py")));
                }
                if cycle >= 4 {
                    assert!(names.contains(&format!("{prefix}cycle_a.py")));
                    for index in 0..500 {
                        assert!(names.contains(&format!("{prefix}reborn_{index}.py")));
                    }
                }
            }
            2 => {
                assert_eq!(names.len(), 500);
                for index in 0..500 {
                    assert!(names.contains(&format!("reborn_{index}.py")));
                }
            }
            _ => {
                assert_eq!(names.len(), 501);
                assert!(names.contains("cycle_a.py"));
                assert!(!names.contains("cycle_b.py") && !names.contains("cycle_c.py"));
                assert_eq!(
                    std::fs::read_to_string(churn.join("cycle_a.py")).unwrap(),
                    "def cycled():\n    return 1\n"
                );
            }
        }
    }
    std::fs::remove_dir_all(work).unwrap();
}

struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    fn start(db: &Path, tree: &Path, socket: &Path) -> Self {
        let child = Command::new(DEVMAP)
            .args([
                "--db",
                &db.display().to_string(),
                "serve",
                &tree.display().to_string(),
                "--socket",
                &socket.display().to_string(),
            ])
            .env("DEVMAP_AUTOSPAWN", "0")
            // Retirement off: this soak decides when a daemon ends, and an idle
            // bound firing mid-cycle would be measured as a crash.
            .env("DEVMAP_MAX_IDLE_SECS", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("daemon spawns");
        Self {
            child,
            socket: socket.to_path_buf(),
        }
    }

    /// Wait for the endpoint, and for the process to still be alive holding it.
    fn wait_until_bound(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return false; // exited instead of binding
            }
            if self.socket.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn rss_kib(&self) -> u64 {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &self.child.id().to_string()])
            .output()
            .expect("ps runs");
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0)
    }

    fn kill9(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn terminate(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.kill9();
    }
}

/// Poll until the queue is empty, or say it never was.
fn wait_for_quiescence(db: &Path, limit: Duration) -> Option<(i64, i64)> {
    let deadline = Instant::now() + limit;
    let mut last = (-1, -1, -1);
    while Instant::now() < deadline {
        last = counts(db);
        if last.2 == 0 {
            // Two consecutive empties: one is a window between a drain claiming
            // a batch and the watcher enqueueing the next.
            std::thread::sleep(Duration::from_millis(700));
            let again = counts(db);
            if again.2 == 0 {
                return Some((again.0, again.1));
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    eprintln!("  never quiesced; last counts were {last:?}");
    None
}

fn mean(values: &[u64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<u64>() as f64 / values.len() as f64
}

#[test]
#[ignore = "minutes-long soak; run explicitly and report the numbers"]
fn a_storm_kill_restart_cycle_converges_on_the_cold_build() {
    let cycles = cycles();
    let work = short_scratch("work");
    let tree = work.join("tree");
    std::fs::create_dir_all(&tree).unwrap();
    let db = work.join("devmap.sqlite");
    let socket = work.join("i.sock");
    assert!(
        socket.display().to_string().len() <= 100,
        "fixture: the endpoint must be bindable ({} bytes)",
        socket.display().to_string().len()
    );

    seed(&tree, 200);
    let build = devmap(&[
        "--db",
        &db.display().to_string(),
        "build",
        &tree.display().to_string(),
    ]);
    assert!(build.status.success(), "cold build must succeed");

    let mut rss_samples: Vec<u64> = Vec::new();
    let mut reclaim_failures: Vec<usize> = Vec::new();

    for cycle in 0..cycles {
        let shape = storm(&tree, cycle);

        // A daemon that dies abruptly, at a point that moves every cycle so the
        // kill lands in a different part of the drain.
        let mut victim = Daemon::start(&db, &tree, &socket);
        if victim.wait_until_bound(Duration::from_secs(30)) {
            let jitter = 60 + (cycle as u64 * 137) % 1_400;
            std::thread::sleep(Duration::from_millis(jitter));
            victim.kill9();
        } else {
            victim.kill9();
            panic!("cycle {cycle}: the daemon never bound its endpoint");
        }

        // THE ASSERTION the existing kill test cannot make: no cleanup here.
        // Whatever the abrupt death left — the socket file, the endpoint lock —
        // the next daemon has to reclaim by itself.
        let mut successor = Daemon::start(&db, &tree, &socket);
        if !successor.wait_until_bound(Duration::from_secs(45)) {
            reclaim_failures.push(cycle);
            successor.kill9();
            continue;
        }

        let quiesced = wait_for_quiescence(&db, Duration::from_secs(180));
        rss_samples.push(successor.rss_kib());
        eprintln!(
            "  cycle {cycle:>2} [{shape}] rss={:>7} KiB counts={:?}",
            rss_samples[rss_samples.len() - 1],
            quiesced
        );
        successor.terminate();

        assert!(
            quiesced.is_some(),
            "cycle {cycle}: the queue never emptied after {shape}; a daemon that \
             cannot finish a storm is one whose `status` is permanently stale"
        );
    }

    assert!(
        reclaim_failures.is_empty(),
        "a daemon killed with SIGKILL must leave an endpoint the next one can \
         reclaim without manual cleanup; it failed on cycles {reclaim_failures:?} \
         of {cycles}"
    );

    // 1. Final counts and symbol membership against a fresh cold build.
    let cold_db = work.join("cold.sqlite");
    let cold = devmap(&[
        "--db",
        &cold_db.display().to_string(),
        "build",
        &tree.display().to_string(),
    ]);
    assert!(
        cold.status.success(),
        "the reference cold build must succeed"
    );

    let (soaked_nodes, soaked_edges, soaked_pending) = counts(&db);
    let (cold_nodes, cold_edges, _) = counts(&cold_db);
    assert_eq!(
        soaked_pending, 0,
        "the soaked store must have drained everything before it is compared"
    );
    assert_eq!(
        (soaked_nodes, soaked_edges),
        (cold_nodes, cold_edges),
        "after {cycles} storms and {cycles} abrupt deaths the incrementally \
         maintained store must match a cold build's node and edge counts"
    );

    let soaked = symbol_digest(&db);
    let reference = symbol_digest(&cold_db);
    if soaked != reference {
        let missing: Vec<_> = reference.difference(&soaked).take(10).collect();
        let extra: Vec<_> = soaked.difference(&reference).take(10).collect();
        panic!(
            "the soaked graph and the cold build hold different symbols, though \
             their counts agree — equal sizes are not equal graphs.\n  missing \
             (up to 10 of {}): {missing:?}\n  extra (up to 10 of {}): {extra:?}",
            reference.difference(&soaked).count(),
            soaked.difference(&reference).count()
        );
    }

    // 2. Compare RSS from restarted daemons across the storm shapes. This
    //    does not establish a continuously running daemon's memory plateau.
    let warmup = rss_samples.len() / 4;
    let measured = &rss_samples[warmup..];
    if measured.len() >= 4 {
        let (first, second) = measured.split_at(measured.len() / 2);
        let (a, b) = (mean(first), mean(second));
        let growth = if a > 0.0 { (b - a) / a } else { 0.0 };
        eprintln!(
            "  RSS half-means after a {warmup}-cycle warm-up: {a:.0} KiB -> {b:.0} KiB \
             ({:+.1}%)",
            growth * 100.0
        );
        assert!(
            growth < 0.25,
            "resident memory grew {:.1}% between the halves of the measured \
             window ({a:.0} -> {b:.0} KiB) across restarted daemon processes",
            growth * 100.0
        );
    } else {
        // Never silently: a check that could not run must not read as one that ran.
        eprintln!(
            "  RSS plateau NOT ASSERTED: {} samples after the warm-up is too few to \
             compare halves (need 4). Raise DEVMAP_SOAK_CYCLES.",
            measured.len()
        );
    }

    // 3. Nothing left behind.
    let lock = work.join("i.sock.lock");
    assert!(
        !socket.exists(),
        "the last daemon exited cleanly and must have removed its socket"
    );
    assert!(
        !lock.exists(),
        "and its endpoint lock: a lock file outlasting its owner is what makes \
         the next start wait out a liveness probe for a daemon that is gone"
    );

    let _ = std::fs::remove_dir_all(&work);
}
