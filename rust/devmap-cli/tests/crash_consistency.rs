//! Crash consistency of the store: a build killed mid-write, and a lost WAL tail.
//!
//! Two different failures, two different instruments.
//!
//! - **Process death** (`a_build_killed_mid_persist_leaves_a_whole_generation`):
//!   a real `devmap build` is sent `SIGKILL` after it announces the persist
//!   stage. SQLite's WAL recovers from a process crash by construction; what
//!   this proves is that *DevMap's* write is one transaction — a reader after
//!   the kill sees the previous generation or the new one, never a generation
//!   missing files — and that the writer lock, the pending queue and the next
//!   build all recover without an operator.
//!
//! - **Power loss at an fsync boundary**
//!   (`a_lost_wal_tail_loses_whole_generations_never_half_of_one`): the store
//!   runs `synchronous = NORMAL` (see `Store::configure_connection`), which does
//!   not fsync per commit, so an OS crash or power loss can drop the most
//!   recent WAL frames. This is simulated by copying the database file and
//!   every prefix of the WAL — truncated at each frame boundary and at a
//!   mid-frame offset — and opening each copy.
//!
//! What the simulation does **not** cover, and so what is not claimed: torn or
//! partially written pages in the main database file, writes reordered by the
//! device or the filesystem, a lying disk cache, and a crash during a
//! checkpoint (which writes the main file). Those need fault injection below
//! the filesystem — a VFS shim or a block-device simulator — which this suite
//! does not have. It is also one filesystem (whatever `temp_dir()` is on) and
//! one operating system per run.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use devmap_analyze::analyze;
use devmap_extract::{extract_file, Extraction};
use devmap_resolve::Resolver;
use devmap_store::Store;

/// One directory per call: pid and a process-wide counter, never a timestamp
/// alone (parallel tests on macOS land in the same `SystemTime` tick).
fn temp_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-crash-{name}-{}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create fixture root");
    root
}

/// `src/m{i}.py` calls into `src/m{(i + shift) % n}.py`. Changing `shift`
/// rewrites every file and every edge, so the next build's persist stage closes
/// and opens a whole generation's worth of rows.
fn write_corpus(root: &Path, n: usize, shift: usize) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    for i in 0..n {
        let next = (i + shift) % n;
        fs::write(
            src.join(format!("m{i}.py")),
            format!(
                "from src.m{next} import f{next}\n\n\
                 def f{i}():\n    return f{next}()\n\n\
                 def g{i}_{shift}(x):\n    return [f{next}() for _ in range(x)]\n"
            ),
        )
        .unwrap();
    }
}

