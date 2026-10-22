//! A watching daemon's query envelope tells the truth about freshness across
//! repeated edit -> rebuild -> query cycles.
//!
//! The envelope's `source_freshness.fresh` is `true`, `false` or `null`
//! (unverified). Two lies are possible and each needs a sequence to show:
//!
//! 1. **`true` while the tree differs from the stored generation.** The verdict
//!    is remembered per generation, so a verdict earned while the index was
//!    current can outlive the edit that ended it.
//! 2. **`false` right after a completed rebuild.** The verdict describes one
//!    generation; a rebuild moves the answer to the next one.
//!
//! The oracle is independent of the store's own verdict: it hashes every file
//! the walk admits and compares that with `latest_file_hashes`, bracketing each
//! query with the stored generation id so a rebuild landing mid-query makes the
//! sample inconclusive instead of a false alarm. A violation of (1) needs the
//! tree to differ before AND after the query under one unchanged generation.
//!
//! `null` is honest (the check did not run) and is counted, not failed, except
//! where the cycle has just asked the daemon for `status`, which verifies.
//!
//! Every wait is bounded; a bound that expires fails the test and names it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devmap_serve::Daemon;
use devmap_store::Store;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[allow(dead_code)] // only `scratch` and `json` are used here
#[path = "support_ignored/mod.rs"]
mod support_ignored;

const CYCLES: usize = 20;
const SETTLE_BOUND: Duration = Duration::from_secs(30);

async fn ipc(socket: &Path, mut request: Value) -> Value {
    request
        .as_object_mut()
        .unwrap()
        .insert("version".into(), json!(1));
    let exchange = async {
        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .expect("the daemon socket accepts a connection");
        let mut payload = serde_json::to_vec(&request).unwrap();
        payload.push(b'\n');
        stream.write_all(&payload).await.unwrap();
        let mut line = String::new();
        tokio::io::BufReader::new(&mut stream)
            .read_line(&mut line)
            .await
            .unwrap();
        serde_json::from_str::<Value>(&line)
            .unwrap_or_else(|error| panic!("envelope is not JSON ({error}): {line}"))
    };
    tokio::time::timeout(Duration::from_secs(30), exchange)
        .await
        .expect("an IPC exchange did not finish within 30 s")
}

/// The first `source_freshness` object in an envelope, as `(fresh, generation)`.
fn verdict(envelope: &Value) -> (Option<bool>, Option<u64>) {
    assert_eq!(envelope["ok"], json!(true), "the query failed: {envelope}");
    let found = ["/result/source_freshness", "/source_freshness"]
        .iter()
        .find_map(|pointer| envelope.pointer(pointer).filter(|v| v.is_object()))
        .unwrap_or_else(|| panic!("envelope carries no source_freshness: {envelope}"));
    (found["fresh"].as_bool(), found["generation_id"].as_u64())
}

/// Does the tree the walk sees differ from the latest stored generation?
fn tree_differs(root: &Path, reader: &Store) -> bool {
    let stored = reader.latest_file_hashes().unwrap();
    let walked = devmap_extract::collect_sources_with_report(root).unwrap().0;
    if walked.len() != stored.len() {
        return true;
    }
    walked.iter().any(|(path, _)| {
        let on_disk = std::fs::read_to_string(root.join(path))
            .map(|source| devmap_extract::content_hash(&source));
        on_disk.ok() != stored.get(path).copied()
    })
}

fn generation(reader: &Store) -> u64 {
    u64::from(reader.latest_generation_id().unwrap().unwrap_or(0))
}

#[derive(Default)]
struct Tally {
    racy_true: usize,
    racy_false: usize,
    racy_null: usize,
    racy_inconclusive: usize,
    settled_true: usize,
    settled_null: usize,
    violations: Vec<String>,
}

