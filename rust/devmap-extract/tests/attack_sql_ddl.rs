//! Adversarial tests for SQL relation recovery (`src/sqlddl.rs`).
//!
//! Each `attack_sql_*` test that is listed as a break pins one violation of the
//! owner's ruling and fails against the lane as committed. Contract numbers
//! refer to the ruling:
//!
//! 1. A `Clean` grammar parse never loses a symbol, call or reference to
//!    recovery, and a quoted-name rename never creates a duplicate.
//! 2. Removal happens only where a symbol overlaps a grammar error range and
//!    sits in a skipped statement body or the unterminated tail.
//! 3. Every loop is bounded and a capped or abandoned run leaves the grammar
//!    output untouched.
//! 4. A wrong symbol is worse than a missing one.
//! 5. Output is deterministic.
//!
//! Every input is SQL a schema file carries. `CHECK(json_valid(x))` is the
//! grammar's own confined error (see `sql_check_constraint_parse.rs`): it gives
//! a table an error range of its own, so that a table the grammar parsed and
//! the removal predicate can meet.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

const JSON_CHECK: &str = "CHECK(json_valid(x))";

fn grammar(source: &str) -> Extraction {
    devmap_extract::treesitter::extract_treesitter("a.sql", "sql", source)
}

fn names(extraction: &Extraction) -> Vec<&str> {
    extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.name.as_str())
        .collect()
}

fn has_error_ranges(extraction: &Extraction) -> bool {
    matches!(&extraction.parse_outcome, ParseOutcome::Partial { error_ranges, .. } if !error_ranges.is_empty())
}

/// The grammar alone read `table` as a symbol and the input is not clean, so
/// recovery is the only thing that can take it away. Without this a test could
/// pass or fail because the grammar lost the table first.
fn assert_grammar_kept_it(source: &str, table: &str) {
    let grammar = grammar(source);
    assert!(
        has_error_ranges(&grammar),
        "the control case needs a grammar error: {:?}",
        grammar.parse_outcome
    );
    assert!(
        names(&grammar).contains(&table),
        "the grammar itself lost `{table}`: {:?}",
        names(&grammar)
    );
}

/// Contract 2 and 4. MySQL scripts define routines between `DELIMITER $$` and
/// `DELIMITER ;`, and end each one with `END$$`. A `$` is an identifier
/// character to the keyword reader, so `END$$` is not an `END`, the block
/// never closes, and the second routine's statement never terminates. The
/// "unterminated tail" then starts at the second `CREATE PROCEDURE`, and the
/// ordinary table after `DELIMITER ;` is removed because it touches an error
/// range of its own.
///
/// HIGH: a real table removed on a real MySQL migration.
#[test]
fn attack_sql_mysql_dollar_delimiter_routines_do_not_make_a_real_table_a_tail() {
    let source = format!(
        "CREATE TABLE a (id INT);\n\
         DELIMITER $$\n\
         CREATE PROCEDURE p() BEGIN SELECT 1; END$$\n\
         CREATE PROCEDURE q() BEGIN SELECT 2; END$$\n\
         DELIMITER ;\n\
         CREATE TABLE after_proc (x TEXT {JSON_CHECK});\n"
    );
    assert_grammar_kept_it(&source, "after_proc");
    let full = extract_file("a.sql", &source);
    assert!(
        names(&full).contains(&"after_proc"),
        "`after_proc` is a table the grammar parsed after `DELIMITER ;`, and \
         recovery removed it: {:?}",
        names(&full)
    );
}

