//! This crate exists so a consumer can take the credential table without
//! taking anything else. GitPulse links it beside MarkDev's highlighter, and a
//! dependency that pulled tree-sitter — as `dc-verify` does — would make the
//! two fail to resolve together, which is the failure the split removed.

/// Tables that give the library a dependency of its own. `[dev-dependencies]`
/// is not among them: tests do not ship.
const DEPENDENCY_TABLES: &[&str] = &["dependencies", "build-dependencies"];

/// Every entry of `manifest` that declares a library or build dependency.
fn dependency_entries(manifest: &str) -> Vec<String> {
    let mut in_dependency_table = false;
    let mut entries = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let table = header.trim();
            // `[dependencies.foo]` and `[target.'cfg(..)'.dependencies]` are
            // dependencies too, written another way.
            in_dependency_table = DEPENDENCY_TABLES.iter().any(|t| {
                table == *t
                    || table.starts_with(&format!("{t}."))
                    || (table.starts_with("target.") && table.ends_with(&format!(".{t}")))
            });
            continue;
        }
        if in_dependency_table {
            entries.push(line.to_string());
        }
    }
    entries
}

#[test]
fn the_library_has_no_dependencies() {
    let entries = dependency_entries(include_str!("../Cargo.toml"));
    assert!(
        entries.is_empty(),
        "dc-redact must stay dependency-free so it can be linked beside any tree-sitter; \
         found {entries:?}"
    );
}

#[test]
fn the_check_sees_each_way_a_dependency_is_written() {
    // The guard above passes trivially if it never recognises an entry, so
    // show it every shape it exists to refuse — and one it must not.
    let manifest = r#"
[package]
name = "x"

[dependencies]
# a comment is not a dependency
tree-sitter = "0.25"

[dependencies.serde]
version = "1"

[build-dependencies]
cc = "1"

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[dev-dependencies]
proptest = "1"
"#;
    assert_eq!(
        dependency_entries(manifest),
        [
            "tree-sitter = \"0.25\"",
            "version = \"1\"",
            "cc = \"1\"",
            "libc = \"0.2\"",
        ]
    );
}
