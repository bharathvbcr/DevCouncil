//! W2.3 at the surface an agent actually types.
//!
//! The engine half is covered in `devmap-query/tests/rung_filter.rs`. This is
//! the other half of "usable from the outside": a ladder addressable only from
//! a Rust struct is not addressable. Three properties, each in both directions:
//!
//! * **The flag exists and narrows.** `--min-rung deterministic` returns fewer
//!   edges than the same query without it.
//! * **A typo is refused.** Not defaulted to no filter — a caller who asked for
//!   a narrow answer and silently got a broad one will act on the wrong list,
//!   which is worse than an error.
//! * **The cost is visible.** Both the JSON `rungs` object and the human note
//!   say how many edges the floor hid, so a short list is never mistaken for a
//!   sparse graph.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

/// The same shape as the engine fixture: one same-file call resolves
/// deterministically, one call to a name two files define resolves ambiguously
/// to both. Mixed rungs are the precondition for any of this to mean anything.
/// `name` is not decoration. `SystemTime` on macOS is coarse enough that four
/// threads starting at once can read the same nanosecond, and with a shared pid
/// that put two tests in one directory — where the first to finish deleted the
/// other's store mid-query. The failure looked like a race in the kernel and
/// was a collision in the harness.
fn corpus(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-min-rung-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("lib_a.py"), "def shared():\n    return 1\n").unwrap();
    std::fs::write(root.join("lib_b.py"), "def shared():\n    return 2\n").unwrap();
    std::fs::write(
        root.join("app.py"),
        "def helper():\n    return shared()\n\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    let built = run(&root, &["build", "."]);
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    root
}

fn json(output: &Output, what: &str) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or("");
    serde_json::from_str(line).unwrap_or_else(|error| {
        panic!(
            "{what}: stdout is not JSON ({error}): {line}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn a_bad_rung_name_is_refused_rather_than_ignored() {
    let root = corpus("refuse");
    for bad in ["Deterministic", "certain", "lsp", ""] {
        let out = run(&root, &["trace", "app.py::main", "--min-rung", bad]);
        assert!(
            !out.status.success(),
            "--min-rung {bad:?} was accepted; a filter that silently does \
             nothing is worse than an error, because the caller acts on the \
             broad list believing it is narrow"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("deterministic"),
            "the refusal must name the valid rungs, got: {stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_flag_narrows_and_says_what_it_hid() {
    let root = corpus("narrows");

    let open = json(
        &run(&root, &["--json", "trace", "app.py::main"]),
        "unfiltered",
    );
    let floored = json(
        &run(
            &root,
            &[
                "--json",
                "trace",
                "app.py::main",
                "--min-rung",
                "deterministic",
            ],
        ),
        "filtered",
    );

    let open_total = open["total"].as_u64().expect("total");
    let floored_total = floored["total"].as_u64().expect("total");
    assert!(
        open_total > 0,
        "the fixture produced no edges, so nothing below discriminates: {open}"
    );
    assert!(
        floored_total < open_total,
        "the floor removed nothing: {floored_total} of {open_total}"
    );

    // The histogram describes the population before the cut, so the two
    // queries must agree on it — that agreement is what lets a caller tell a
    // filtered answer from a sparse graph.
    let hist = &floored["rungs"];
    assert_eq!(
        hist["deterministic"].as_u64().unwrap()
            + hist["high"].as_u64().unwrap()
            + hist["speculative"].as_u64().unwrap(),
        open["rungs"]["deterministic"].as_u64().unwrap()
            + open["rungs"]["high"].as_u64().unwrap()
            + open["rungs"]["speculative"].as_u64().unwrap(),
        "the histogram must count the same population either way: {floored}"
    );
    assert!(
        hist["filtered_out"].as_u64().unwrap() > 0,
        "the filtered answer must report a non-zero cost: {floored}"
    );
    assert_eq!(
        open["rungs"]["filtered_out"].as_u64().unwrap(),
        0,
        "an unfiltered answer must report no cost: {open}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The terminal reader gets the same disclosure the JSON reader does — and only
/// when there is something to disclose.
#[test]
fn the_human_output_names_the_cost_only_when_there_is_one() {
    let root = corpus("human");

    let quiet = run(&root, &["trace", "app.py::main"]);
    let stdout = String::from_utf8_lossy(&quiet.stdout);
    assert!(
        !stdout.contains("--min-rung hid"),
        "an unfiltered query must read exactly as it did before the flag \
         existed, got:\n{stdout}"
    );

    let loud = run(
        &root,
        &["trace", "app.py::main", "--min-rung", "deterministic"],
    );
    let stdout = String::from_utf8_lossy(&loud.stdout);
    assert!(
        stdout.contains("--min-rung hid"),
        "a filtered query must say what it hid, got:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `deps`, `impact`, `trace` — and `neighbors`, which is the first two composed.
///
/// `neighbors` pinned the floor to `None` inside its fan-out while both halves
/// took one, so `dev map query` — whose edge lists come from this command —
/// could not ask for deterministic-only edges even though the two queries
/// behind it could. Included in the same sweep as its parts, because a
/// composition that accepts fewer filters than what it composes is the defect,
/// not a separate feature.
#[test]
fn the_flag_reaches_every_query_the_plan_named() {
    let root = corpus("every-query");
    for args in [
        vec!["deps", "app.py"],
        vec!["impact", "lib_a.py::shared"],
        vec!["trace", "app.py::main"],
        vec!["neighbors", "app.py::main"],
    ] {
        let mut good = args.clone();
        good.extend(["--min-rung", "high"]);
        let out = run(&root, &good);
        assert!(
            out.status.success(),
            "{args:?} rejected a valid rung: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let mut bad = args.clone();
        bad.extend(["--min-rung", "nonsense"]);
        let out = run(&root, &bad);
        assert!(!out.status.success(), "{args:?} accepted an invalid rung");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// And the composed command really narrows, at the binary a caller runs.
///
/// The engine-level parity lives in `devmap-query/tests/neighbors_composition.rs`.
/// This is the other half of "usable from the outside": a floor the engine
/// honours and the CLI drops is a flag that does nothing, and the answer looks
/// exactly like a correctly narrowed one.
#[test]
fn the_composed_query_narrows_at_the_binary() {
    let root = corpus("composed-narrows");
    let count = |value: &serde_json::Value| -> usize {
        value["neighbors"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry["callers"]["items"].as_array().map_or(0, Vec::len)
                            + entry["callees"]["items"].as_array().map_or(0, Vec::len)
                    })
                    .sum()
            })
            .unwrap_or(0)
    };

    let open = json(
        &run(&root, &["--json", "neighbors", "app.py", "app.py::helper"]),
        "unfiltered neighbors",
    );
    let floored = json(
        &run(
            &root,
            &[
                "--json",
                "neighbors",
                "app.py",
                "app.py::helper",
                "--min-rung",
                "deterministic",
            ],
        ),
        "floored neighbors",
    );

    let (before, after) = (count(&open), count(&floored));
    assert!(
        before > 0,
        "the corpus must produce edges, or this compares two empty answers"
    );
    assert!(
        after < before,
        "`--min-rung deterministic` must cut the composed answer as it cuts \
         `deps` and `impact`: {before} edges unfiltered, {after} floored"
    );
    let _ = std::fs::remove_dir_all(&root);
}