/// Contract 2 and 4. `[` opens a T-SQL bracketed name in the noise eater, and
/// `read_quoted` treats `]]` as an escaped `]`. Postgres nested array
/// constructors and subscripts end in `]]`, so the bracket never closes, the
/// eater reports `Unclosed`, and everything after it is "the tail".
///
/// HIGH: `ARRAY[ARRAY[...]]` and `a[b[1]]` are ordinary Postgres.
#[test]
fn attack_sql_nested_array_brackets_do_not_make_the_rest_of_the_file_a_tail() {
    for (label, prefix) in [
        (
            "nested ARRAY constructor",
            "INSERT INTO t VALUES (ARRAY[ARRAY[1,2],ARRAY[3,4]]);",
        ),
        ("nested subscript", "SELECT a[b[1]];"),
    ] {
        let source = format!("{prefix}\nCREATE TABLE after (x TEXT {JSON_CHECK});\n");
        assert_grammar_kept_it(&source, "after");
        let full = extract_file("a.sql", &source);
        assert!(
            names(&full).contains(&"after"),
            "{label}: recovery removed the table written after it: {:?}",
            names(&full)
        );
    }
}

/// Contract 2. The same `]]` misreading inside a table's own statement: the
/// table that declares a multi-dimensional array default is removed together
/// with the table after it, though neither is in a trigger, a routine or a
/// comment.
///
/// HIGH.
#[test]
fn attack_sql_a_multidimensional_array_default_does_not_remove_its_own_table() {
    let source = format!(
        "CREATE TABLE grid (g int[][] DEFAULT ARRAY[[1,2],[3,4]]);\n\
         CREATE TABLE after (x TEXT {JSON_CHECK});\n"
    );
    assert_grammar_kept_it(&source, "grid");
    let full = extract_file("a.sql", &source);
    for table in ["grid", "after"] {
        assert!(
            names(&full).contains(&table),
            "`{table}` was removed: {:?}",
            names(&full)
        );
    }
}

/// Contract 2. `begin` is a legal column name, and `SELECT begin FROM r` at
/// parenthesis depth zero reads as a block opener that never closes. The view
/// statement is then "unterminated", and the real table after it is dropped
/// as tail.
///
/// HIGH: `begin`/`end` columns are normal in time-range views.
#[test]
fn attack_sql_a_view_selecting_a_column_named_begin_does_not_remove_the_next_table() {
    let source = format!(
        "CREATE VIEW v AS SELECT begin FROM r;\n\
         CREATE TABLE after (x TEXT {JSON_CHECK});\n\
         CREATE TABLE z (x INT);\n"
    );
    assert_grammar_kept_it(&source, "after");
    let full = extract_file("a.sql", &source);
    assert!(
        names(&full).contains(&"after"),
        "recovery removed `after` as tail of an unterminated view: {:?}",
        names(&full)
    );
}

/// Contract 4. `read_ident` only reads ASCII identifier bytes, so the virtual
/// table `données` is recovered as `donn`: a symbol that exists nowhere in the
/// source and that nothing can ever resolve to.
///
/// MEDIUM: a wrong symbol, which the ruling ranks below a missing one.
#[test]
fn attack_sql_a_non_ascii_virtual_table_name_is_not_truncated() {
    let source = "CREATE VIRTUAL TABLE données USING fts5(x);\nCREATE TABLE z (id int);\n";
    let full = extract_file("a.sql", source);
    let found = names(&full);
    assert!(
        !found.contains(&"donn"),
        "invented `donn`, a prefix of the written name `données`: {found:?}"
    );
}

/// Contract 4. A `CREATE VIRTUAL TABLE` text inside a dollar-quoted routine
/// body is not a schema object. The grammar reads it as one with a span that
/// leaks past the routine into the next table, and the removal predicate only
/// removes a symbol that ends inside the skipped statement, so the declaration
/// survives with a span that covers `real_t`.
///
/// MEDIUM.
#[test]
fn attack_sql_a_table_inside_a_dollar_quoted_body_is_not_left_as_a_symbol() {
    let source = "\
CREATE FUNCTION f() RETURNS void AS $$ CREATE VIRTUAL TABLE ghost USING fts5(x); $$ LANGUAGE sql;
CREATE TABLE real_t (id int);
CREATE TABLE r2 (id int);
";
    let full = extract_file("a.sql", source);
    let found = names(&full);
    assert!(
        found.contains(&"real_t") && found.contains(&"r2"),
        "{found:?}"
    );
    assert!(
        !found.contains(&"ghost"),
        "`ghost` is text inside a `$$` body, not a table: {found:?}"
    );
}

