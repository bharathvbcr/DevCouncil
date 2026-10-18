//! Attack probe (not a regression test): resolves every non-Swift accuracy
//! fixture tree and dumps the sorted edge set to `edges-dump.txt` at the
//! worktree root, so two builds can be diffed.
//! Run: cargo test -p devmap-resolve --test non_swift_edge_compare -- --ignored

use devmap_extract::extract_tree;
use devmap_resolve::Resolver;
use std::fmt::Write as _;
use std::path::Path;

#[test]
#[ignore]
fn dump_edges() {
    let fixtures = Path::new("/Users/bharath/Code/devtools/DevCouncil/.claude/worktrees/agent-a345ecfa01fea3074/rust/testdata/accuracy");
    let mut out = String::new();
    let mut dirs: Vec<_> = std::fs::read_dir(fixtures)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if name == "swift" {
            continue;
        }
        let Ok(extractions) = extract_tree(&dir) else {
            writeln!(out, "=== {name}: extract failed").unwrap();
            continue;
        };
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let result = resolver.resolve_all(&extractions).unwrap();
        let mut lines: Vec<String> = result.edges.iter().map(|e| format!("{e:?}")).collect();
        lines.sort();
        writeln!(out, "=== {name}: {} files, {} edges", extractions.len(), lines.len()).unwrap();
        for line in lines {
            writeln!(out, "{line}").unwrap();
        }
    }
    let dest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../edges-dump.txt");
    std::fs::write(dest, out).unwrap();
}
