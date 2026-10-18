//! Measurement, not a probe: with `DEVMAP_ATTACK4_EDGE_DIR` set, writes one
//! sorted edge listing per `rust/testdata/accuracy/python*` fixture so two
//! builds (103272ea and 042133bf) can be diffed. Unset, it only asserts the
//! listing is deterministic.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(String, String)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn collect(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "py") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.push((rel, text));
            }
        }
    }
}

fn listing(res: &ResolutionResult) -> String {
    let mut lines: Vec<String> = res
        .edges
        .iter()
        .map(|e| {
            format!(
                "{} -> {} [{:?}] {:?}",
                e.source_symbol, e.target_symbol, e.edge_kind, e.confidence
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

#[test]
fn python_fixture_edge_sets_are_deterministic_and_dumpable() {
    let accuracy = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy");
    let mut fixtures: Vec<_> = std::fs::read_dir(&accuracy)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("python") && e.path().is_dir())
        .collect();
    fixtures.sort_by_key(|e| e.path());
    assert!(!fixtures.is_empty());
    let dump_dir = std::env::var("DEVMAP_ATTACK4_EDGE_DIR").ok();
    for fixture in fixtures {
        let mut files = Vec::new();
        collect(&fixture.path(), &fixture.path(), &mut files);
        let first = listing(&resolve(&files));
        assert_eq!(first, listing(&resolve(&files)), "{:?}", fixture.path());
        if let Some(dir) = &dump_dir {
            let name = fixture.file_name().to_string_lossy().into_owned();
            std::fs::write(std::path::Path::new(dir).join(format!("{name}.txt")), first).unwrap();
        }
    }
}