/// Contract 1: quoted-name renames "must never create duplicates". `s1."t"`
/// and `s2.t` are distinct qualified names in the grammar's output
/// (`a.sql::"t"` and `a.sql::t`). Stripping the quotes makes both
/// `a.sql::t`, on a file the grammar parsed `Clean`.
///
/// MEDIUM: the two tables now share one qualified name, and every edge from
/// either is attributed to both.
#[test]
fn attack_sql_unquoting_a_name_never_collides_with_another_symbol() {
    let source = "CREATE TABLE s1.\"t\" (id INT);\nCREATE TABLE s2.t (id INT);\n";
    let grammar = grammar(source);
    assert_eq!(grammar.parse_outcome, ParseOutcome::Clean, "control");
    assert_eq!(
        qualified_collisions(&grammar),
        0,
        "control: the grammar kept them apart"
    );
    let full = extract_file("a.sql", source);
    assert_eq!(full.parse_outcome, ParseOutcome::Clean);
    assert_eq!(
        qualified_collisions(&full),
        0,
        "recovery's unquoting made two symbols share a qualified name: {:?}",
        full.symbols
            .iter()
            .map(|symbol| symbol.qualified_name.as_str())
            .collect::<Vec<_>>()
    );
}

fn qualified_collisions(extraction: &Extraction) -> usize {
    let mut seen = std::collections::HashSet::new();
    extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .filter(|symbol| !seen.insert(symbol.qualified_name.clone()))
        .count()
}

/// Contract 4, dialect-ambiguous. Postgres and the SQL standard nest block
/// comments; the scanner reads the first `*/` as the end, so the text after it
/// is code and a `CREATE VIRTUAL TABLE` inside the outer comment is
/// synthesised as a table with no grammar involved. (SQLite does not nest, so
/// there it is a real table: the file does not say which it is.)
///
/// LOW.
#[test]
fn attack_sql_a_declaration_inside_a_nested_block_comment_is_not_synthesised() {
    let source = "\
/* disabled for now /* old note */
CREATE VIRTUAL TABLE ghost USING fts5(x);
*/
CREATE TABLE z (id int);
";
    let full = extract_file("a.sql", source);
    let found = names(&full);
    assert!(found.contains(&"z"), "{found:?}");
    assert!(
        !found.contains(&"ghost"),
        "`ghost` is inside an outer block comment: {found:?}"
    );
}

