//! The corpus: the files `dcgrep` admitted into an index, restricted to the
//! source suffixes `dc-sparse-encode` encodes, so arms A, B and C rank the
//! same set.
//!
//! The list is read from the published slot's `files.bin` (a `u32` id, a `u16`
//! length and that many path bytes, repeated), not guessed from a walk of our
//! own, so it is exactly what `dcgrep` admitted.

use std::fs;
use std::path::Path;

/// `dc-sparse-encode`'s `SOURCE_SUFFIXES`. Kept identical on purpose.
pub const SOURCE_SUFFIXES: &[&str] = &[
    ".c", ".cc", ".cpp", ".cs", ".css", ".go", ".h", ".hpp", ".html", ".java", ".js", ".json",
    ".jsx", ".kt", ".lua", ".md", ".mjs", ".php", ".proto", ".py", ".rb", ".rs", ".scala", ".sh",
    ".sql", ".swift", ".toml", ".ts", ".tsx", ".txt", ".vue", ".yaml", ".yml", ".zsh",
];

/// Suffixes that count as code for commit-subject ground truth.
const CODE_SUFFIXES: &[&str] = &[
    ".rs", ".go", ".py", ".swift", ".kt", ".kts", ".js", ".mjs", ".ts", ".tsx", ".sh", ".sql",
];

pub fn has_source_suffix(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    SOURCE_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

pub fn has_code_suffix(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    CODE_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

/// Paths that are tests, fixtures, docs, lockfiles or generated output. A
/// commit's ground truth never includes them; the corpus still does.
pub fn is_excluded_ground_truth(path: &str) -> bool {
    const DIRS: &[&str] = &[
        "tests", "test", "__tests__", "testdata", "fixtures", "fixture", "docs", "vendor",
        "generated", "results", "node_modules", "target", "dist", ".github", ".devcouncil",
        ".claude", ".codex", "snapshots", "golden",
    ];
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_some() && DIRS.contains(&part) {
            return true;
        }
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    name.ends_with("_test.go")
        || name.ends_with(".md")
        || name.ends_with(".lock")
        || name == "package-lock.json"
        || name.contains(".golden")
        || name.contains(".snap")
        || stem == "tests"
        || stem.ends_with("_tests")
        || stem.ends_with("_test")
        || stem.starts_with("test_")
}

/// Read the admitted paths from a repository whose `.devcouncil/dcgrep` slot
/// `dcgrep index` published.
pub fn read_slot_files(root: &Path) -> Result<Vec<String>, String> {
    let cache = root.join(".devcouncil").join("dcgrep");
    let pointer = fs::read_to_string(cache.join("current.json"))
        .map_err(|e| format!("{}: {e}", cache.join("current.json").display()))?;
    let pointer: serde_json::Value =
        serde_json::from_str(&pointer).map_err(|e| format!("current.json: {e}"))?;
    let slot = pointer["slot"].as_str().ok_or("current.json has no slot")?;
    let expected = pointer["files_indexed"].as_u64().ok_or("current.json has no files_indexed")?;
    let bytes = fs::read(cache.join(slot).join("files.bin"))
        .map_err(|e| format!("files.bin: {e}"))?;
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let header = bytes.get(at..at + 6).ok_or("files.bin: truncated record header")?;
        let len = u16::from_le_bytes([header[4], header[5]]) as usize;
        let raw = bytes
            .get(at + 6..at + 6 + len)
            .ok_or("files.bin: truncated path")?;
        out.push(String::from_utf8(raw.to_vec()).map_err(|_| "files.bin: path is not utf-8")?);
        at += 6 + len;
    }
    if out.len() as u64 != expected {
        return Err(format!(
            "files.bin holds {} paths but current.json says {expected}; refusing to guess",
            out.len()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_docs_and_fixtures_are_not_ground_truth() {
        for p in [
            "rust/dc-grep/tests/x.rs",
            "rust/testdata/a.rs",
            "backend/go_orchestrator/policy/file_test.go",
            "docs/x.md",
            "rust/Cargo.lock",
            "rust/devmap-query/src/doc_drift_tests.rs",
            "rust/devmap-extract/src/treesitter/tests.rs",
            "benchmarks/results/run.py",
        ] {
            assert!(is_excluded_ground_truth(p), "{p}");
        }
        for p in [
            "rust/dc-grep/src/lib.rs",
            "backend/go_orchestrator/policy/file.go",
            "rust/devmap-cli/src/commands/preview.rs",
        ] {
            assert!(!is_excluded_ground_truth(p), "{p}");
        }
    }

    #[test]
    fn a_directory_named_like_a_file_is_not_a_test_directory() {
        // The last component is the file, so a file called `test` is judged by
        // its name rules, not the directory rules.
        assert!(!is_excluded_ground_truth("rust/dc-grep/src/test"));
    }

    #[test]
    fn suffixes_are_case_insensitive() {
        assert!(has_source_suffix("A/B.RS"));
        assert!(!has_source_suffix("a/b.png"));
        assert!(has_code_suffix("x.go") && !has_code_suffix("x.toml"));
    }
}
