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
/// them, `(target_file, target_symbol, resolution)`. `edges_for_symbol` is not
/// used: it returns the delta's raw, name-keyed call records, which carry no
/// family.
fn call_targets(lane: &Path, file: &str, symbol: &str) -> Vec<(String, String, String)> {
    let store = StackedStore::open(lane).unwrap();
    let edges = store
        .file_edges(file, 0.0, &Cancel::new())
        .unwrap()
        .expect("the delta file is indexed")
        .edges;
    edges
        .into_iter()
        .filter(|e| e.edge_kind == "Calls" && e.source_symbol == format!("{file}::{symbol}"))
        .map(|e| (e.target_file, e.target_symbol, e.resolution.unwrap_or_default()))
        .collect()
}

fn write(lane: &Path, rel: &str, text: &str) {
    let path = lane.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The stacked resolver binds `ImportScoped` when exactly one candidate sits in
/// the caller's own directory, whatever its family, so a caller placed beside
/// its target would pass with or without the split. Every caller here lives in
/// a directory of its own, so the only rung that can bind is the global one:
/// before the split three cross-group namesakes made it ambiguous (no edge) or
/// admitted a wrong-group target; after it, one same-group candidate is a
/// `UniqueGlobal`.
///
/// Two sites compare families: the delta-to-delta loop (`other_ext` in
/// `added_extractions`) and the base loop (`base_symbols_for_callee`). The
/// first four callers exercise the base site, the `dd_*` pair the delta site.
fn staged_lane(tag: &str) -> (PathBuf, PathBuf) {
    let (root, lane) = lane_over(
        tag,
        &[
            ("lib/lib.c", "int only_c(int x) { return x; }\nint shared_name(int x) { return x; }\n"),
            ("jv/Sh.java", "class Sh { static int shared_name(int x) { return x; } }\n"),
            ("cs0/Sh.cs", "class Sh2 { static int shared_name(int x) { return x; } }\n"),
        ],
    );
    // Base-site callers, each in its own directory.
    write(&lane, "app/ns.c", "int c_uses_shared(int v) { return shared_name(v); }\n");
    write(&lane, "app2/Use.cs", "class V { static int go() { return shared_name(1); } }\n");
    write(&lane, "app3/Use.java", "class U { int go() { return shared_name(1); } }\n");
    write(&lane, "app4/JC.java", "class JC { int go() { return only_c(1); } }\n");
    // Delta-to-delta: a delta Java declaration, a delta C caller (negative), a
    // delta Java caller (positive), and a delta C declaration with a C caller.
    write(&lane, "d1/Decl.java", "class Decl { static int dd_name(int x) { return x; } }\n");
    write(&lane, "d2/Call.c", "int c_calls_dd(int v) { return dd_name(v); }\n");
    write(&lane, "d3/Call.java", "class JD { int go() { return dd_name(1); } }\n");
    write(&lane, "d4/Decl2.c", "int dd_c(int x) { return x; }\n");
    write(&lane, "d5/Call2.c", "int c_calls_ddc(int v) { return dd_c(v); }\n");
    let mut store = StackedStore::open(&lane).unwrap();
    store.sync_worktree().unwrap();
    (root, lane)
}

fn assert_unique(lane: &Path, file: &str, sym: &str, target_file: &str, target_sym: &str) {
    assert_eq!(
        call_targets(lane, file, sym),
        vec![(target_file.to_string(), target_sym.to_string(), "UniqueGlobal".to_string())],
        "{file}::{sym}"
    );
}

fn assert_none(lane: &Path, file: &str, sym: &str) {
    let got = call_targets(lane, file, sym);
    assert!(got.is_empty(), "{file}::{sym} must bind nothing: {got:?}");
}

/// Base site, C caller: three namesakes, one in its group.
#[cfg(unix)]
#[test]
fn a_delta_c_call_binds_the_c_like_base_declaration_uniquely() {
    let (root, lane) = staged_lane("c");
    assert_unique(&lane, "app/ns.c", "c_uses_shared", "lib/lib.c", "lib/lib.c::shared_name");
    let _ = fs::remove_dir_all(root);
}

/// Base site, C# caller.
#[cfg(unix)]
#[test]
fn a_delta_csharp_call_binds_the_csharp_base_declaration_uniquely() {
    let (root, lane) = staged_lane("cs");
    assert_unique(&lane, "app2/Use.cs", "V.go", "cs0/Sh.cs", "cs0/Sh.cs::Sh2.shared_name");
    let _ = fs::remove_dir_all(root);
}

/// Base site, Java callers: the namesake call binds Java's own, and a Java call
/// of a name only C declares binds nothing.
#[cfg(unix)]
#[test]
fn a_delta_java_call_binds_the_java_base_declaration_and_never_a_c_one() {
    let (root, lane) = staged_lane("java");
    assert_unique(&lane, "app3/Use.java", "U.go", "jv/Sh.java", "jv/Sh.java::Sh.shared_name");
    assert_none(&lane, "app4/JC.java", "JC.go");
    let _ = fs::remove_dir_all(root);
}

/// Delta-to-delta site: a delta C call of a name only a delta Java file
/// declares binds nothing; a delta Java call of it, and a delta C call of a
/// delta C declaration, bind uniquely.
#[cfg(unix)]
#[test]
fn a_delta_call_binds_another_delta_files_declaration_only_inside_its_group() {
    let (root, lane) = staged_lane("dd");
    assert_none(&lane, "d2/Call.c", "c_calls_dd");
    assert_unique(&lane, "d3/Call.java", "JD.go", "d1/Decl.java", "d1/Decl.java::Decl.dd_name");
    assert_unique(&lane, "d5/Call2.c", "c_calls_ddc", "d4/Decl2.c", "d4/Decl2.c::dd_c");
    let _ = fs::remove_dir_all(root);
}
