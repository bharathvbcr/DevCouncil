use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn build(root: &std::path::Path) {
    let out = Command::new(devmap())
        .args(["build", "."])
        .current_dir(root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every edge in the committed generation, as a comparable set.
fn graph(root: &std::path::Path) -> Vec<String> {
    let store = devmap_store::Store::open(db_path(root)).unwrap();
    let mut rows = store.latest_edges_for_test().unwrap();
    rows.sort();
    rows
}

/// The committed generation's analysis, as a comparable string.
///
/// Serialized rather than compared field by field so that a field added later
/// is covered without anyone remembering to add it here — the failure this
/// whole test guards against is a part of the generation nobody thought to
/// compare.
fn analysis(root: &std::path::Path) -> String {
    let store = devmap_store::Store::open(db_path(root)).unwrap();
    let summary = store
        .latest_analysis()
        .unwrap()
        .expect("a committed generation carries its analysis");
    serde_json::to_string_pretty(&summary).unwrap()
}

/// The dead-code rows `devmap dead` answers from.
///
/// A separate table from the analysis JSON above, and the one a consumer
/// actually reads, so it is compared separately rather than assumed to agree.
fn dead(root: &std::path::Path) -> Vec<String> {
    let store = devmap_store::Store::open(db_path(root)).unwrap();
    let mut rows: Vec<String> = store
        .latest_dead_symbols()
        .unwrap()
        .into_iter()
        .map(|report| {
            format!(
                "{}::{} {:.3} exempt={} {:?}",
                report.file_path,
                report.symbol_name,
                report.confidence,
                report.is_exempt,
                report.exemption_reason
            )
        })
        .collect();
    rows.sort();
    rows
}

/// An incremental build must produce the same graph as a cold build.
///
/// B3/SC2 resolves only the files a change can reach: the changed files plus
/// every file mentioning a name whose definition moved. That is sound because
/// the resolver's two global indexes are keyed by symbol name — but "sound
/// because of an argument" is exactly what SC16 was, where an incremental build
/// diverged permanently and no gate could see it. So the property is asserted
/// directly: edit a file, build incrementally, then build the identical tree
/// cold, and require the graphs to be equal.
#[test]
fn an_incremental_build_equals_a_cold_build() {
    let root = std::env::temp_dir().join(format!("devmap-incr-{}", std::process::id()));
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.py"), "def a():\n    return helper()\n").unwrap();
    std::fs::write(src.join("b.py"), "def helper():\n    return 1\n").unwrap();
    std::fs::write(
        src.join("c.py"),
        "from b import helper\n\ndef c():\n    return helper()\n",
    )
    .unwrap();

    build(&root);

    // Change a definition other files depend on: `helper` gains a sibling, so
    // the symbol index changes for a name two other files call.
    std::fs::write(
        src.join("b.py"),
        "def helper():\n    return 1\n\ndef helper_two():\n    return 2\n",
    )
    .unwrap();
    build(&root);
    let incremental = graph(&root);

    // Same tree, no history.
    std::fs::remove_dir_all(devmap_extract::paths::state_dir(&root)).unwrap();
    build(&root);
    let cold = graph(&root);

    assert_eq!(
        incremental, cold,
        "an incremental build must produce exactly the cold graph"
    );
    assert!(
        !cold.is_empty(),
        "fixture precondition: the graph is non-empty"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The same property as above, over the part of the generation that is *not*
/// the graph, on a tree where the affected set is genuinely a small subset.
///
/// The test above passes a three-file fixture where editing one file makes the
/// other two affected too, so the incremental path barely narrows anything —
/// and it compares only the edges, which carry forward correctly by
/// construction. Both of those were why it stayed green through a defect that
/// committed 433 dead-code candidates where a cold build found 14: the analyser
/// was handed the changed file's edges alone, and liveness and clustering are
/// global questions that no subset of the edges can answer.
///
/// So this fixture is twenty independent modules, of which an edit touches one,
/// and the assertion is the whole generation: edges, the analysis summary, and
/// the dead-code rows a consumer reads.
#[test]
fn an_incremental_build_equals_a_cold_build_in_analysis_too() {
    let root = std::env::temp_dir().join(format!("devmap-incr-analysis-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();

    // Twenty modules that do not mention each other, so editing one leaves the
    // other nineteen unaffected — which is the case the subset path narrows to
    // and the case the old fixture could not produce.
    for index in 0..20 {
        std::fs::write(
            src.join(format!("mod_{index}.py")),
            format!(
                "def leaf_{index}():\n    return {index}\n\n\ndef caller_{index}():\n    return leaf_{index}()\n"
            ),
        )
        .unwrap();
    }
    // One hub that calls across files, so the graph is not twenty islands.
    std::fs::write(
        src.join("hub.py"),
        "from mod_0 import caller_0\nfrom mod_1 import caller_1\n\n\ndef hub():\n    return caller_0() + caller_1()\n",
    )
    .unwrap();

    build(&root);
    let cold_first = analysis(&root);
    assert!(
        cold_first.contains("\"total_edges\""),
        "fixture precondition: an analysis was committed"
    );

    // Edit one leaf. Nothing else names `leaf_19`, so the affected set is this
    // file alone and the subset path narrows as far as it ever does.
    std::fs::write(
        src.join("mod_19.py"),
        "def leaf_19():\n    return 19\n\n\ndef caller_19():\n    return leaf_19() + 1\n",
    )
    .unwrap();
    build(&root);

    let incremental_graph = graph(&root);
    let incremental_analysis = analysis(&root);
    let incremental_dead = dead(&root);

    // Same tree, no history.
    std::fs::remove_dir_all(devmap_extract::paths::state_dir(&root)).unwrap();
    build(&root);

    assert_eq!(
        incremental_graph,
        graph(&root),
        "an incremental build must store exactly the cold graph"
    );
    assert_eq!(
        incremental_dead,
        dead(&root),
        "an incremental build must store exactly the cold dead-code rows; \
         a symbol reported callerless here is one somebody deletes"
    );
    assert_eq!(
        incremental_analysis,
        analysis(&root),
        "an incremental build must store exactly the cold analysis"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A fixture of independent modules plus one hub, so an edit to a single leaf
/// leaves the rest unaffected and the differential path narrows as far as it
/// ever does.
fn write_fixture(src: &std::path::Path, modules: usize) {
    std::fs::create_dir_all(src).unwrap();
    for index in 0..modules {
        std::fs::write(
            src.join(format!("mod_{index}.py")),
            format!(
                "def leaf_{index}():\n    return {index}\n\n\ndef caller_{index}():\n    return leaf_{index}()\n"
            ),
        )
        .unwrap();
    }
    std::fs::write(
        src.join("hub.py"),
        "from mod_0 import caller_0\nfrom mod_1 import caller_1\n\n\ndef hub():\n    return caller_0() + caller_1()\n",
    )
    .unwrap();
}

/// The store the build under test writes.
///
/// Resolved rather than spelled out, so these tests follow
/// `devmap_extract::paths` wherever the state directory goes. They are about
/// incremental-vs-cold equivalence; which directory holds the store is not
/// their claim to make, and a literal here would fail the day the layout
/// changes for reasons that have nothing to do with what they assert.
fn db_path(root: &std::path::Path) -> std::path::PathBuf {
    devmap_extract::paths::store_path(root)
}

/// Rewrite the stored payload identity of the latest generation, which is what
/// upgrading the kernel does to a store that already has one: same bytes on
/// disk, same content hashes, rows produced by an extractor that no longer
/// exists.
fn age_stored_payloads(root: &std::path::Path, analyzer: Option<&str>) {
    let conn = rusqlite::Connection::open(db_path(root)).unwrap();
    // `generation_files` is a view since schema v17 and is not updatable; the
    // identity lives on `file_payloads`, and it is unique per
    // (file, content, language, grammar, analyzer).
    //
    // That uniqueness is why this is three statements rather than one. Aging a
    // payload *changes its identity*, and a store that has been aged before can
    // already hold the aged twin — so a blind update collides. What the fixture
    // means is "every stored payload now looks like an older kernel wrote it",
    // and the honest way to say that is: age what can be aged, repoint anything
    // whose aged twin already exists, then drop what nothing points at.
    let changed = age_payloads(&conn, analyzer);
    assert!(
        changed > 0,
        "fixture precondition: a generation exists to age"
    );
}

/// Re-stamp every stored payload with `analyzer`, merging duplicates.
fn age_payloads(conn: &rusqlite::Connection, analyzer: Option<&str>) -> usize {
    // The twin of a payload: same file and content, differing only in the
    // analyzer identity, already carrying the value we are aging towards.
    const TWIN: &str = "SELECT o.payload_id FROM file_payloads o
                         WHERE o.file_id = mine.file_id
                           AND o.content_hash = mine.content_hash
                           AND o.language = mine.language
                           AND COALESCE(o.grammar_version, '') = COALESCE(mine.grammar_version, '')
                           AND COALESCE(o.analyzer_version, '') = COALESCE(?1, '')
                           AND o.payload_id <> mine.payload_id";

    let aged = conn
        .execute(
            &format!(
                "UPDATE file_payloads AS mine SET analyzer_version = ?1
                  WHERE COALESCE(mine.analyzer_version, '') <> COALESCE(?1, '')
                    AND NOT EXISTS ({TWIN})"
            ),
            rusqlite::params![analyzer],
        )
        .unwrap();

    let repointed = conn
        .execute(
            &format!(
                "UPDATE generation_file_rows SET payload_id = (
                     SELECT ({TWIN}) FROM file_payloads mine
                      WHERE mine.payload_id = generation_file_rows.payload_id)
                  WHERE EXISTS (
                     SELECT 1 FROM file_payloads mine
                      WHERE mine.payload_id = generation_file_rows.payload_id
                        AND COALESCE(mine.analyzer_version, '') <> COALESCE(?1, '')
                        AND EXISTS ({TWIN}))"
            ),
            rusqlite::params![analyzer],
        )
        .unwrap();

    conn.execute(
        "DELETE FROM file_payloads
          WHERE payload_id NOT IN (SELECT payload_id FROM generation_file_rows)",
        [],
    )
    .unwrap();

    aged + repointed
}

/// Copy a working tree without its index, so the same sources can be built cold.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        // Both state directory names: the source tree may carry either, and a
        // copy that brought one along would not be a cold build.
        if devmap_extract::paths::is_state_dir_name(&name.to_string_lossy()) {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The whole committed generation, as one comparable value.
fn generation(root: &std::path::Path) -> (Vec<String>, String, Vec<String>) {
    (graph(root), analysis(root), dead(root))
}

/// Building the same sources with no history, for comparison.
fn cold_generation(
    tree: &std::path::Path,
    scratch: &std::path::Path,
) -> (Vec<String>, String, Vec<String>) {
    let _ = std::fs::remove_dir_all(scratch);
    copy_tree(tree, scratch);
    build(scratch);
    generation(scratch)
}

/// Delete stored edges from the latest generation, which is what a *less
/// capable* extractor leaves behind.
///
/// Relabelling a version string is not enough to reproduce this: the rows would
/// still be the ones today's extractor produces, so nothing would disagree. The
/// real store held rows written when eleven languages had no call extraction at
/// all, so the previous generation genuinely carried fewer edges for files
/// whose bytes never changed. Removing rows reproduces that, and only that.
fn drop_stored_edges(root: &std::path::Path, keep_source: &str, drop_count: usize) {
    let conn = rusqlite::Connection::open(db_path(root)).unwrap();
    let removed = conn
        .execute(
            // `generation_edges` is a view over the validity ranges since v18
            // and is not deletable; the rows live in `edge_rows`, keyed by
            // `edge_id`. Only currently-valid rows are removed — a closed row
            // is already invisible to the latest generation, so deleting one
            // would not reproduce anything.
            "DELETE FROM edge_rows
             WHERE valid_to IS NULL
               AND edge_id IN (
                 SELECT e.edge_id FROM edge_rows e
                 JOIN paths sp ON sp.id = e.source_file_id
                 WHERE e.valid_to IS NULL
                   AND sp.path <> ?1
                 LIMIT ?2
             )",
            rusqlite::params![keep_source, drop_count as i64],
        )
        .unwrap();
    assert_eq!(
        removed, drop_count,
        "fixture precondition: the generation had edges to remove"
    );
}

/// Upgrading the kernel must cost one full build, not the store.
///
/// The extraction cache keys on `(content hash, grammar version, analyzer
/// version)` and correctly re-extracts after a bump. The *generation* carried
/// its rows forward on the content hash alone, so an upgraded kernel kept
/// committing generations assembled from payloads the previous kernel wrote.
/// Nothing noticed until a file changed, at which point the stored edges and
/// the fresh analysis disagreed and the write was refused — measured on
/// DevCouncil at 65,615 stored against 65,798 analysed, with `dev map`
/// unusable until the database was deleted by hand.
///
/// Both endings are asserted: the build succeeds, *and* what it stored is the
/// cold answer rather than a mix of two kernels.
#[test]
fn a_generation_from_an_older_extractor_is_rebuilt_not_refused() {
    let root = std::env::temp_dir().join(format!("devmap-aged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write_fixture(&root.join("src"), 20);
    build(&root);

    // An older kernel: fewer edges recorded, and a payload identity that says
    // so. The edited file's own edges are left alone so the divergence lives
    // entirely in files the differential path would carry forward.
    drop_stored_edges(&root, "src/mod_19.py", 3);
    age_stored_payloads(&root, Some("0.0.9:extract-v1"));

    std::fs::write(
        root.join("src/mod_19.py"),
        "def leaf_19():\n    return 19\n\n\ndef caller_19():\n    return leaf_19() + 1\n",
    )
    .unwrap();
    build(&root);

    let scratch = std::env::temp_dir().join(format!("devmap-aged-cold-{}", std::process::id()));
    assert_eq!(
        generation(&root),
        cold_generation(&root, &scratch),
        "a build over an aged store must store the cold answer, not a mix of two kernels"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The same recovery when the payload identity gives no warning at all.
///
/// The identity gate catches an upgraded extractor. It cannot catch an edge
/// that resolves differently today for a file whose own bytes never changed —
/// a Go package renamed, an import alias repointed, a name that became
/// ambiguous — because the affected-set closure keys on symbol names and those
/// did not move. That case reached the same refusal, and no version check
/// would have prevented it.
///
/// So the fixture leaves the identity current and only makes the stored edges
/// disagree with what a fresh resolution produces. The build must still commit
/// the cold answer, because edges now come from this build's resolution rather
/// than from the generation before it.
#[test]
fn stored_edges_that_disagree_with_a_fresh_resolution_are_replaced_not_carried() {
    let root = std::env::temp_dir().join(format!("devmap-drift-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write_fixture(&root.join("src"), 20);
    build(&root);

    drop_stored_edges(&root, "src/mod_4.py", 5);

    std::fs::write(
        root.join("src/mod_4.py"),
        "def leaf_4():\n    return 4\n\n\ndef caller_4():\n    return leaf_4() + 4\n",
    )
    .unwrap();
    build(&root);

    let scratch = std::env::temp_dir().join(format!("devmap-drift-cold-{}", std::process::id()));
    assert_eq!(
        generation(&root),
        cold_generation(&root, &scratch),
        "stored edges must be this build's resolution, whatever the previous generation held"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A row from before the identity columns existed carries no identity, and
/// unknown is not a match. Fail closed: rebuild rather than reuse.
#[test]
fn a_generation_with_no_recorded_identity_is_rebuilt_not_reused() {
    let root = std::env::temp_dir().join(format!("devmap-nullid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write_fixture(&root.join("src"), 20);
    build(&root);

    drop_stored_edges(&root, "src/mod_7.py", 3);
    age_stored_payloads(&root, None);

    std::fs::write(
        root.join("src/mod_7.py"),
        "def leaf_7():\n    return 7\n\n\ndef caller_7():\n    return leaf_7() + 7\n",
    )
    .unwrap();
    build(&root);

    let scratch = std::env::temp_dir().join(format!("devmap-nullid-cold-{}", std::process::id()));
    assert_eq!(
        generation(&root),
        cold_generation(&root, &scratch),
        "a generation with no recorded payload identity must not be carried forward"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Deterministic pseudo-random source. A fixed seed keeps a failure
/// reproducible; `Math.random`-style churn that cannot be replayed is not
/// evidence of anything.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn pick(&mut self, bound: usize) -> usize {
        (self.next() as usize) % bound.max(1)
    }
}

/// The property, under churn the fixed fixtures do not produce.
///
/// Every targeted test above encodes a failure somebody already found. This one
/// exists for the ones nobody has: a few hundred builds of edits, renames,
/// additions, deletions, simulated kernel upgrades and simulated
/// less-capable-extractor generations, in orders no one chose, each followed by
/// the same question — is the committed generation the one a cold build of these
/// exact bytes produces?
///
/// It is the assertion that does not depend on knowing *why* the differential
/// path might diverge, which is the part that kept being wrong.
#[test]
fn an_incremental_build_equals_a_cold_build_under_random_churn() {
    let root = std::env::temp_dir().join(format!("devmap-churn-{}", std::process::id()));
    let scratch = std::env::temp_dir().join(format!("devmap-churn-cold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("src");
    write_fixture(&src, 12);
    build(&root);

    // Seed and length are overridable so the same test can be swept across many
    // seeds without editing it; the defaults keep the checked-in run fixed and
    // reproducible.
    let seed = std::env::var("DEVMAP_CHURN_SEED")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0x5EED_1234_ABCD_0001);
    let rounds = std::env::var("DEVMAP_CHURN_ROUNDS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(40);
    let mut rng = Lcg(seed);
    let mut live: Vec<usize> = (0..12).collect();
    let mut next_module = 12usize;

    for round in 0..rounds {
        // Every write carries `# rev {round}` so the bytes always move. Without
        // it an "edit" could reproduce the file's current contents — seed 3
        // round 1 wrote the value that was already there — and the build would
        // correctly do nothing, leaving the round asserting against a store the
        // build was never asked to touch.
        let operation = rng.pick(6);
        let description = match operation {
            // Edit a body: same symbols, different calls.
            0 => {
                let index = live[rng.pick(live.len())];
                let extra = rng.pick(5);
                std::fs::write(
                    src.join(format!("mod_{index}.py")),
                    format!(
                        "# rev {round}\ndef leaf_{index}():\n    return {extra}\n\n\ndef caller_{index}():\n    return leaf_{index}() + {extra}\n"
                    ),
                )
                .unwrap();
                format!("edit mod_{index}")
            }
            // Rename a definition, which moves a name other files may call.
            1 => {
                let index = live[rng.pick(live.len())];
                std::fs::write(
                    src.join(format!("mod_{index}.py")),
                    format!(
                        "def leaf_{index}_v{round}():\n    return {index}\n\n\ndef caller_{index}():\n    return leaf_{index}_v{round}()\n"
                    ),
                )
                .unwrap();
                format!("rename in mod_{index}")
            }
            // Add a module that calls into an existing one.
            2 => {
                let target = live[rng.pick(live.len())];
                let index = next_module;
                next_module += 1;
                std::fs::write(
                    src.join(format!("mod_{index}.py")),
                    format!(
                        "from mod_{target} import caller_{target}\n\n\ndef leaf_{index}():\n    return caller_{target}()\n"
                    ),
                )
                .unwrap();
                live.push(index);
                format!("add mod_{index}")
            }
            // Remove a module other files may still import.
            3 if live.len() > 4 => {
                let slot = rng.pick(live.len());
                let index = live.remove(slot);
                std::fs::remove_file(src.join(format!("mod_{index}.py"))).unwrap();
                format!("delete mod_{index}")
            }
            // The kernel was upgraded under a store that already had rows.
            4 => {
                age_stored_payloads(&root, Some("0.0.9:extract-v1"));
                let index = live[rng.pick(live.len())];
                std::fs::write(
                    src.join(format!("mod_{index}.py")),
                    format!(
                        "# rev {round}\ndef leaf_{index}():\n    return {round}\n\n\ndef caller_{index}():\n    return leaf_{index}()\n"
                    ),
                )
                .unwrap();
                format!("aged payloads, then edit mod_{index}")
            }
            // The previous generation recorded fewer edges than this extractor
            // finds — an older kernel's answer, with a current-looking identity.
            _ => {
                let index = live[rng.pick(live.len())];
                let conn = rusqlite::Connection::open(db_path(&root)).unwrap();
                conn.execute(
                    "DELETE FROM edge_rows
                      WHERE valid_to IS NULL
                        AND edge_id IN (
                          SELECT edge_id FROM edge_rows WHERE valid_to IS NULL LIMIT 2
                        )",
                    [],
                )
                .unwrap();
                std::fs::write(
                    src.join(format!("mod_{index}.py")),
                    format!(
                        "# rev {round}\ndef leaf_{index}():\n    return {round}0\n\n\ndef caller_{index}():\n    return leaf_{index}()\n"
                    ),
                )
                .unwrap();
                format!("dropped stored edges, then edit mod_{index}")
            }
        };

        build(&root);
        assert_eq!(
            generation(&root),
            cold_generation(&root, &scratch),
            "seed {seed} round {round} ({description}): the incremental generation diverged from a cold build"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// `--affected` skips the closure that decides a cold build is needed, so it
/// reaches the differential write with an aged store behind it.
///
/// That path stays correct for a reason worth pinning: the CLI holds the whole
/// tree in memory, so a file whose stored payload fails the identity gate is
/// simply written fresh instead of carried. Nothing is refused and nothing
/// stale survives. The daemon, which passes only the files it re-extracted, is
/// the caller that cannot do this — and it checks the identity before it
/// builds.
#[test]
fn an_explicit_affected_list_over_an_aged_store_still_equals_a_cold_build() {
    let root = std::env::temp_dir().join(format!("devmap-explicit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write_fixture(&root.join("src"), 20);
    build(&root);

    drop_stored_edges(&root, "src/mod_11.py", 4);
    age_stored_payloads(&root, Some("0.0.9:extract-v1"));

    std::fs::write(
        root.join("src/mod_11.py"),
        "def leaf_11():\n    return 11\n\n\ndef caller_11():\n    return leaf_11() + 11\n",
    )
    .unwrap();

    let out = std::process::Command::new(devmap())
        .args(["build", ".", "--affected", "src/mod_11.py"])
        .current_dir(&root)
        .output()
        .expect("devmap build --affected");
    assert!(
        out.status.success(),
        "an explicit affected list must not be refused: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let scratch = std::env::temp_dir().join(format!("devmap-explicit-cold-{}", std::process::id()));
    assert_eq!(
        generation(&root),
        cold_generation(&root, &scratch),
        "a differential write driven by --affected must still store the cold answer"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The no-change fast path must ask *which kernel* wrote what it is about to
/// call current.
///
/// Skipping resolve and analyse when the tree is byte-for-byte the committed
/// one saves more than half a build, and it is right — for one kernel. Across
/// an upgrade the same bytes produce a different graph, which is what an
/// extraction schema bump is, so content hashes alone reported a generation as
/// current that this kernel would never have written. Measured on DevCouncil:
/// the first `dev map` after two bumps printed "No source changes; generation
/// #412 still current (1,152 files)" over a store whose every row came from
/// `extract-v23`, and the failure only surfaced on the *next* build, as a
/// refusal with no obvious cause.
///
/// Found by the churn sweep above, not by review: seed 3 happened to rewrite a
/// file with the contents it already had.
#[test]
fn an_unchanged_tree_over_an_aged_store_is_still_rebuilt() {
    let root = std::env::temp_dir().join(format!("devmap-unchanged-aged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write_fixture(&root.join("src"), 20);
    build(&root);

    drop_stored_edges(&root, "src/hub.py", 3);
    age_stored_payloads(&root, Some("0.0.9:extract-v1"));

    // Not one byte of the tree changes.
    build(&root);

    let scratch =
        std::env::temp_dir().join(format!("devmap-unchanged-aged-cold-{}", std::process::id()));
    assert_eq!(
        generation(&root),
        cold_generation(&root, &scratch),
        "an unchanged tree over an aged store must be rebuilt, not declared current"
    );

    let conn = rusqlite::Connection::open(db_path(&root)).unwrap();
    let aged: i64 = conn
        .query_row(
            "SELECT count(*) FROM generation_files
             WHERE generation_id = (SELECT max(id) FROM generations)
               AND analyzer_version = '0.0.9:extract-v1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(aged, 0, "no row written by the older kernel may survive");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&scratch);
}
