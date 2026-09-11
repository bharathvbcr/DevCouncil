//! W4.2 — a grammar bump that silently stops extracting calls fails CI on the
//! language it broke.
//!
//! The golden fixtures pin identity and the W4.1 corpus pins dead-code
//! precision. Neither catches the failure this test exists for: a tree-sitter
//! upgrade that quietly stops matching one node kind. Every golden still passes
//! — the nodes and edges that *are* produced are unchanged — while the share of
//! calls the resolver can attribute collapses for that one language.
//!
//! gortex fences exactly this and calls it `eval parity`. It is a proxy rather
//! than a correctness measure, and it is the only discipline any of the three
//! comparable tools has that identity fixtures do not already cover.
//!
//! Two honesty properties, both asserted below:
//!
//! * **A rate that was measured and is now absent is a regression**, not a
//!   match. `Permille` is `Option<u32>`: `None` means "no call site here, so no
//!   rate was attempted". A baseline of 800 against a current of `None` means
//!   the calls stopped being extracted, which is the exact failure this fences.
//! * **The message names the language and the delta.** "Resolution dropped" is
//!   not actionable; "rust: 235 -> 71 permille" is.

use devmap_extract::extract_file;
use devmap_resolve::Resolver;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How far a rate may move before it is a regression.
///
/// Small on purpose. These fixtures are deterministic and tiny — `python_app`
/// resolves 2 of 2 call sites — so a single lost edge moves the rate by
/// hundreds of permille. The tolerance absorbs rounding when a count shifts by
/// one, not a real regression.
const TOLERANCE_PERMILLE: i64 = 10;

fn workspace_root() -> PathBuf {
    // `devmap-analyze` sits directly under the Cargo workspace root (`rust/`).
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("package sits under the workspace root")
        .to_path_buf()
}

/// `fixture -> language -> net permille`, as measured now.
fn measure() -> BTreeMap<String, BTreeMap<String, Option<u32>>> {
    let root = workspace_root();
    let golden = root.join("testdata/golden");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&golden)
        .expect("testdata/golden is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("truth.json").is_file())
        .collect();
    dirs.sort();

    let mut out = BTreeMap::new();
    for dir in dirs {
        let truth: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("truth.json")).expect("truth.json is readable"),
        )
        .expect("truth.json is JSON");
        let source = root.join(truth["source"].as_str().expect("source"));
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&source)
            .unwrap_or_else(|error| panic!("{}: {error}", source.display()))
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        paths.sort();
        let extractions: Vec<_> = paths
            .iter()
            .map(|path| {
                let rel = path
                    .strip_prefix(&source)
                    .expect("under the fixture")
                    .to_string_lossy()
                    .replace('\\', "/");
                extract_file(&rel, &std::fs::read_to_string(path).expect("readable"))
            })
            .collect();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = devmap_analyze::analyze(&extractions, &resolution);

        let by_language = analysis
            .resolution_rate
            .by_language
            .iter()
            .map(|(language, row)| (language.clone(), row.net_permille))
            .collect();
        out.insert(
            dir.file_name().unwrap().to_string_lossy().into_owned(),
            by_language,
        );
    }
    out
}

fn baseline_path() -> PathBuf {
    workspace_root().join("testdata/resolution_baseline.json")
}

fn write_baseline(measured: &BTreeMap<String, BTreeMap<String, Option<u32>>>) {
    let existing: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(baseline_path()).expect("baseline readable"))
            .expect("baseline is JSON");
    let mut updated = existing.clone();
    updated["fixtures"] = serde_json::to_value(measured).expect("serializable");
    std::fs::write(
        baseline_path(),
        serde_json::to_string_pretty(&updated).expect("serializable") + "\n",
    )
    .expect("baseline is writable");
}

#[test]
fn per_language_resolution_has_not_regressed() {
    let measured = measure();

    if std::env::var_os("DEVMAP_UPDATE_RESOLUTION_BASELINE").is_some() {
        write_baseline(&measured);
        eprintln!("resolution baseline rewritten from the current measurement");
        return;
    }

    let baseline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(baseline_path()).expect("baseline readable"))
            .expect("baseline is JSON");
    let frozen = baseline["fixtures"]
        .as_object()
        .expect("fixtures is an object");

    let mut regressions: Vec<String> = Vec::new();

    for (fixture, languages) in frozen {
        let Some(current) = measured.get(fixture) else {
            regressions.push(format!(
                "{fixture}: baselined but no longer measured — the fixture or its \
                 truth.json is gone, so the baseline is fencing nothing"
            ));
            continue;
        };
        for (language, expected) in languages.as_object().expect("language map") {
            let want = expected.as_i64();
            let got = current.get(language).copied().flatten().map(i64::from);
            match (want, got) {
                (Some(want), Some(got)) if got + TOLERANCE_PERMILLE < want => {
                    regressions.push(format!(
                        "{fixture}/{language}: {want} -> {got} permille \
                         (-{} beyond the {TOLERANCE_PERMILLE} tolerance)",
                        want - got
                    ));
                }
                (Some(want), None) => {
                    // The failure this test exists for. A language that
                    // extracted calls and now reports no rate at all is not
                    // "unchanged" and is not "zero" — it stopped attempting.
                    regressions.push(format!(
                        "{fixture}/{language}: {want} permille -> no rate at all. The \
                         resolver attempted no call site in a fixture that used to \
                         have them, which is what a grammar that stopped matching a \
                         node kind looks like"
                    ));
                }
                _ => {}
            }
        }
    }

    // A language the corpus gained is not a failure, but it must be recorded,
    // or it is fenced by nothing while looking like it is.
    for (fixture, languages) in &measured {
        let Some(frozen_languages) = frozen.get(fixture).and_then(|v| v.as_object()) else {
            regressions.push(format!(
                "{fixture}: measured but not baselined — add it with \
                 DEVMAP_UPDATE_RESOLUTION_BASELINE=1"
            ));
            continue;
        };
        for language in languages.keys() {
            if !frozen_languages.contains_key(language) {
                regressions.push(format!(
                    "{fixture}/{language}: measured but not baselined, so nothing \
                     would notice it regressing"
                ));
            }
        }
    }

    assert!(
        regressions.is_empty(),
        "per-language call resolution regressed:\n  {}\n\nIf the drop is \
         intentional, say why and rewrite the baseline with \
         DEVMAP_UPDATE_RESOLUTION_BASELINE=1.",
        regressions.join("\n  ")
    );
}

/// The OFF direction: the baseline must describe the corpus that exists.
///
/// Without this, a baseline listing fixtures nobody measures any more would
/// pass forever while fencing nothing.
#[test]
fn every_baselined_fixture_is_still_measured() {
    let baseline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(baseline_path()).expect("baseline readable"))
            .expect("baseline is JSON");
    let frozen: Vec<&String> = baseline["fixtures"]
        .as_object()
        .expect("fixtures is an object")
        .keys()
        .collect();
    assert!(!frozen.is_empty(), "the baseline fences nothing");

    let measured = measure();
    for fixture in frozen {
        assert!(
            measured.contains_key(fixture),
            "{fixture} is baselined but no longer measured"
        );
    }
}
