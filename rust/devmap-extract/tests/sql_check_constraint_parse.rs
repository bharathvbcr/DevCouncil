//! `CHECK(json_valid(body))` and the parse outcome of the repository's SQL.
//!
//! `docs/archive/devmap/BENCHMARK_HARDENING_AUDIT.md` recorded that all eight `.sql`
//! files parsed only partially and that `CHECK(json_valid(body))` — which
//! SQLite executes — made the parser report an error. The linked grammar still
//! reports exactly that `CHECK(...)` text when the check calls a function
//! (`CHECK(revision>0)` parses). That confined gap is a grammar limitation, not
//! a dependency this crate is allowed to take.
//!
//! A different gap is not confined: `CREATE VIRTUAL TABLE ... USING fts5` and
//! a trigger body are ERROR spans that swallow the next `CREATE TABLE`, so
//! the relation is absent or wears the span of the broken statement in front
//! of it. Those relations are recovered from the statement text. This file
//! pins both: the CHECK text stays an error, and every written table or view
//! is a symbol.

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
        ParseOutcome::Partial { error_ranges, .. } => error_ranges
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

/// Written tables the extraction loses. Empty: a dialect ERROR no longer
/// drops the relation that follows it, and a virtual table is a symbol.
const LOST: &[&str] = &[];

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
    assert_eq!(
        extract_file("t.sql", source).parse_outcome,
        ParseOutcome::Clean
    );
}

/// The names a `CREATE [TEMP|VIRTUAL] TABLE|VIEW [IF NOT EXISTS] <name>` line
/// declares, read off the text — the authority the extraction is checked
/// against. Indexes and triggers are not symbols in this extractor at all, so
/// they are not counted as lost.
fn written_declarations(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(create) = words
            .iter()
            .position(|word| word.eq_ignore_ascii_case("CREATE"))
        else {
            continue;
        };
        let mut rest = words[create + 1..].iter().copied().peekable();
        while rest.peek().is_some_and(|word| {
            ["UNIQUE", "VIRTUAL", "TEMP", "TEMPORARY"]
                .iter()
                .any(|k| word.eq_ignore_ascii_case(k))
        }) {
            rest.next();
        }
        let Some(kind) = rest.next() else { continue };
        if !["TABLE", "VIEW"]
            .iter()
            .any(|k| kind.eq_ignore_ascii_case(k))
        {
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
    assert_eq!(
        other_errors, OTHER_ERRORS,
        "the SQLite-dialect gap changed size"
    );
    assert_eq!(
        lost, LOST,
        "the tables the SQLite-dialect gap loses changed"
    );
}

/// A relation the grammar dropped only because a dialect ERROR swallowed it
/// is a symbol, with the identity that statement has when parsed alone.
#[test]
fn a_swallowed_table_matches_the_statement_parsed_alone() {
    let swallowed = "\
CREATE VIRTUAL TABLE docs_fts USING fts5(title, body);
CREATE TABLE work_events (
    sequence INTEGER PRIMARY KEY,
    payload TEXT NOT NULL CHECK(json_valid(payload))
);
";
    let statement = "\
CREATE TABLE work_events (
    sequence INTEGER PRIMARY KEY,
    payload TEXT NOT NULL CHECK(json_valid(payload))
);
";
    let offset = swallowed
        .find(statement)
        .expect("statement sits in the file");
    let alone = extract_file("schema.sql", statement);
    let full = extract_file("schema.sql", swallowed);
    let expect = relation(&alone, "work_events");
    let got = relation(&full, "work_events");
    assert_eq!(got.kind, expect.kind);
    assert_eq!(got.qualified_name, expect.qualified_name);
    assert_eq!(got.is_exported, expect.is_exported);
    assert_eq!(got.parent_symbol, expect.parent_symbol);
    assert_eq!(got.span.start_byte, offset + expect.span.start_byte);
    assert_eq!(got.span.end_byte, offset + expect.span.end_byte);
    assert_eq!(
        full.symbols
            .iter()
            .filter(|symbol| symbol.name == "work_events")
            .count(),
        1
    );
}