/// One edit shape per cycle: rewrite, add, delete, rewrite.
fn edit(root: &Path, cycle: usize) {
    let src = root.join("src");
    match cycle % 4 {
        0 | 3 => std::fs::write(
            src.join(format!("m{}.py", cycle % 3)),
            format!("def edited_{cycle}(x):\n    return x + {cycle}\n"),
        ),
        1 => std::fs::write(
            src.join(format!("added_{cycle}.py")),
            format!("def added_{cycle}(x):\n    return x * {cycle}\n"),
        ),
        _ => std::fs::remove_file(src.join("m2.py")).or_else(|_| {
            std::fs::write(src.join("m2.py"), format!("def back_{cycle}():\n    return 1\n"))
        }),
    }
    .unwrap_or_else(|error| panic!("cycle {cycle}: the edit failed: {error}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_envelope_never_lies_about_freshness_across_edit_rebuild_cycles() {
    let root = support_ignored::scratch("envelope-cycles");
    for index in 0..3 {
        std::fs::write(
            root.join(format!("src/m{index}.py")),
            format!("def base_{index}(x):\n    return x\n"),
        )
        .unwrap();
    }
    let build = support_ignored::json(&root, &["--json", "build", "."]);
    assert!(build["generation_id"].as_u64().is_some(), "{build}");

    let store_path = devmap_extract::paths::store_path(&root);
    let socket = PathBuf::from("/tmp").join(format!("devmap-fc-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let daemon = Daemon::new(Store::open(&store_path).unwrap(), root.clone())
        .with_store_path(store_path.clone())
        .with_ipc_path(socket.clone())
        .with_idle_poll(Duration::from_millis(20))
        .with_max_idle(None);
    let handle = daemon.clone();
    let mut running = tokio::spawn(async move { daemon.run_loop().await });
    tokio::time::timeout(Duration::from_secs(60), async {
        tokio::select! {
            biased;
            () = handle.wait_until_serving() => {}
            ended = &mut running => panic!("the daemon stopped before serving: {ended:?}"),
        }
    })
    .await
    .expect("run_loop neither served nor returned within 60 s");

    let reader = Store::open(&store_path).unwrap();
    let quiet = Instant::now();
    while reader.status("").unwrap().pending_count != 0 && quiet.elapsed() < Duration::from_secs(20)
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The oracle must be able to say "same" before it is trusted to say "differs".
    assert!(
        !tree_differs(&root, &reader),
        "precondition: the oracle sees the freshly built tree as differing, so it is not measuring \
         what the store stored"
    );

    let mut tally = Tally::default();
    for cycle in 0..CYCLES {
        // Quiescent: ask for a verified verdict, then query. `status` verifies,
        // so the next query may say `true`, and it is what makes (1) possible.
        let status = ipc(&socket, json!({"cmd": "status"})).await;
        assert_eq!(status["result"]["source_freshness"], json!(true), "cycle {cycle}: {status}");
        let (fresh, _) = verdict(&ipc(&socket, json!({"cmd": "search", "query": "base_0"})).await);
        if fresh != Some(true) {
            tally.violations.push(format!(
                "cycle {cycle}: after a verified `status` on an unchanged tree, search said {fresh:?}"
            ));
        }

        let before_generation = generation(&reader);
        edit(&root, cycle);
        let differs_before = tree_differs(&root, &reader);
        assert!(differs_before || generation(&reader) != before_generation, "cycle {cycle}: the edit changed nothing the oracle can see");

        // Racy query: the daemon may or may not have rebuilt yet.
        let query = if cycle % 2 == 0 {
            json!({"cmd": "search", "query": "base_0"})
        } else {
            json!({"cmd": "impact", "target": "base_1"})
        };
        // Sample once the daemon has noticed the edit (queued it or already
        // rebuilt). Before that the edit is invisible to everything but a walk
        // of the tree, which a query does not do; that window is the watcher's
        // debounce and is not what this test judges. Bounded.
        let noticed = Instant::now();
        while reader.status("").unwrap().pending_count == 0
            && generation(&reader) == before_generation
        {
            assert!(
                noticed.elapsed() < Duration::from_secs(20),
                "cycle {cycle}: the daemon never noticed the edit within 20 s"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let g0 = generation(&reader);
        let differs0 = tree_differs(&root, &reader);
        let envelope = ipc(&socket, query).await;
        let g1 = generation(&reader);
        let differs1 = tree_differs(&root, &reader);
        let (fresh, claimed) = verdict(&envelope);
        let stable = g0 == g1 && differs0 && differs1;
        match fresh {
            Some(true) => {
                tally.racy_true += 1;
                if stable && claimed == Some(g0) {
                    tally.violations.push(format!(
                        "cycle {cycle}: fresh:true for generation {g0} while the tree differed from it before and after the query: {envelope}"
                    ));
                }
            }
            Some(false) => tally.racy_false += 1,
            None => tally.racy_null += 1,
        }
        if !stable {
            tally.racy_inconclusive += 1;
        }

        // Wait for the rebuild: the tree matches the stored generation and the
        // queue is empty. Bounded.
        let waited = Instant::now();
        loop {
            if !tree_differs(&root, &reader) && reader.status("").unwrap().pending_count == 0 {
                break;
            }
            assert!(
                waited.elapsed() < SETTLE_BOUND,
                "cycle {cycle}: the daemon did not rebuild within {SETTLE_BOUND:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Settled: the verdict must not be `false`, for either query shape.
        for query in [
            json!({"cmd": "search", "query": "base_0"}),
            json!({"cmd": "impact", "target": "base_1"}),
        ] {
            let g_before = generation(&reader);
            let envelope = ipc(&socket, query.clone()).await;
            let g_after = generation(&reader);
            let still_settled = !tree_differs(&root, &reader);
            let (fresh, _) = verdict(&envelope);
            match fresh {
                Some(false) if g_before == g_after && still_settled => tally.violations.push(format!(
                    "cycle {cycle}: fresh:false right after a completed rebuild of generation {g_after} for {query}: {envelope}"
                )),
                Some(true) => tally.settled_true += 1,
                None => tally.settled_null += 1,
                Some(false) => {}
            }
        }
    }

    handle.request_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(30), running).await;
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&root);

    eprintln!(
        "envelope freshness over {CYCLES} cycles: racy true/false/null = {}/{}/{} \
         ({} inconclusive); settled true/null = {}/{}",
        tally.racy_true,
        tally.racy_false,
        tally.racy_null,
        tally.racy_inconclusive,
        tally.settled_true,
        tally.settled_null
    );
    assert!(
        CYCLES - tally.racy_inconclusive >= 5,
        "only {} of {CYCLES} racy samples saw the tree differing before and after the query; the \
         test is not measuring the window it exists for",
        CYCLES - tally.racy_inconclusive
    );
    assert!(
        tally.violations.is_empty(),
        "{} freshness violations:\n{}",
        tally.violations.len(),
        tally.violations.join("\n")
    );
}