/// Wait for `child` to exit, bounded. A wait that cannot end is not a test.
fn wait_bounded(child: &mut Child, limit: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn build(db: &Path, root: &Path, full: bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
    command
        .arg("--db")
        .arg(db)
        .args(["--json", "build", "--progress", "never"]);
    if full {
        command.arg("--full");
    }
    let mut child = command
        .arg(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn build");
    let status = wait_bounded(&mut child, Duration::from_secs(300));
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    assert!(status.success(), "build failed: {status}\n{stderr}");
}

/// `PRAGMA integrity_check` on a store no live `Store` in this process holds.
///
/// Opened only after every `Store` on the file is dropped: closing a second
/// connection on a file this process already has open releases the first
/// connection's POSIX locks.
fn integrity(db: &Path) -> String {
    let conn = rusqlite::Connection::open(db).expect("open for integrity_check");
    conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("integrity_check runs")
}

/// What the latest generation answers, in a form two stores can be compared by:
/// every node and every edge, keyed by path and symbol, sorted.
fn latest_answers(db: &Path) -> (Vec<String>, Vec<String>) {
    let conn = rusqlite::Connection::open(db).expect("open for comparison");
    let latest: i64 = conn
        .query_row("SELECT MAX(id) FROM generations", [], |row| row.get(0))
        .unwrap();
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let mut out: Vec<String> = stmt
            .query_map([latest], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        out.sort();
        out
    };
    let nodes = rows(
        "SELECT p.path || '|' || n.qualified_name || '|' || n.kind
           FROM generation_nodes n JOIN paths p ON p.id = n.file_id
          WHERE n.generation_id = ?1",
    );
    let edges = rows(
        "SELECT s.path || '|' || e.source_symbol || '->' || t.path || '|' || e.target_symbol
                || '|' || e.edge_kind
           FROM generation_edges e
           JOIN paths s ON s.id = e.source_file_id
           JOIN paths t ON t.id = e.target_file_id
          WHERE e.generation_id = ?1",
    );
    (nodes, edges)
}

/// The store after a crash: it opens, reports one of the `allowed` generations,
/// and that generation holds every file of the corpus.
fn assert_whole_generation(db: &Path, n: usize, allowed: &[u32]) -> u32 {
    let generation = {
        let store = Store::open(db).expect("store opens after the crash");
        let generation = store
            .latest_generation_id()
            .unwrap()
            .expect("a generation survives");
        assert!(
            allowed.contains(&generation),
            "latest generation {generation} is not one of {allowed:?}"
        );
        let members = store.list_generation_paths(generation).unwrap();
        let sources = members.iter().filter(|p| p.starts_with("src/")).count();
        assert_eq!(
            sources, n,
            "generation {generation} holds {sources} of {n} source files"
        );
        generation
    };
    assert_eq!(integrity(db), "ok");
    generation
}

#[cfg(unix)]
#[test]
fn a_build_killed_mid_persist_leaves_a_whole_generation() {
    const FILES: usize = 1200;
    // Delays after the persist stage is announced. Zero lands at the start of
    // the write; the larger ones walk through it, and the last can land after
    // the commit — both outcomes are legal, and each is recorded.
    const DELAYS_MS: &[u64] = &[0, 2, 5, 10, 20, 40, 80, 160];

    let root = temp_root("killed-build");
    let db = root.join("state").join("devmap.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    write_corpus(&root, FILES, 1);
    build(&db, &root, false);
    let mut generation = assert_whole_generation(&db, FILES, &[1]);

    let mut landed_in_persist = 0usize;
    let mut outcomes = Vec::new();
    for (round, delay) in DELAYS_MS.iter().enumerate() {
        write_corpus(&root, FILES, round + 2);
        let mut child = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .arg("--db")
            .arg(&db)
            .args(["build", "--progress", "always"])
            .arg(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the build to kill");
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                let persisting = line.starts_with("[4/5] persisting");
                if tx.send((persisting, line)).is_err() || persisting {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut announced = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok((true, _)) => {
                    announced = true;
                    break;
                }
                Ok((false, _)) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if child.try_wait().unwrap().is_some() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(announced, "round {round}: the build never announced persistence");
        std::thread::sleep(Duration::from_millis(*delay));
        child.kill().expect("SIGKILL the build");
        let status = wait_bounded(&mut child, Duration::from_secs(10));
        // Dead, and by the signal rather than by finishing first.
        use std::os::unix::process::ExitStatusExt;
        let killed = status.signal() == Some(libc::SIGKILL);
        drop(rx);
        reader.join().unwrap();

        let after = assert_whole_generation(&db, FILES, &[generation, generation + 1]);
        let mid_write = killed && after == generation;
        landed_in_persist += usize::from(mid_write);
        outcomes.push((*delay, killed, mid_write));

        // The next build needs no operator: the writer lock died with the
        // process and the store accepts a writer.
        build(&db, &root, false);
        generation = assert_whole_generation(&db, FILES, &[after + 1]);
    }
    eprintln!("kill outcomes (delay_ms, killed_by_signal, landed_before_commit): {outcomes:?}");
    assert!(
        landed_in_persist > 0,
        "no kill landed between the persist announcement and the commit: {outcomes:?}"
    );

    // The recovered store answers what a cold build of the same tree answers.
    let cold = root.join("cold").join("devmap.sqlite");
    fs::create_dir_all(cold.parent().unwrap()).unwrap();
    build(&cold, &root, true);
    let recovered = latest_answers(&db);
    let fresh = latest_answers(&cold);
    assert!(!fresh.1.is_empty(), "the comparison must have edges to compare");
    assert_eq!(recovered.0, fresh.0, "nodes differ from a cold build");
    assert_eq!(recovered.1, fresh.1, "edges differ from a cold build");
    let _ = fs::remove_dir_all(&root);
}

fn generation_inputs(n: usize, shift: usize) -> Vec<Extraction> {
    (0..n)
        .map(|i| {
            let next = (i + shift) % n;
            extract_file(
                &format!("src/m{i}.py"),
                &format!(
                    "from src.m{next} import f{next}\n\ndef f{i}():\n    return f{next}()\n"
                ),
            )
        })
        .collect()
}

fn save(store: &Store, files: &[Extraction]) -> u32 {
    let mut resolver = Resolver::new();
    resolver.index_extractions(files);
    let resolution = resolver.resolve_all(files).unwrap();
    let analysis = analyze(files, &resolution);
    store.save_generation(files, &resolution, &analysis).unwrap()
}

const WAL_HEADER: usize = 32;
const FRAME_HEADER: usize = 24;

/// Byte offsets just past each WAL frame, with whether that frame commits a
/// transaction (a non-zero "database size after commit" field).
fn wal_frames(wal: &[u8]) -> (usize, Vec<(usize, bool)>) {
    assert!(wal.len() >= WAL_HEADER, "WAL shorter than its header");
    let page_size = u32::from_be_bytes(wal[8..12].try_into().unwrap()) as usize;
    let frame = FRAME_HEADER + page_size;
    let mut frames = Vec::new();
    let mut offset = WAL_HEADER;
    while offset + frame <= wal.len() {
        let commit = u32::from_be_bytes(wal[offset + 4..offset + 8].try_into().unwrap()) != 0;
        offset += frame;
        frames.push((offset, commit));
    }
    (page_size, frames)
}

#[test]
fn a_lost_wal_tail_loses_whole_generations_never_half_of_one() {
    const FILES: usize = 150;
    let root = temp_root("wal-tail");
    let live = root.join("live");
    fs::create_dir_all(&live).unwrap();
    let db = live.join("devmap.sqlite");
    let wal_path = live.join("devmap.sqlite-wal");

    // Generation 1, then close: the last connection's close checkpoints it
    // into the main file, so the WAL the copies are cut from holds only
    // generation 2.
    {
        let store = Store::open(&db).unwrap();
        assert_eq!(save(&store, &generation_inputs(FILES, 1)), 1);
    }
    let checkpointed = fs::read(&db).unwrap();

    let store = Store::open(&db).unwrap();
    assert_eq!(save(&store, &generation_inputs(FILES, 2)), 2);
    // Copied while the writer is still open, so nothing is checkpointed: this
    // is what the disk holds if the machine stops now and the WAL's tail never
    // reached it. Plain file reads, not a second SQLite connection.
    let main = fs::read(&db).unwrap();
    let wal = fs::read(&wal_path).unwrap();
    drop(store);
    assert_eq!(
        main, checkpointed,
        "generation 2 must still be WAL-only; an autocheckpoint moved it into the main file"
    );
    let (page_size, frames) = wal_frames(&wal);
    let last_commit = frames
        .iter()
        .rposition(|(_, commit)| *commit)
        .expect("generation 2 committed at least one WAL frame");
    assert!(
        frames.len() >= 2,
        "too few frames ({}) to cut between",
        frames.len()
    );

    // Every frame boundary, the header alone, and a cut through the middle of
    // the committing frame.
    let mut cuts: Vec<(usize, &str)> = vec![(0, "no wal"), (WAL_HEADER, "header only")];
    cuts.extend(frames.iter().map(|(end, _)| (*end, "frame boundary")));
    let mid = frames[last_commit].0 - (FRAME_HEADER + page_size) / 2;
    cuts.push((mid, "mid commit frame"));

    let mut seen = std::collections::BTreeMap::new();
    for (index, (cut, what)) in cuts.iter().enumerate() {
        let copy = root.join(format!("cut-{index}"));
        fs::create_dir_all(&copy).unwrap();
        fs::write(copy.join("devmap.sqlite"), &main).unwrap();
        if *cut > 0 {
            fs::write(copy.join("devmap.sqlite-wal"), &wal[..*cut]).unwrap();
        }
        // A prefix reaching the end of the final commit frame must recover
        // generation 2; a prefix holding no commit frame must stay at 1.
        let commits_kept = frames
            .iter()
            .filter(|(end, commit)| *commit && end <= cut)
            .count();
        let complete = frames[last_commit].0 <= *cut;
        let allowed: &[u32] = if complete {
            &[2]
        } else if commits_kept == 0 {
            &[1]
        } else {
            &[1, 2]
        };
        let db_copy = copy.join("devmap.sqlite");
        let generation = std::panic::catch_unwind(|| {
            assert_whole_generation(&db_copy, FILES, allowed)
        })
        .unwrap_or_else(|_| panic!("cut {index} at byte {cut} ({what}) did not recover"));
        *seen.entry(generation).or_insert(0usize) += 1;
    }
    eprintln!(
        "WAL of {} bytes, {} frames of {page_size} B, {} cuts; generations recovered: {seen:?}",
        wal.len(),
        frames.len(),
        cuts.len()
    );
    // Both outcomes occurred, so the cuts straddled the commit.
    assert!(seen.contains_key(&1) && seen.contains_key(&2), "{seen:?}");
    let _ = fs::remove_dir_all(&root);
}