/// `CREATE VIRTUAL TABLE ... USING fts5` never becomes a `create_table` node.
/// The relation is still the symbol a code map looks up.
#[test]
fn a_virtual_table_is_a_struct_symbol_and_not_a_second_copy() {
    let source = "CREATE VIRTUAL TABLE work_items_fts USING fts5(title, description);\n";
    let extraction = extract_file("schema.sql", source);
    let symbol = relation(&extraction, "work_items_fts");
    assert_eq!(symbol.kind, SymbolKind::Struct);
    assert_eq!(symbol.qualified_name, "schema.sql::work_items_fts");
    let span = &source[symbol.span.start_byte..symbol.span.end_byte];
    assert!(span.contains("work_items_fts"), "{span}");
    assert!(span.contains("fts5"), "{span}");
    assert_eq!(
        extraction
            .symbols
            .iter()
            .filter(|s| s.name == "work_items_fts")
            .count(),
        1
    );
}

/// The repository schema is the corpus that reported the hole.
#[test]
fn the_workbench_schema_keeps_work_events_and_the_fts_table() {
    let path = "dc-store/src/workbench/schema.sql";
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(path),
    )
    .expect(path);
    let extraction = extract_file(path, &source);
    for name in ["work_events", "work_items_fts"] {
        let symbol = relation(&extraction, name);
        assert_eq!(symbol.kind, SymbolKind::Struct, "{name}");
        assert_eq!(symbol.qualified_name, format!("{path}::{name}"));
        assert!(source.is_char_boundary(symbol.span.start_byte));
        assert!(source.is_char_boundary(symbol.span.end_byte));
    }
    assert!(
        !declared(&extraction).contains(&"work_events_entity"),
        "an index is not a symbol"
    );
    assert!(
        !declared(&extraction).contains(&"work_items_insert"),
        "a trigger is not a symbol"
    );
}

