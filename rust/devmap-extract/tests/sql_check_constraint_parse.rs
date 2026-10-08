//! `CHECK(json_valid(body))` and the parse outcome of the repository's SQL.
//!
//! `docs/devmap/BENCHMARK_HARDENING_AUDIT.md` recorded that all eight `.sql`
//! files parsed only partially (23 error ranges) and that a script containing
//! `CHECK(json_valid(body))` — which SQLite executes — made the parser report
//! an error. Reproduced 2026-10-08: the linked SQL grammar reports exactly the
//! `CHECK(...)` text as an error range when the check calls a function
//! (`CHECK(revision>0)` parses), and nothing else in the statement or after it
//! is lost. Fixing it means changing the grammar, a dependency change nobody
//! has authorised, so this pins the gap as a *confined* one: if a grammar bump
//! repairs it, or widens it to swallow a declaration, this fails and the audit
//! sentence has to be rewritten either way.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

const MINIMAL: &str = "\
CREATE TABLE docs (
    id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK(revision>0),
    body TEXT NOT NULL CHECK(json_valid(body))
);
CREATE INDEX docs_revision ON docs(revision);
CREATE TABLE after_check (id INTEGER PRIMARY KEY);
";

/// The text of every error range, or a panic naming the outcome if the file
/// did not parse with error ranges at all.
fn error_texts<'a>(extraction: &Extraction, source: &'a str) -> Vec<&'a str> {
    match &extraction.parse_outcome {
        ParseOutcome::Clean => Vec::new(),
        ParseOutcome::Partial { error_ranges } => error_ranges
            .iter()
            .map(|range| &source[range.start_byte..range.end_byte])
            .collect(),
        other => panic!("{}: {other:?}", extraction.file_path),
    }
}

fn declared(extraction: &Extraction) -> Vec<&str> {
    extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.name.as_str())
        .collect()
}

/// Error ranges in the repository schema that are not the CHECK gap: SQLite
/// trigger bodies (`BEGIN … END`, `RAISE(ABORT, …)`) and `CREATE VIRTUAL TABLE
/// … USING fts5(…)`, which the linked grammar does not know.
const OTHER_ERRORS: usize = 10;

/// Written tables the extraction loses to those ranges: the FTS5 virtual
/// table itself, and the ordinary table written directly after the trigger
/// whose body the parser could not close.
const LOST: &[&str] = &[
    "dc-store/src/workbench/schema.sql: work_items_fts",
    "dc-store/src/workbench/schema.sql: work_events",
];

/// An error range the grammar is known to report: a column `CHECK` whose
/// expression calls a function.
fn is_function_check(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    compact.starts_with("CHECK(") && compact.ends_with(')') && compact[6..].contains('(')
}

#[test]
fn a_function_call_check_is_the_whole_error_and_swallows_nothing() {
    let extraction = extract_file("schema.sql", MINIMAL);
    assert_eq!(
        error_texts(&extraction, MINIMAL),
        vec!["CHECK(json_valid(body))"],
        "the grammar gap moved: if it is repaired, retire this pin and the \
         audit sentence; if it grew, it is no longer confined"
    );
    let tables = declared(&extraction);
    for table in ["docs", "after_check"] {
        assert!(
            tables.contains(&table),
            "`{table}` is missing, so the CHECK swallowed a statement: {tables:?}"
        );
    }
    assert!(
        !tables.contains(&"json_valid") && !tables.contains(&"body"),
        "a CHECK expression is not a declaration: {tables:?}"
    );
}

#[test]
fn a_comparison_check_parses_cleanly() {
    let source = "CREATE TABLE t (revision INTEGER NOT NULL CHECK(revision>0));\n";
    assert_eq!(extract_file("t.sql", source).parse_outcome, ParseOutcome::Clean);
}

/// The names a `CREATE [TEMP|VIRTUAL] TABLE|VIEW [IF NOT EXISTS] <name>` line
/// declares, read off the text — the authority the extraction is checked
/// against. Indexes and triggers are not symbols in this extractor at all, so
/// they are not counted as lost.
fn written_declarations(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(create) = words.iter().position(|word| word.eq_ignore_ascii_case("CREATE"))
        else {
            continue;
        };
        let mut rest = words[create + 1..].iter().copied().peekable();
        while rest
            .peek()
            .is_some_and(|word| ["UNIQUE", "VIRTUAL", "TEMP", "TEMPORARY"].iter().any(|k| word.eq_ignore_ascii_case(k)))
        {
            rest.next();
        }
        let Some(kind) = rest.next() else { continue };
        if !["TABLE", "VIEW"].iter().any(|k| kind.eq_ignore_ascii_case(k)) {
            continue;
        }
        let mut name = rest.next();
        if name.is_some_and(|word| word.eq_ignore_ascii_case("IF")) {
            rest.next();
            rest.next();
            name = rest.next();
        }
        if let Some(name) = name {
            let name: String = name
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    names
}

/// The repository's schema files: which written declarations the extraction
/// loses, and how many error ranges are the function-call CHECK gap versus
/// SQLite-only statements (trigger bodies, `RAISE`, `USING fts5`) the linked
/// grammar does not know. Pinned as counts so a grammar change in either
/// direction is noticed.
#[test]
fn the_repository_schema_gaps_are_measured_and_pinned() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut check_errors = 0;
    let mut other_errors = 0;
    let mut written = 0;
    let mut lost: Vec<String> = Vec::new();
    for relative in [
        "dc-store/src/workbench/attention.sql",
        "dc-store/src/workbench/automation.sql",
        "dc-store/src/workbench/decisions.sql",
        "dc-store/src/workbench/enhancements.sql",
        "dc-store/src/workbench/notifications.sql",
        "dc-store/src/workbench/runs.sql",
        "dc-store/src/workbench/schema.sql",
        "tools/fanout.sql",
    ] {
        let path = root.join(relative);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let extraction = extract_file(relative, &source);
        for text in error_texts(&extraction, &source) {
            if is_function_check(text) {
                check_errors += 1;
            } else {
                other_errors += 1;
            }
        }
        let symbols = declared(&extraction);
        for name in written_declarations(&source) {
            written += 1;
            if !symbols.contains(&name.as_str()) {
                lost.push(format!("{relative}: {name}"));
            }
        }
    }
    eprintln!(
        "{written} written declarations, {} lost: {lost:?}; {check_errors} \
         function-call CHECK error ranges, {other_errors} other error ranges",
        lost.len()
    );
    assert_eq!(check_errors, 13, "the function-call CHECK gap changed size");
    assert!(written > 0, "the declaration reader found nothing to check");
    assert_eq!(other_errors, OTHER_ERRORS, "the SQLite-dialect gap changed size");
    assert_eq!(lost, LOST, "the tables the SQLite-dialect gap loses changed");
}

