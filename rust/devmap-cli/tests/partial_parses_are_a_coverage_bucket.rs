//! A file whose tree holds `ERROR`/`MISSING` nodes is a coverage gap of its
//! own — `partial_parse` — named with the byte ranges lost, and `dead`/`impact`
//! say so when the file could hold calls.
//!
//! The audit's reproductions (2026-10-09, round 2): in a `.tsx`,
//! `type R<T> = import('m').Ref<T>;` erases every later declaration —
//! ScholarLM's `frontend/contexts/AuthContext.tsx` lost `AuthProvider` and
//! `useAuth`, one of 55 partially parsed frontend files, none of them in
//! `status.coverage_gaps`. Kotlin declarations vanish the same way under
//! grammar mis-parses; the fixture here is `suspend { … }` — the standard
//! library's `suspend` function with a trailing lambda, valid Kotlin the linked
//! grammar turns into an error region that swallows the rest of the file.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "devmap {args:?} stdout is not JSON ({error}): {stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-partial-parse-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn partial_rows(status: &serde_json::Value) -> Vec<(String, String)> {
    status["coverage_gaps"]["partial_parse"]["paths"]
        .as_array()
        .unwrap_or_else(|| panic!("status carries a partial_parse bucket: {status}"))
        .iter()
        .map(|row| {
            (
                row["path"].as_str().unwrap().to_string(),
                row["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn the_ts_import_type_erasure_is_listed_and_caveats_dead_and_impact() {
    let root = scratch("tsx");
    std::fs::create_dir_all(root.join("contexts")).unwrap();
    std::fs::write(
        root.join("contexts/session.ts"),
        "export function readSession() {\n  return 1;\n}\n",
    )
    .unwrap();
    // `useAuth` is `readSession`'s only caller, and it sits after the line the
    // grammar cannot parse.
    std::fs::write(
        root.join("contexts/AuthContext.tsx"),
        "import { readSession } from './session';\n\
         type R<T> = import('m').Ref<T>;\n\
         export function AuthProvider() { return null; }\n\
         export function useAuth() { return readSession(); }\n",
    )
    .unwrap();
    let _ = json(&root, &["--json", "build", "."]);

    let status = json(&root, &["--json", "status"]);
    let rows = partial_rows(&status);
    let (path, reason) = rows
        .iter()
        .find(|(path, _)| path == "contexts/AuthContext.tsx")
        .unwrap_or_else(|| panic!("the partially parsed file is listed: {rows:?}"));
    assert!(
        reason.contains("byte(s) lost") && reason.contains(".."),
        "{path}: the reason names the byte ranges lost: {reason}"
    );
    assert!(
        rows.iter().all(|(path, _)| path != "contexts/session.ts"),
        "a clean file is not listed: {rows:?}"
    );

    let dead = json(&root, &["--json", "dead"]);
    let caveat = dead["walk_incomplete"].as_str().unwrap_or("");
    assert!(
        caveat.contains("ERROR/MISSING"),
        "dead names the partial parse as a reason its list is a lower bound: {dead}"
    );
    if let Some(finding) = dead["items"].as_array().unwrap().iter().find(|item| {
        item["symbol_name"]
            .as_str()
            .is_some_and(|name| name.ends_with("readSession"))
    }) {
        assert!(
            finding["confidence"].as_f64().unwrap() < 0.9,
            "readSession's only caller was lost to the parse: {finding}"
        );
    }
    let impact = json(&root, &["--json", "impact", "readSession"]);
    assert!(
        impact["walk_incomplete"]
            .as_str()
            .is_some_and(|reason| reason.contains("ERROR/MISSING")),
        "impact carries the same caveat: {impact}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_kotlin_mis_parse_is_listed_with_its_byte_range() {
    let root = scratch("kotlin");
    std::fs::write(
        root.join("Lib.kt"),
        "package a\n\nfun libHelper(): Int = 1\n",
    )
    .unwrap();
    std::fs::write(
        root.join("App.kt"),
        "package a\n\nfun app() {\n    val c = suspend { 1 }\n    libHelper()\n}\n",
    )
    .unwrap();
    let _ = json(&root, &["--json", "build", "."]);
    let status = json(&root, &["--json", "status"]);
    let rows = partial_rows(&status);
    let (_, reason) = rows
        .iter()
        .find(|(path, _)| path == "App.kt")
        .unwrap_or_else(|| panic!("the Kotlin mis-parse is listed: {rows:?}"));
    assert!(reason.contains("byte(s) lost"), "{reason}");
    assert!(
        rows.iter().all(|(path, _)| path != "Lib.kt"),
        "a clean file is not listed: {rows:?}"
    );
    let impact = json(&root, &["--json", "impact", "libHelper"]);
    assert!(
        impact["walk_incomplete"]
            .as_str()
            .is_some_and(|reason| reason.contains("ERROR/MISSING")),
        "an empty blast radius over a mis-parsed caller says so: {impact}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The `start..end` byte ranges a `partial_parse` reason names.
fn lost_ranges(reason: &str) -> Vec<(usize, usize)> {
    let listed = reason.split("byte(s) lost: ").nth(1).unwrap_or("");
    // The ranges end at the first `;` (a `charged: ..` note follows) or at the
    // `, and N more region(s)` tail.
    let listed = listed.split(';').next().unwrap_or("");
    listed
        .split(", ")
        .filter_map(|part| {
            let (start, end) = part.split_once("..")?;
            Some((start.parse().ok()?, end.parse().ok()?))
        })
        .collect()
}

fn covers(ranges: &[(usize, usize)], offset: usize) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= offset && offset < *end)
}

/// ft-5ad86836 AC3. The grammar rejects a one-line `class C { companion object
/// { .. } }` (an `ERROR` node, not a clean tree). extract-v98 repairs that shape
/// with a same-length separator, so it is no longer the example of a loss that is
/// not recovered; the same body with no space before its last `}` has no room
/// for a separator and still is. `C`, its companion function and the top-level
/// function after it are not extracted, and the file must be listed in
/// `coverage_gaps.partial_parse` with a range that covers every one of them: the
/// loss is reported even though the declaration is not recovered.
#[test]
fn a_one_line_companion_object_loss_is_listed_with_a_range_covering_it() {
    let root = scratch("companion");
    let source = "package a\n\nclass C { companion object { fun f() = 1 }}\nfun top() = 2\n";
    std::fs::write(root.join("Comp.kt"), source).unwrap();
    std::fs::write(root.join("Clean.kt"), "package a\n\nfun tidy() = 3\n").unwrap();
    let _ = json(&root, &["--json", "build", "."]);
    let status = json(&root, &["--json", "status"]);
    let rows = partial_rows(&status);
    let (_, reason) = rows
        .iter()
        .find(|(path, _)| path == "Comp.kt")
        .unwrap_or_else(|| panic!("the one-line companion mis-parse is listed: {rows:?}"));
    let ranges = lost_ranges(reason);
    for lost in ["class C", "fun f()", "fun top()"] {
        let offset = source.find(lost).unwrap();
        assert!(
            covers(&ranges, offset),
            "`{lost}` (byte {offset}) is not inside a listed lost range {ranges:?}: {reason}"
        );
    }
    assert!(
        rows.iter().all(|(path, _)| path != "Clean.kt"),
        "a clean file is not listed: {rows:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// extract-v98 row 16. The same shape with a space before the last `}` is
/// repaired, so it is not a coverage gap at all and its declarations are found.
#[test]
fn a_one_line_companion_object_with_room_for_a_separator_is_repaired_not_listed() {
    let root = scratch("companion-repaired");
    let source = "package a\n\nclass C { companion object { fun f() = 1 } }\nfun top() = 2\n";
    std::fs::write(root.join("Comp.kt"), source).unwrap();
    let _ = json(&root, &["--json", "build", "."]);
    let status = json(&root, &["--json", "status"]);
    let rows = partial_rows(&status);
    assert!(
        rows.iter().all(|(path, _)| path != "Comp.kt"),
        "a repaired file is not listed as a partial parse: {rows:?}"
    );
    let found = json(&root, &["--json", "search", "top"]);
    assert!(
        found["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["symbol_name"]
                    .as_str()
                    .is_some_and(|name| name.ends_with("top"))
            })),
        "the declaration after the class is found: {found}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// ft-5ad86836 AC4 (ChronosTheme.kt:136 shape). An extension function on a
/// library type in a file that also holds an `ERROR` region is either emitted,
/// or the file is reported partial with a range that covers it. It is never
/// silently absent from a file that looks clean.
#[test]
fn an_extension_function_beside_a_kotlin_error_region_is_emitted_or_covered() {
    let root = scratch("extension");
    let source = "package a\n\nclass C { companion object { fun f() = 1 } }\n\
                  fun ColorScheme.withChronosReadableContentColors(): ColorScheme = copy()\n";
    std::fs::write(root.join("Theme.kt"), source).unwrap();
    let _ = json(&root, &["--json", "build", "."]);
    let found = json(
        &root,
        &["--json", "search", "withChronosReadableContentColors"],
    );
    // Only the result items count: a miss's explanation can name the function
    // (the partial-parse reason), which would pass without proving emission.
    let emitted = found["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["symbol_name"]
                .as_str()
                .is_some_and(|name| name.ends_with("withChronosReadableContentColors"))
        })
    });
    if emitted {
        eprintln!("AC4 branch: EMITTED");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    eprintln!("AC4 branch: not emitted, checking covered-by-partial");
    let status = json(&root, &["--json", "status"]);
    let rows = partial_rows(&status);
    let (_, reason) = rows.iter().find(|(path, _)| path == "Theme.kt").unwrap_or_else(|| {
        panic!("the extension function is absent from a file not listed as partial: {rows:?} / {found}")
    });
    let offset = source.find("fun ColorScheme.").unwrap();
    assert!(
        covers(&lost_ranges(reason), offset),
        "the extension function (byte {offset}) is absent and not inside a listed range: {reason}"
    );
    eprintln!("AC4 branch: COVERED-BY-PARTIAL ({reason})");
    let _ = std::fs::remove_dir_all(&root);
}
