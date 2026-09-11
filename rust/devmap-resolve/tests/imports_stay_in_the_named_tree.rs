//! An import outside the indexed tree must never alias a file inside it.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

fn targets(files: &[(&str, &str)]) -> Vec<String> {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    assert!(
        !extractions[0].imports.is_empty(),
        "fixture must exercise import resolution"
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver
        .resolve_all(&extractions)
        .edges
        .into_iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Imports && edge.source_file == files[0].0)
        .map(|edge| edge.target_file)
        .collect()
}

#[test]
fn root_crossing_imports_do_not_alias_local_files() {
    for (source_path, source, target_path, target) in [
        (
            "src/app.ts",
            "import { helper } from '../../victim'; helper();",
            "victim.ts",
            "export function helper() {}",
        ),
        (
            "src/main.c",
            "#include \"../../victim.h\"\nint main(void) { return 0; }",
            "victim.h",
            "int helper(void);",
        ),
        (
            "src/main.rb",
            "require_relative '../../victim'\n",
            "victim.rb",
            "def helper; end",
        ),
        (
            "src/main.tf",
            "module \"victim\" { source = \"../../victim\" }",
            "victim/main.tf",
            "variable \"name\" {}",
        ),
    ] {
        let found = targets(&[(source_path, source), (target_path, target)]);
        assert!(
            found.is_empty(),
            "{source_path} imported an outside path but resolved to {found:?}"
        );
    }
}

#[test]
fn in_tree_parent_imports_keep_resolving() {
    let found = targets(&[
        (
            "src/app.ts",
            "import { helper } from '../victim'; helper();",
        ),
        ("victim.ts", "export function helper() {}"),
    ]);
    assert_eq!(found, ["victim.ts"]);
}

#[test]
fn an_absolute_include_cannot_become_a_relative_header() {
    let found = targets(&[
        (
            "src/main.c",
            "#include \"/victim.h\"\nint main(void) { return 0; }",
        ),
        ("src/victim.h", "int helper(void);"),
        ("victim.h", "int unrelated(void);"),
    ]);
    assert!(
        found.is_empty(),
        "an absolute header path aliased {found:?}"
    );
}
