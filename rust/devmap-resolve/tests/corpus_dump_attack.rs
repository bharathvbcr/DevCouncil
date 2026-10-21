//! Not an assertion: dumps resolver edges of a Python corpus for an A/B diff.
//! CORPUS_ROOT=<dir> CORPUS_OUT=<file>
use devmap_extract::extract_file;
use devmap_resolve::Resolver;
use std::fmt::Write as _;

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if matches!(
                name.as_str(),
                ".venv" | "venv" | "site-packages" | "node_modules" | "target" | "__pycache__" | ".git" | "build" | "dist" | ".tox"
            ) || name.starts_with("target-")
            {
                continue;
            }
            walk(&p, out);
        } else if name.ends_with(".py") {
            out.push(p);
        }
    }
}

#[test]
fn corpus_dump_attack() {
    let (Ok(root), Ok(outp)) = (std::env::var("CORPUS_ROOT"), std::env::var("CORPUS_OUT")) else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let mut extractions = Vec::new();
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else { continue };
        if src.len() > 400_000 {
            continue;
        }
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().to_string();
        extractions.push(extract_file(&rel, &src));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let mut lines: Vec<String> = result
        .edges
        .iter()
        .map(|e| {
            format!(
                "{:?}\t{}\t{}\t{:?}",
                e.edge_kind, e.source_symbol, e.target_symbol, e.confidence
            )
        })
        .collect();
    lines.sort();
    let mut s = String::new();
    for l in &lines {
        writeln!(s, "{l}").unwrap();
    }
    std::fs::write(&outp, s).unwrap();
    println!("files={} edges={}", extractions.len(), lines.len());
}