/// Contracts 3, 4, 5 over a spread of dialects: the same input gives the same
/// extraction twice, every span is inside the file on char boundaries, and a
/// file the grammar parsed `Clean` keeps every symbol with its span.
///
/// Written as a battery so a regression names the input. It is expected to
/// pass today; it is the record of what held.
#[test]
fn attack_sql_battery_is_deterministic_in_bounds_and_loses_nothing_clean() {
    let corpus: &[(&str, &str)] = &[
        ("crlf", "CREATE VIRTUAL TABLE d USING fts5(x);\r\nCREATE TABLE z (id int);\r\n"),
        ("bom", "\u{feff}CREATE VIRTUAL TABLE d USING fts5(x);\nCREATE TABLE z (id int);\n"),
        ("unicode names", "CREATE TABLE \"表\" (id int);\nCREATE TABLE é (id int);\n"),
        ("tsql brackets", "CREATE TABLE [dbo].[Order Details] (id INT);\nCREATE TABLE [a]]b] (id INT);\n"),
        ("temp and mat view", "CREATE TEMP TABLE tt (id int);\nCREATE MATERIALIZED VIEW mv AS SELECT 1;\n"),
        ("quoted semicolon", "CREATE TABLE \"a;b\" (id int);\nCREATE TABLE z (id int);\n"),
        ("case in check", "CREATE TABLE t (id INT CHECK (CASE WHEN id>0 THEN 1 ELSE 0 END = 1), \"end\" INT DEFAULT (CASE WHEN 1 THEN 2 END));\n"),
        ("column names", "CREATE TABLE t (begin INT, \"end\" INT, \"case\" INT);\nCREATE TABLE z (id int);\n"),
        ("dollar function", "CREATE FUNCTION f() RETURNS int AS $a$ SELECT $b$ x $b$ $a$ LANGUAGE sql;\nCREATE TABLE after (id int);\n"),
        ("trigger when", "CREATE TABLE a (id int);\nCREATE TRIGGER tr AFTER INSERT ON a WHEN new.id > 0 BEGIN INSERT INTO log VALUES (CASE WHEN new.id>1 THEN 1 ELSE 2 END); END;\nCREATE TABLE b (id int);\n"),
        ("begin atomic", "CREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; END;\nCREATE VIRTUAL TABLE d USING fts5(x);\nCREATE TABLE z (id int);\n"),
        ("unterminated dollar", "CREATE VIRTUAL TABLE d USING fts5(x);\nCREATE TABLE a (id int);\nSELECT $$ oops;\nCREATE TABLE b (id INT);\n"),
        ("only an error", "CREATE VIRTUAL TABLE d USING fts5("),
        ("fts5 options", "CREATE VIRTUAL TABLE d USING fts5(x, tokenize = \"unicode61 remove_diacritics 2\");\nCREATE TABLE z (id int);\n"),
        ("empty statements", ";;;\nCREATE VIRTUAL TABLE d USING fts5(x);;\nCREATE TABLE z (id int);\n"),
        ("hash comment", "# it's a comment\nCREATE VIRTUAL TABLE d USING fts5(x);\nCREATE TABLE z (id int);\n"),
        ("escaped string", "SELECT E'it\\'s';\nCREATE VIRTUAL TABLE d USING fts5(x);\nCREATE TABLE z (id int);\n"),
        ("multibyte near keyword", "CREATE TABLE t (CREATÉ INT);\nCREATE TABLE after_unicode (id INT);\n"),
    ];
    for (label, source) in corpus {
        let first = extract_file("a.sql", source);
        let second = extract_file("a.sql", source);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap(),
            "{label}: not deterministic"
        );
        assert_eq!(
            serde_json::to_string(&first.for_durable_store()).unwrap(),
            serde_json::to_string(&second.for_durable_store()).unwrap(),
            "{label}: durable form not deterministic"
        );
        for symbol in first.symbols.iter().filter(|s| s.kind != SymbolKind::File) {
            let (start, end) = (symbol.span.start_byte, symbol.span.end_byte);
            assert!(
                start <= end
                    && end <= source.len()
                    && source.is_char_boundary(start)
                    && source.is_char_boundary(end),
                "{label}: `{}` span {start}..{end} is outside the file or off a char boundary",
                symbol.name
            );
        }
        let grammar = grammar(source);
        if grammar.parse_outcome == ParseOutcome::Clean {
            assert_eq!(first.parse_outcome, ParseOutcome::Clean, "{label}");
            assert_eq!(first.symbols.len(), grammar.symbols.len(), "{label}");
            assert_eq!(first.calls.len(), grammar.calls.len(), "{label}: calls");
            assert_eq!(
                first.references.len(),
                grammar.references.len(),
                "{label}: references"
            );
            for kept in &grammar.symbols {
                assert!(
                    first.symbols.iter().any(|symbol| symbol.span.start_byte
                        == kept.span.start_byte
                        && symbol.span.end_byte == kept.span.end_byte),
                    "{label}: recovery changed the span of `{}`",
                    kept.name
                );
            }
        }
    }
}