#[test]
fn recovery_does_not_invent_relations_the_source_did_not_declare() {
    let source = "\
CREATE TABLE kept (note TEXT DEFAULT 'CREATE TABLE fake (id INT);');
-- CREATE TABLE commented (id INT);
CREATE VIRTUAL TABLE docs_fts USING fts5(title);
CREATE TRIGGER tr AFTER INSERT ON kept BEGIN
    SELECT 'CREATE TABLE quoted_in_trigger (id INT);';
    CREATE TABLE inner_fake (id INT);
END;
CREATE FUNCTION f() RETURNS int AS $$ CREATE TABLE dollar_fake (id INT); $$ LANGUAGE sql;
CREATE TABLE after_all (id INT);
CREATE TABLE \"Weird Name\" (id INT);
CREATE TABLE \"caf\u{e9}\" (id INT);
CREATE VIRTUAL TABLE IF NOT EXISTS main.docs_fts2 USING fts5(body);
CREATE TABLE literal_semi (body TEXT DEFAULT 'a;b');
CREATE TABLE nope (id INT
";
    let extraction = extract_file("schema.sql", source);
    let names = declared(&extraction);
    for present in [
        "kept",
        "after_all",
        "Weird Name",
        "caf\u{e9}",
        "docs_fts",
        "docs_fts2",
        "literal_semi",
    ] {
        assert!(names.contains(&present), "missing {present}: {names:?}");
    }
    // A stored function is a callable the extractor already records. A table
    // that exists only as text inside that body, a trigger body, a comment,
    // a string, or an unterminated statement is not a schema object.
    assert_eq!(relation(&extraction, "f").kind, SymbolKind::Function);
    for absent in [
        "fake",
        "commented",
        "quoted_in_trigger",
        "inner_fake",
        "dollar_fake",
        "nope",
        "tr",
        "docs_fts2_nope",
    ] {
        assert!(!names.contains(&absent), "invented {absent}: {names:?}");
    }
    assert_eq!(
        relation(&extraction, "Weird Name").qualified_name,
        "schema.sql::Weird Name"
    );
    let semi = relation(&extraction, "literal_semi");
    let slice = &source[semi.span.start_byte..semi.span.end_byte];
    assert!(
        slice.contains("'a;b'"),
        "the literal semicolon split the statement: {slice}"
    );
    assert_eq!(names.iter().filter(|name| **name == "kept").count(), 1);
}

#[test]
fn an_empty_sql_file_and_a_clean_table_are_unchanged() {
    let empty = extract_file("empty.sql", "");
    assert!(declared(&empty).is_empty(), "{:?}", declared(&empty));
    let clean = "CREATE TABLE t (id INTEGER PRIMARY KEY);\n";
    let extraction = extract_file("t.sql", clean);
    assert_eq!(extraction.parse_outcome, ParseOutcome::Clean);
    assert_eq!(declared(&extraction), vec!["t"]);
}

#[test]
fn many_virtual_tables_and_many_swallowed_tables_all_come_back() {
    let mut source = String::from("CREATE VIRTUAL TABLE lead USING fts5(x);\n");
    for index in 0..40 {
        source.push_str(&format!("CREATE VIRTUAL TABLE v{index} USING fts5(x);\n"));
        source.push_str(&format!("CREATE TABLE t{index} (id INT);\n"));
    }
    let extraction = extract_file("bulk.sql", &source);
    let names = declared(&extraction);
    assert!(names.contains(&"lead"));
    assert!(names.contains(&"v39"));
    assert!(names.contains(&"t39"));
    assert_eq!(names.len(), 1 + 40 + 40, "{names:?}");
}

/// Names, spans and non-relations the grammar gets wrong together. Each arm
/// has failed on its own: a quote character left on the name, a view wearing
/// the virtual table in front of it, a multibyte character panicking the
/// keyword scan, a table that exists only inside a routine or a block, and a
/// statement that never terminates.
#[test]
fn hostile_statements_keep_real_relations_and_drop_the_rest() {
    let source = "\
CREATE VIRTUAL TABLE lead USING fts5(x);
create table lower_case (id int);
CREATE OR REPLACE TEMP VIEW shown AS SELECT 1;
CREATE MATERIALIZED VIEW monthly AS SELECT 1;
CREATE TABLE analytics.events (id INT);
CREATE TABLE \"a\"\"b\" (id INT);
CREATE TABLE [Order Details] (id INT);
CREATE TABLE t (CREATÉ INT);
CREATE TABLE after_unicode (id INT);
CREATE FUNCTION g() RETURNS int AS $body$
    CREATE TABLE inside_fn (id INT);
$body$ LANGUAGE sql;
BEGIN
    CREATE TABLE inside_block (id INT);
END;
/* CREATE TABLE inside_comment (id INT);
CREATE TABLE nope_unclosed (id INT
";
    let extraction = extract_file("hostile.sql", source);
    let names = declared(&extraction);
    for present in [
        "lead",
        "lower_case",
        "shown",
        "monthly",
        "events",
        "a\"b",
        "Order Details",
        "t",
        "after_unicode",
        "g",
    ] {
        assert!(names.contains(&present), "missing {present}: {names:?}");
    }
    for absent in [
        "inside_fn",
        "inside_block",
        "inside_comment",
        "nope_unclosed",
        "a",
        "Details",
        "Order",
    ] {
        assert!(!names.contains(&absent), "invented {absent}: {names:?}");
    }
    assert_eq!(relation(&extraction, "g").kind, SymbolKind::Function);
    for name in ["lower_case", "shown", "monthly", "events", "after_unicode"] {
        let symbol = relation(&extraction, name);
        let end = symbol.span.end_byte.min(source.len());
        let slice = source.get(symbol.span.start_byte..end).unwrap_or("");
        assert!(slice.contains(name), "{name} span is {slice:?}");
        assert!(
            !slice.contains("fts5"),
            "{name} wore the virtual table's span: {slice:?}"
        );
    }
    assert_eq!(
        relation(&extraction, "a\"b").qualified_name,
        "hostile.sql::a\"b"
    );
}

#[test]
fn a_quoted_name_on_a_clean_statement_loses_the_quotes() {
    let source = "CREATE TABLE \"Weird Name\" (id INT);\n";
    let extraction = extract_file("q.sql", source);
    let symbol = relation(&extraction, "Weird Name");
    assert_eq!(symbol.kind, SymbolKind::Struct);
    assert_eq!(symbol.qualified_name, "q.sql::Weird Name");
}

fn relation<'a>(
    extraction: &'a Extraction,
    name: &str,
) -> &'a devmap_extract::model::ExtractedSymbol {
    extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}: {:?}", declared(extraction)))
}
