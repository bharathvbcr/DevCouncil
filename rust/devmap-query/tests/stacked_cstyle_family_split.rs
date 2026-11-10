//! The stacked (delta over base) resolver compares `LangFamily::from_lang`
//! results by equality at its two cross-file candidate sites, so it is a second
//! owner of the cross-family rule beside `LangFamily::admits`. It must follow
//! the CStyle split: a bare call in a delta file reaches a same-named
//! declaration in the base only inside its own group (C-like, Java, C#).
//!
//! This file owns its process: it shells out to `git`, and the in-crate stacked
//! tests hold a lock around the `git` path that a shared target would not honour.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use devmap_extract::paths::store_path;
use devmap_query::{provision_stacked_worktree, Cancel, StackedStore};
use devmap_store::{GenerationWriteOpts, Store};

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-cstyle-split-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A committed, indexed anchor holding `files`, and a lane provisioned over it.
fn lane_over(tag: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let root = scratch(tag);
    let anchor = root.join("anchor");
    for (rel, text) in files {
        let path = anchor.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fs::write(anchor.join(".gitignore"), ".devmap/\n.devcouncil/\n").unwrap();
    git(&anchor, &["init", "-b", "main"]);
    git(&anchor, &["add", "."]);
    git(&anchor, &["commit", "-m", "initial"]);

    let db_path = store_path(&anchor);
    fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let store = Store::open(&db_path).unwrap();
    store.bind_repo_root(&anchor.canonicalize().unwrap()).unwrap();
    let exts: Vec<_> = files
        .iter()
        .map(|(rel, text)| devmap_extract::extract_file(rel, text))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&exts);
    let res = resolver.resolve_all(&exts).unwrap();
    let analysis = devmap_analyze::analyze(&exts, &res);
    store
        .save_generation_with_metadata(
            &exts,
            &res,
            &analysis,
            GenerationWriteOpts::default(),
            &git(&anchor, &["rev-parse", "HEAD"]),
        )
        .unwrap();
    let _ = store.write_latest_csr_sidecar().unwrap();
    drop(store);

    let lane = root.join("lane");
    git(&anchor, &["worktree", "add", "-b", "lane", lane.to_str().unwrap()]);
    provision_stacked_worktree(&anchor, &lane).unwrap();
    (root, lane)
}

/// Resolved `Calls` edges of `symbol` as the merged (base + delta) index holds
/// them, `(target_file, target_symbol)`. `edges_for_symbol` is not used: it
/// returns the delta's raw, name-keyed call records, which carry no family.
fn call_targets(lane: &Path, file: &str, symbol: &str) -> Vec<(String, String)> {
    let store = StackedStore::open(lane).unwrap();
    let edges = store
        .file_edges(file, 0.0, &Cancel::new())
        .unwrap()
        .expect("the delta file is indexed")
        .edges;
    edges
        .into_iter()
        .filter(|e| e.edge_kind == "Calls" && e.source_symbol == format!("{file}::{symbol}"))
        .map(|e| (e.target_file, e.target_symbol))
        .collect()
}

#[cfg(unix)]
#[test]
fn a_delta_call_binds_a_base_declaration_only_inside_its_own_group() {
    let (root, lane) = lane_over(
        "delta",
        &[
            ("c/lib.c", "int only_c(int x) { return x; }\nint shared_name(int x) { return x; }\n"),
            ("java/Sh.java", "class Sh { static int shared_name(int x) { return x; } }\n"),
            ("cs/Sh.cs", "class Sh2 { static int shared_name(int x) { return x; } }\n"),
        ],
    );
    // Three delta callers: a C call whose name three groups declare, a Java
    // call of a C function, and a C# call of a Java method.
    fs::create_dir_all(lane.join("c")).unwrap();
    fs::write(
        lane.join("c/ns.c"),
        "int c_uses_shared(int v) { return shared_name(v); }\n",
    )
    .unwrap();
    fs::write(
        lane.join("java/Use.java"),
        "class U { int go() { return only_c(1); } }\n",
    )
    .unwrap();
    fs::write(
        lane.join("cs/Use.cs"),
        "class V { static int go() { return shared_name(1); } }\n",
    )
    .unwrap();
    let mut store = StackedStore::open(&lane).unwrap();
    store.sync_worktree().unwrap();
    drop(store);

    let c = call_targets(&lane, "c/ns.c", "c_uses_shared");
    assert_eq!(
        c,
        vec![("c/lib.c".to_string(), "c/lib.c::shared_name".to_string())],
        "the C call binds only the C-like declaration"
    );
    let java = call_targets(&lane, "java/Use.java", "U.go");
    assert!(java.is_empty(), "Java must not bind a C function: {java:?}");
    let cs = call_targets(&lane, "cs/Use.cs", "V.go");
    assert_eq!(
        cs,
        vec![("cs/Sh.cs".to_string(), "cs/Sh.cs::Sh2.shared_name".to_string())],
        "the C# call binds only the C# declaration"
    );
    let _ = fs::remove_dir_all(root);
}
