//! Measurement, not a probe: with `DEVMAP_ATTACK5_EDGE_DIR` set, writes one
//! sorted edge listing per NON-Python `rust/testdata/accuracy/*` fixture, plus
//! synthetic Ruby / Kotlin / TypeScript diamond and opaque-base self-call
//! shapes, so bf936983 and 5de18fd1 can be diffed byte for byte. Unset, it only
//! asserts the listings are deterministic.

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
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| !matches!(e, "py" | "json" | "md" | "tsv" | "txt" | "lock"))
        {
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

fn synthetic() -> Vec<(String, Vec<(String, String)>)> {
    let s = |p: &str, t: &str| (p.to_string(), t.to_string());
    vec![
        (
            "synthetic-ruby".into(),
            vec![s(
                "a.rb",
                "module M\n  def m\n    1\n  end\nend\nclass A\n  def m\n    2\n  end\nend\nclass Z\nend\nclass B < A\nend\nclass C < Z\n  def m\n    3\n  end\nend\nclass D < B\n  include M\n  def go\n    self.m\n    m\n  end\nend\nclass E < Unknown\n  def go\n    self.m\n  end\nend\n",
            )],
        ),
        (
            "synthetic-kotlin".into(),
            vec![s(
                "a.kt",
                "interface I { fun m(): Int { return 1 } }\nopen class A { open fun m(): Int = 2 }\nopen class B : A()\nclass D : B(), I {\n    fun go(): Int { return this.m() + m() }\n}\nclass E : Unknown() {\n    fun go(): Int = m()\n}\nopen class C : A() { override fun m(): Int = 3 }\nclass F : C(), I { fun go(): Int = this.m() }\n",
            )],
        ),
        (
            "synthetic-ts".into(),
            vec![s(
                "a.ts",
                "class A { m() { return 1; } }\nclass Z {}\nclass B extends A {}\nclass C extends Z { m() { return 2; } }\nclass D extends B implements C {\n  go() { return this.m(); }\n}\nclass E extends (Unknown as any) {\n  go() { return this.m(); }\n}\nclass F extends C { go() { const f = () => this.m(); return f(); } }\n",
            )],
        ),
    ]
}

#[test]
fn non_python_fixture_edge_sets_are_deterministic_and_dumpable() {
    let accuracy = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy");
    let mut fixtures: Vec<_> = std::fs::read_dir(&accuracy)
        .unwrap()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with("python") && e.path().is_dir())
        .collect();
    fixtures.sort_by_key(|e| e.path());
    assert!(!fixtures.is_empty());
    let dump_dir = std::env::var("DEVMAP_ATTACK5_EDGE_DIR").ok();
    let mut sets: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for fixture in fixtures {
        let mut files = Vec::new();
        collect(&fixture.path(), &fixture.path(), &mut files);
        sets.push((fixture.file_name().to_string_lossy().into_owned(), files));
    }
    sets.extend(synthetic());
    for (name, files) in sets {
        let first = listing(&resolve(&files));
        assert_eq!(first, listing(&resolve(&files)), "{name}");
        if let Some(dir) = &dump_dir {
            std::fs::write(std::path::Path::new(dir).join(format!("{name}.txt")), first).unwrap();
        }
    }
}
