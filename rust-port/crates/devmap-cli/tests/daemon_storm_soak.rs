//! Storm, kill, restart — and then check the store against a cold build.
//!
//! The existing daemon coverage proves single steps: a drain completes, an
//! endpoint is reclaimed, a generation is written. This proves the *composition*
//! survives repetition, which is the only thing that catches the failures that
//! need a sequence — a queue that grows a little each cycle, a generation
//! written half-way, an endpoint reclaimed 49 times out of 50.
//!
//! Three assertions, and the first is the one the others exist to support:
//!
//! 1. **The store converges on a cold build.** After every storm and every
//!    abrupt death, a daemon left to quiesce must arrive at exactly the graph a
//!    `devmap build` of the same tree produces — same node and edge counts, and
//!    the same symbols by name and file. Counts alone are not enough: two
//!    different graphs of equal size are a thing this kernel can produce, and
//!    "the numbers matched" would report them as agreement.
//! 2. **RSS plateaus.** Compared as two half-means after a warm-up quarter,
//!    which is `tools/soak.sh`'s method and for its reason: the early cycles are
//!    a process loading its graph, and a soak that called that growth a leak
//!    would fail on every healthy kernel.
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
/// A count is a summary, and two different graphs can share one — so the
/// comparison is on membership, not on a statistic about it.
///
/// `ast` is budgeted like every other answer here, and its default page is 100.
/// A digest built from that page would be a capped sample presented as a whole
/// graph, which is the one thing this file must not do, so the limit is raised
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
        "the digest must be the whole graph and not a page of it: {total} symbols \
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
    let churn = tree.join("churn");
    match cycle % 4 {
        // Ten thousand creations in one directory, as fast as the filesystem
        // will take them.
        0 => {
            let _ = std::fs::create_dir_all(&churn);
            for i in 0..10_000 {
                let _ = std::fs::write(
                    churn.join(format!("burst_{i}.py")),
                    format!("def burst_{i}():\n    return {i}\n"),
                );
            }
            "10,000 creations in one directory"
        }
        // Rename every one of them. No content changes at all, so a watcher
        // that keys on content sees nothing while every path it knows is wrong.
        1 => {
            if let Ok(entries) = std::fs::read_dir(&churn) {
                for entry in entries.flatten() {
                    let from = entry.path();
                    let Some(name) = from.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    let _ = std::fs::rename(&from, churn.join(format!("moved_{name}")));
                }
            }
            "mass rename of the whole directory"
        }
        // Delete the directory and recreate it with different content. Every
        // path that existed is gone and every path that exists is new.
        2 => {
            let _ = std::fs::remove_dir_all(&churn);
            let _ = std::fs::create_dir_all(&churn);
            for i in 0..500 {
                let _ = std::fs::write(
                    churn.join(format!("reborn_{i}.py")),
                    format!("def reborn_{i}(v):\n    return v * {i}\n"),
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
            let _ = std::fs::create_dir_all(&churn);
            let _ = std::fs::write(&a, "def cycled():\n    return 1\n");
            for _ in 0..1_000 {
                let _ = std::fs::rename(&a, &b);
                let _ = std::fs::rename(&b, &c);
                let _ = std::fs::rename(&c, &a);
            }
            "1,000 a->b->c->a rename cycles"
        }
    }
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

    // 1. Convergence. A fresh cold build of the same tree, into its own store.
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
         maintained store must hold the same graph a single build produces"
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

    // 2. Plateau, by `tools/soak.sh`'s method: two half-means after a warm-up
    //    quarter, because the early cycles are a process loading its graph.
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
             window ({a:.0} -> {b:.0} KiB); a daemon an agent host keeps open for \
             hours must plateau",
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
