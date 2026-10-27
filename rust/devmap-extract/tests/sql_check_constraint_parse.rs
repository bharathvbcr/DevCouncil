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
    // that exists only as text inside a comment, a string, or an unterminated
    // statement is not a schema object. `inner_fake` (trigger body) and
    // `dollar_fake` (function body) are not asserted either way: the grammar
    // parses both as tables outside every error range, and recovery removes a
    // symbol only where the grammar reported an error (see
    // `recovery_never_removes_a_symbol_the_grammar_parsed_outside_an_error`).
    assert_eq!(relation(&extraction, "f").kind, SymbolKind::Function);
    for absent in [
        "fake",
        "commented",
        "quoted_in_trigger",
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
    // `inside_fn`, `inside_block` and `inside_comment` are not asserted: the
    // grammar parses each as a table outside every error range, so recovery
    // leaves it (the ruling: removal only inside an error range).
    for absent in ["nope_unclosed", "a", "Details", "Order"] {
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

/// What the grammar alone says about `source`, before any recovery.
fn grammar_only(source: &str) -> Extraction {
    devmap_extract::treesitter::extract_treesitter("recover.sql", "sql", source)
}

fn bare(name: &str) -> &str {
    name.trim_matches(|c| c == '"' || c == '`')
}

/// The ruling: on a file the grammar parsed `Clean`, recovery never removes a
/// symbol, a call or a reference. Every grammar symbol is still there under
/// its (unquoted) name with its span, and no edge went missing.
fn assert_clean_file_loses_nothing(label: &str, source: &str) {
    let grammar = grammar_only(source);
    assert_eq!(
        grammar.parse_outcome,
        ParseOutcome::Clean,
        "{label}: the grammar no longer parses this cleanly, so the case proves nothing"
    );
    let full = extract_file("recover.sql", source);
    assert_eq!(full.parse_outcome, ParseOutcome::Clean, "{label}");
    for symbol in grammar
        .symbols
        .iter()
        .filter(|s| s.kind != SymbolKind::File)
    {
        assert!(
            full.symbols
                .iter()
                .any(|kept| kept.name == bare(&symbol.name)
                    && kept.span.start_byte == symbol.span.start_byte
                    && kept.span.end_byte == symbol.span.end_byte),
            "{label}: recovery removed `{}`: {:?}",
            symbol.name,
            declared(&full)
        );
    }
    assert_eq!(full.symbols.len(), grammar.symbols.len(), "{label}");
    assert_eq!(full.calls.len(), grammar.calls.len(), "{label}: calls");
    assert_eq!(
        full.references.len(),
        grammar.references.len(),
        "{label}: references"
    );
}

#[test]
fn a_clean_file_never_loses_a_symbol_to_recovery() {
    for (label, source) in [
        (
            "last statement has no semicolon",
            "CREATE TABLE a(id int);\nCREATE TABLE b(id int)",
        ),
        (
            "last statement has no semicolon and a trailing comment",
            "CREATE TABLE a(id int);\nCREATE TABLE b(id int) -- end\n",
        ),
        (
            "postgres transaction",
            "BEGIN;\nCREATE TABLE t1 (id INT);\nEND;\nCREATE TABLE t2 (id INT);\n",
        ),
        (
            "sqlite dump transaction",
            "BEGIN TRANSACTION;\nCREATE TABLE t1 (id INT);\nCOMMIT;\nCREATE TABLE t2 (id INT);\n",
        ),
        (
            "backslash that is a literal in a standard string",
            "CREATE TABLE a (n TEXT DEFAULT 'C:\\');\nCREATE TABLE b (id INT);\n",
        ),
        (
            "quoted names",
            "CREATE TABLE \"a\" (id INT);\nCREATE TABLE `b` (id INT);\nCREATE TABLE c (id INT)",
        ),
    ] {
        assert_clean_file_loses_nothing(label, source);
    }
}

#[test]
fn a_last_statement_without_a_semicolon_is_a_table() {
    let source = "CREATE TABLE a(id int);\nCREATE TABLE b(id int)";
    let extraction = extract_file("nosemi.sql", source);
    assert_eq!(declared(&extraction), vec!["a", "b"]);
    let b = relation(&extraction, "b");
    assert_eq!(
        &source[b.span.start_byte..b.span.end_byte],
        "CREATE TABLE b(id int)"
    );
}

/// MySQL `#` comments and backslash-escaped quotes. A `'` in a comment, or the
/// quote in `'O\'Brien'`, must not pair with a quote three statements later.
#[test]
fn mysql_comments_and_escaped_quotes_do_not_unpair_the_quotes() {
    let hash = "\
# dump header
CREATE TABLE a (id INT); # trailing
# it's an apostrophe in a comment
CREATE TABLE b (id INT);
CREATE TABLE c (id INT);
";
    let extraction = extract_file("hash.sql", hash);
    let names = declared(&extraction);
    for table in ["a", "b", "c"] {
        assert!(names.contains(&table), "missing {table}: {names:?}");
    }

    let escaped = "\
CREATE TABLE a (n TEXT DEFAULT 'O\\'Brien');
CREATE TABLE b (id INT);
CREATE TABLE c (n TEXT DEFAULT 'it\\'s');
CREATE TABLE d (id INT);
";
    let extraction = extract_file("escaped.sql", escaped);
    let names = declared(&extraction);
    for table in ["b", "d"] {
        assert!(names.contains(&table), "missing {table}: {names:?}");
    }
}

/// `END CASE` reopened the block it had just closed, so the procedure never
/// ended and every table after it was dropped.
#[test]
fn end_if_end_case_and_end_loop_close_a_procedure_body_once() {
    for (label, body) in [
        ("case", "CASE x WHEN 1 THEN SELECT 1; END CASE;"),
        ("if", "IF 1 THEN SELECT 1; END IF;"),
        ("loop", "lp: LOOP SELECT 1; LEAVE lp; END LOOP lp;"),
        ("while", "WHILE 1 DO SELECT 1; END WHILE;"),
        ("repeat", "REPEAT SELECT 1; UNTIL 1 END REPEAT;"),
        (
            "nested",
            "IF 1 THEN CASE x WHEN 1 THEN SELECT 1; END CASE; END IF; SELECT CASE WHEN 1 THEN 2 END;",
        ),
    ] {
        let source = format!(
            "CREATE PROCEDURE p() BEGIN {body} END;\nCREATE TABLE after_proc (id INT);\nCREATE TABLE last_one (id INT);\n"
        );
        let extraction = extract_file("proc.sql", &source);
        let names = declared(&extraction);
        for table in ["after_proc", "last_one"] {
            assert!(
                names.contains(&table),
                "{label}: `{table}` was dropped: {names:?}"
            );
        }
    }
}

/// `BEGIN TRANSACTION; … COMMIT;` is how `sqlite3 .dump` frames a schema. It is
/// not a block, so the fts5 table inside it is recovered like any other.
#[test]
fn a_dump_transaction_is_not_a_block_and_its_tables_are_recovered() {
    for begin in [
        "BEGIN TRANSACTION;",
        "BEGIN;",
        "BEGIN DEFERRED TRANSACTION;",
        "BEGIN IMMEDIATE;",
        "BEGIN EXCLUSIVE TRANSACTION;",
        "BEGIN WORK;",
    ] {
        let source = format!(
            "{begin}\nCREATE VIRTUAL TABLE docs_fts USING fts5(title);\nCREATE TABLE t (id INT);\nCOMMIT;\n"
        );
        let extraction = extract_file("dump.sql", &source);
        let names = declared(&extraction);
        assert!(names.contains(&"docs_fts"), "{begin}: {names:?}");
        let t = relation(&extraction, "t");
        assert_eq!(
            &source[t.span.start_byte..t.span.end_byte],
            "CREATE TABLE t (id INT)",
            "{begin}"
        );
        let fts = relation(&extraction, "docs_fts");
        assert!(
            source[fts.span.start_byte..fts.span.end_byte].contains("fts5"),
            "{begin}"
        );
        assert_eq!(names.len(), 2, "{begin}: {names:?}");
    }
}

/// The grammar names the table after a swallowed `fts5` statement `"work_events"`
/// with the quotes, wearing the virtual table's span. Matching on the raw name
/// missed it, pushed a second copy, and renamed the original to `lead`.
#[test]
fn a_quoted_name_after_a_dialect_error_is_one_symbol() {
    let source = "\
CREATE VIRTUAL TABLE lead USING fts5(x);
CREATE TABLE \"work_events\" (id INT CHECK(json_valid(id)));
";
    let extraction = extract_file("quoted.sql", source);
    let names = declared(&extraction);
    assert_eq!(names.len(), 2, "{names:?}");
    for name in ["lead", "work_events"] {
        assert_eq!(
            names.iter().filter(|n| **n == name).count(),
            1,
            "{name} is not exactly one symbol: {names:?}"
        );
    }
    let lead = relation(&extraction, "lead");
    assert!(source[lead.span.start_byte..lead.span.end_byte].contains("fts5"));
    let events = relation(&extraction, "work_events");
    assert!(source[events.span.start_byte..events.span.end_byte].starts_with("CREATE TABLE"));
    assert_eq!(events.qualified_name, "quoted.sql::work_events");
    assert!(
        !extraction.calls.is_empty(),
        "the CHECK call is the edge this case follows"
    );
    for call in &extraction.calls {
        assert_eq!(
            call.caller_symbol.as_deref(),
            Some("quoted.sql::work_events"),
            "{call:?}"
        );
    }
}

/// Recovery is a second reader of the text, so it removes a symbol only where
/// the grammar itself reported an error. A table the grammar parsed outside
/// every error range stays even when the scan reads that text as a routine
/// body; a table inside the unterminated tail, where the grammar's error is,
/// goes.
#[test]
fn recovery_never_removes_a_symbol_the_grammar_parsed_outside_an_error() {
    let source = "\
CREATE VIRTUAL TABLE docs_fts USING fts5(title);
CREATE FUNCTION f() RETURNS int AS $$ CREATE TABLE in_body (id INT); $$ LANGUAGE sql;
CREATE TABLE kept (id INT);
CREATE TABLE tail_open (id INT
";
    let grammar = grammar_only(source);
    assert!(
        matches!(grammar.parse_outcome, ParseOutcome::Partial { .. }),
        "{:?}",
        grammar.parse_outcome
    );
    let in_body = grammar
        .symbols
        .iter()
        .find(|symbol| symbol.name == "in_body")
        .expect("the grammar reads the function body as a table");
    let touches_error = match &grammar.parse_outcome {
        ParseOutcome::Partial { error_ranges, .. } => error_ranges.iter().any(|range| {
            range.start_byte <= in_body.span.end_byte && in_body.span.start_byte <= range.end_byte
        }),
        _ => unreachable!(),
    };
    assert!(
        !touches_error,
        "the case needs a grammar symbol outside every error range"
    );
    let extraction = extract_file("partial.sql", source);
    let names = declared(&extraction);
    for table in ["docs_fts", "in_body", "kept"] {
        assert!(names.contains(&table), "missing {table}: {names:?}");
    }
    assert!(
        !names.contains(&"tail_open"),
        "the unterminated tail is where the error is: {names:?}"
    );
}

/// Twenty thousand backticked tables (a MySQL dump), each with the function
/// `CHECK` the grammar rejects, so the file is `Partial` with twenty thousand
/// error ranges. Every name loses its backticks, every edge follows its table,
/// and the file finishes: the work is bounded by the file, not by renames
/// times edges.
#[test]
fn twenty_thousand_backticked_tables_finish_with_every_name_and_edge_intact() {
    const TABLES: usize = 20_000;
    let mut source = String::new();
    for index in 0..TABLES {
        source.push_str(&format!(
            "CREATE TABLE `t{index}` (a INT CHECK(abs(a)>0));\n"
        ));
    }
    assert!(source.len() < 1024 * 1024, "{} bytes", source.len());
    let (sender, receiver) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let extraction = devmap_extract::extract_file_with_budget(
            "dump.sql",
            &source,
            std::time::Duration::from_secs(60),
        );
        let _ = sender.send(extraction);
    });
    let extraction = receiver
        .recv_timeout(std::time::Duration::from_secs(90))
        .expect("a 20,000-table dump did not finish within the hard timeout");
    handle.join().expect("extraction thread");
    assert!(
        matches!(
            extraction.parse_outcome,
            ParseOutcome::Clean | ParseOutcome::Partial { .. }
        ),
        "{:?}",
        extraction.parse_outcome
    );
    let tables: Vec<&str> = declared(&extraction);
    assert_eq!(tables.len(), TABLES);
    let mut seen = std::collections::HashSet::new();
    for name in &tables {
        assert!(!name.contains('`'), "quotes left on {name}");
        assert!(seen.insert(*name), "duplicate {name}");
    }
    assert!(seen.contains("t0") && seen.contains("t19999"));
    let qualified: std::collections::HashSet<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert!(!extraction.calls.is_empty());
    for call in &extraction.calls {
        if let Some(caller) = call.caller_symbol.as_deref() {
            assert!(qualified.contains(caller), "dangling caller {caller}");
        }
    }
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
