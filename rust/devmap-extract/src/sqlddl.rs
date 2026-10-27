//! Relations the SQL grammar drops inside a dialect `ERROR` span.
//!
//! tree-sitter-sequel has no production for `CREATE VIRTUAL TABLE ... USING
//! fts5` or for a SQLite trigger body. The error is not confined to that
//! clause: the node that survives can wear the following table's name and a
//! span that starts at the broken statement, or the following `CREATE TABLE`
//! is absent entirely. Both are the same fact — the statement text is a
//! relation the grammar accepts when it is parsed alone.
//!
//! Recovery reparses that statement and installs the symbol the grammar
//! itself emits for it, shifted back into the file. A virtual table never
//! parses, so its name and span come from the statement text and it is a
//! `Struct`, the kind every other relation already has. Indexes and triggers
//! stay non-symbols. Nothing is invented for an unterminated statement, or
//! for a `CREATE` inside a string, a comment, a dollar quote, a parenthesised
//! expression, or a `BEGIN`/`END` body.

use crate::model::{ExtractedSymbol, Extraction, ParseOutcome, Span, SymbolKind};

/// Reparsed statements per file. Past this the rest stay as the grammar left
/// them: a cap is a partial recovery, and it must not be reported as a
/// finished scan by dropping the error ranges those statements still sit in.
const MAX_REPARSES: usize = 256;

/// Relation statements read from one file. Past this the tail is not scanned.
const MAX_STATEMENTS: usize = 4096;

/// Put back relations whose statement overlaps a parse error and whose symbol
/// is missing or wears the wrong span.
///
/// A no-op for every other language. Existing error ranges are left in place:
/// a `CHECK(json_valid(...))` the grammar already confined stays a recorded
/// error, and a trigger body the grammar cannot parse stays one too.
///
/// A `CREATE TABLE` the grammar saw inside a trigger body, a routine body, or
/// an unterminated statement is not a relation in the schema. Those symbols
/// are removed. A stored function itself stays: it is the callable the call
/// graph already attributes. Quoted identifiers are stored as the name inside
/// the quotes.
pub(crate) fn recover_lost_relations(extraction: &mut Extraction, source: &str) {
    if extraction.language != "sql" || source.is_empty() {
        return;
    }
    let scan = scan(source);
    if let ParseOutcome::Partial { error_ranges, .. } = &extraction.parse_outcome {
        if !error_ranges.is_empty() {
            let ranges: Vec<(usize, usize)> = error_ranges
                .iter()
                .map(|range| (range.start_byte, range.end_byte))
                .collect();
            let mut reparses = 0usize;
            for statement in &scan.relations {
                if !ranges
                    .iter()
                    .any(|&(start, end)| overlaps(statement.start, statement.end, start, end))
                {
                    continue;
                }
                let Some(desired) = symbol_for(extraction, source, statement, &mut reparses) else {
                    continue;
                };
                install(extraction, desired, statement.start, statement.end);
            }
        }
    }
    apply_scanned_names(extraction, &scan.relations);
    normalize_quoted_names(extraction);
    drop_spurious(extraction, &scan);
}

/// The grammar's name node ends at the first quote or space inside a delimited
/// identifier (`"a""b"` becomes `a`, `[Order Details]` becomes `Details`).
/// The statement scan already read the identifier. Write that name onto the
/// declaration that starts inside the statement.
fn apply_scanned_names(extraction: &mut Extraction, relations: &[Statement]) {
    let file = extraction.file_path.clone();
    for statement in relations {
        let Some(index) = extraction
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, symbol)| {
                symbol.kind != SymbolKind::File
                    && symbol.span.start_byte >= statement.start
                    && symbol.span.start_byte < statement.end
            })
            .min_by_key(|(_, symbol)| symbol.span.start_byte)
            .map(|(index, _)| index)
        else {
            continue;
        };
        let (old_qualified, new_qualified) = {
            let symbol = &mut extraction.symbols[index];
            if symbol.name == statement.name {
                continue;
            }
            let old_qualified = symbol.qualified_name.clone();
            let old_name = symbol.name.clone();
            symbol.qualified_name = renamed(&old_qualified, &old_name, &statement.name, &file);
            symbol.name = statement.name.clone();
            (old_qualified, symbol.qualified_name.clone())
        };
        retarget(extraction, &old_qualified, &new_qualified);
    }
}

struct Statement {
    start: usize,
    end: usize,
    name: String,
    /// `CREATE VIRTUAL TABLE` — the grammar has no node for it.
    virtual_table: bool,
}

fn symbol_for(
    extraction: &Extraction,
    source: &str,
    statement: &Statement,
    reparses: &mut usize,
) -> Option<ExtractedSymbol> {
    if statement.virtual_table {
        return Some(synthesized(extraction, statement));
    }
    if *reparses >= MAX_REPARSES {
        return None;
    }
    *reparses += 1;
    let snippet = source.get(statement.start..statement.end)?;
    let parsed = crate::treesitter::extract_treesitter(&extraction.file_path, "sql", snippet);
    let mut symbol = parsed.symbols.into_iter().find(|symbol| {
        symbol.kind != SymbolKind::File && same_name(&symbol.name, &statement.name)
    })?;
    symbol.span.start_byte = symbol.span.start_byte.saturating_add(statement.start);
    symbol.span.end_byte = symbol.span.end_byte.saturating_add(statement.start);
    // The grammar's name node includes the quote characters. The statement
    // scanner already stored the identifier.
    symbol.qualified_name = format!("{}::{}", extraction.file_path, statement.name);
    symbol.name = statement.name.clone();
    Some(symbol)
}

fn synthesized(extraction: &Extraction, statement: &Statement) -> ExtractedSymbol {
    let file = extraction.file_path.clone();
    ExtractedSymbol {
        qualified_name: format!("{file}::{}", statement.name),
        name: statement.name.clone(),
        kind: SymbolKind::Struct,
        span: Span {
            start_byte: statement.start,
            end_byte: statement.end,
        },
        is_exported: !statement.name.starts_with('_'),
        docstring: None,
        signature: None,
        parent_symbol: Some(file),
        body_signature: None,
        declaration_hash: None,
        return_type: None,
    }
}

/// Replace a same-named symbol whose span overlaps this statement, or append
/// one. A symbol that already has the grammar's span is left untouched, so a
/// table the grammar parsed — including one whose `CHECK(...)` is an error
/// inside it — is not rewritten.
fn install(extraction: &mut Extraction, desired: ExtractedSymbol, start: usize, end: usize) {
    if let Some(existing) = extraction.symbols.iter_mut().find(|symbol| {
        symbol.kind != SymbolKind::File
            && symbol.name == desired.name
            && overlaps(symbol.span.start_byte, symbol.span.end_byte, start, end)
    }) {
        if existing.span.start_byte == desired.span.start_byte
            && existing.span.end_byte == desired.span.end_byte
            && existing.kind == desired.kind
            && existing.qualified_name == desired.qualified_name
            && existing.is_exported == desired.is_exported
            && existing.parent_symbol == desired.parent_symbol
        {
            return;
        }
        existing.span = desired.span;
        existing.kind = desired.kind;
        existing.qualified_name = desired.qualified_name;
        existing.is_exported = desired.is_exported;
        existing.parent_symbol = desired.parent_symbol;
        return;
    }
    if extraction.symbols.iter().any(|symbol| {
        symbol.kind != SymbolKind::File
            && symbol.name == desired.name
            && symbol.span.start_byte == desired.span.start_byte
            && symbol.span.end_byte == desired.span.end_byte
    }) {
        return;
    }
    extraction.symbols.push(desired);
}

fn overlaps(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start < b_end && b_start < a_end
}

struct Scan {
    relations: Vec<Statement>,
    /// Non-relation statements (`CREATE FUNCTION`, `CREATE TRIGGER`, …),
    /// start inclusive and end exclusive. A symbol strictly inside one of
    /// these is a declaration the body text mentioned, not a schema object.
    skipped: Vec<(usize, usize)>,
    /// Byte of a `CREATE` whose statement never terminated. Symbols from here
    /// to the end of the file are the grammar inventing a declaration.
    unterminated_at: Option<usize>,
}

fn scan(source: &str) -> Scan {
    let bytes = source.as_bytes();
    let mut relations = Vec::new();
    let mut skipped = Vec::new();
    let mut unterminated_at = None;
    let mut i = 0usize;
    let mut paren = 0i32;
    let mut block = 0i32;
    // Start of the outermost top-level `BEGIN`/`CASE`. A relation the grammar
    // reports inside that block is not a schema object.
    let mut block_at: Option<usize> = None;
    while i < bytes.len() && relations.len() < MAX_STATEMENTS {
        match eat_noise(source, i) {
            Eat::Unclosed => {
                unterminated_at = Some(i);
                break;
            }
            Eat::Ate(next) if next > i => {
                i = next;
                continue;
            }
            Eat::Ate(_) | Eat::No => {}
        }
        if bytes[i] == b'(' {
            paren += 1;
            i += 1;
            continue;
        }
        if bytes[i] == b')' {
            paren = paren.saturating_sub(1);
            i += 1;
            continue;
        }
        if paren == 0 && (keyword(source, i, "BEGIN") || keyword(source, i, "CASE")) {
            if block == 0 {
                block_at = Some(i);
            }
            block += 1;
            i += if keyword(source, i, "BEGIN") { 5 } else { 4 };
            continue;
        }
        if paren == 0 && block > 0 && keyword(source, i, "END") {
            block -= 1;
            let end = i + 3;
            if block == 0 {
                if let Some(start) = block_at.take() {
                    skipped.push((start, end));
                }
            }
            i = end;
            continue;
        }
        if paren == 0 && block == 0 && keyword(source, i, "CREATE") {
            match classify(source, i) {
                Classified::Relation(statement) => {
                    i = statement.end;
                    relations.push(statement);
                }
                Classified::Skip(next) if next > i => {
                    skipped.push((i, next));
                    i = next;
                }
                Classified::Skip(_) | Classified::Stop => {
                    unterminated_at = Some(i);
                    break;
                }
            }
            continue;
        }
        i = next_char(source, i);
    }
    Scan {
        relations,
        skipped,
        unterminated_at,
    }
}

enum Classified {
    Relation(Statement),
    /// A `CREATE` that is not a relation, consumed through its terminator.
    Skip(usize),
    /// No terminator. The tail of the file is not a statement.
    Stop,
}

fn classify(source: &str, create_at: usize) -> Classified {
    let Some(mut cursor) = skip_sep(source, create_at + "CREATE".len()) else {
        return Classified::Stop;
    };
    let mut virtual_table = false;
    let kind = loop {
        let Some((word, next)) = read_word(source, cursor) else {
            return finish_non_relation(source, create_at);
        };
        let upper = word.to_ascii_uppercase();
        match upper.as_str() {
            "OR" => {
                let Some(after) = skip_sep(source, next) else {
                    return Classified::Stop;
                };
                let Some((replace, after_replace)) = read_word(source, after) else {
                    return finish_non_relation(source, create_at);
                };
                if !replace.eq_ignore_ascii_case("REPLACE") {
                    return finish_non_relation(source, create_at);
                }
                let Some(after_sep) = skip_sep(source, after_replace) else {
                    return Classified::Stop;
                };
                cursor = after_sep;
            }
            "TEMP" | "TEMPORARY" | "UNLOGGED" | "GLOBAL" | "LOCAL" | "UNIQUE" => {
                let Some(after) = skip_sep(source, next) else {
                    return Classified::Stop;
                };
                cursor = after;
            }
            "VIRTUAL" => {
                virtual_table = true;
                let Some(after) = skip_sep(source, next) else {
                    return Classified::Stop;
                };
                cursor = after;
            }
            "MATERIALIZED" => {
                let Some(after) = skip_sep(source, next) else {
                    return Classified::Stop;
                };
                let Some((view, _)) = read_word(source, after) else {
                    return finish_non_relation(source, create_at);
                };
                if !view.eq_ignore_ascii_case("VIEW") {
                    return finish_non_relation(source, create_at);
                }
                // Leave the cursor on VIEW so the step below consumes it,
                // the same way TABLE and VIEW already sit under the cursor.
                cursor = after;
                break RelationKind::View;
            }
            "TABLE" => break RelationKind::Table,
            "VIEW" => break RelationKind::View,
            _ => return finish_non_relation(source, create_at),
        }
    };
    let Some(mut cursor) = skip_sep(
        source,
        match kind {
            RelationKind::Table | RelationKind::View => {
                // `cursor` still points at the kind word. Step over it.
                let Some((_, next)) = read_word(source, cursor) else {
                    return Classified::Stop;
                };
                next
            }
        },
    ) else {
        return Classified::Stop;
    };
    if let Some((word, next)) = read_word(source, cursor) {
        if word.eq_ignore_ascii_case("IF") {
            let Some(after_if) = skip_sep(source, next) else {
                return Classified::Stop;
            };
            let Some((not, after_not)) = read_word(source, after_if) else {
                return finish_non_relation(source, create_at);
            };
            if !not.eq_ignore_ascii_case("NOT") {
                return finish_non_relation(source, create_at);
            }
            let Some(after_not_sep) = skip_sep(source, after_not) else {
                return Classified::Stop;
            };
            let Some((exists, after_exists)) = read_word(source, after_not_sep) else {
                return finish_non_relation(source, create_at);
            };
            if !exists.eq_ignore_ascii_case("EXISTS") {
                return finish_non_relation(source, create_at);
            }
            let Some(after) = skip_sep(source, after_exists) else {
                return Classified::Stop;
            };
            cursor = after;
        }
    }
    let Some(name) = read_qualified_name(source, cursor) else {
        return finish_non_relation(source, create_at);
    };
    let Some(end) = statement_end(source, create_at) else {
        return Classified::Stop;
    };
    if name.is_empty() || end <= create_at {
        return finish_non_relation(source, create_at);
    }
    Classified::Relation(Statement {
        start: create_at,
        end,
        name,
        virtual_table: virtual_table && matches!(kind, RelationKind::Table),
    })
}

enum RelationKind {
    Table,
    View,
}

fn finish_non_relation(source: &str, create_at: usize) -> Classified {
    match statement_end(source, create_at) {
        Some(end) if end > create_at => Classified::Skip(end),
        _ => Classified::Stop,
    }
}

/// Byte after the terminating semicolon, at parenthesis and `BEGIN`/`CASE`
/// depth zero. `None` when the statement never terminates or a quote or
/// comment never closes.
fn statement_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = start;
    let mut paren = 0i32;
    let mut block = 0i32;
    while i < bytes.len() {
        match eat_noise(source, i) {
            Eat::Unclosed => return None,
            Eat::Ate(next) if next > i => {
                i = next;
                continue;
            }
            Eat::Ate(_) | Eat::No => {}
        }
        match bytes[i] {
            b'(' => {
                paren += 1;
                i += 1;
            }
            b')' => {
                paren = paren.saturating_sub(1);
                i += 1;
            }
            b';' if paren == 0 && block == 0 => return Some(i + 1),
            _ => {
                if paren == 0 && (keyword(source, i, "BEGIN") || keyword(source, i, "CASE")) {
                    block += 1;
                    i += if keyword(source, i, "BEGIN") { 5 } else { 4 };
                } else if paren == 0 && block > 0 && keyword(source, i, "END") {
                    block -= 1;
                    i += 3;
                } else {
                    i = next_char(source, i);
                }
            }
        }
    }
    None
}

fn read_qualified_name(source: &str, start: usize) -> Option<String> {
    let (mut name, mut cursor) = read_ident(source, start)?;
    loop {
        let Some(after) = skip_sep(source, cursor) else {
            return None;
        };
        if after >= source.len() || source.as_bytes()[after] != b'.' {
            break;
        }
        let Some(after_dot) = skip_sep(source, after + 1) else {
            return None;
        };
        let (next, next_cursor) = read_ident(source, after_dot)?;
        name = next;
        cursor = next_cursor;
    }
    (!name.is_empty()).then_some(name)
}

fn read_ident(source: &str, start: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    if start >= bytes.len() {
        return None;
    }
    match bytes[start] {
        b'"' => read_quoted(source, start, b'"', b'"'),
        b'`' => read_quoted(source, start, b'`', b'`'),
        b'[' => read_quoted(source, start, b']', b']'),
        b if is_ident_start(b) => {
            let mut end = start + 1;
            while end < bytes.len() && is_ident_continue(bytes[end]) {
                end += 1;
            }
            Some((source[start..end].to_string(), end))
        }
        _ => None,
    }
}

/// The interior of a quoted ident. A doubled closer is the closer itself.
fn read_quoted(source: &str, start: usize, closer: u8, escape: u8) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    let mut i = start + 1;
    let mut name = String::new();
    while i < bytes.len() {
        if bytes[i] == closer {
            let doubled = i + 1 < bytes.len() && bytes[i + 1] == escape && closer == escape;
            if doubled || (closer == b']' && i + 1 < bytes.len() && bytes[i + 1] == b']') {
                name.push(bytes[i] as char);
                i += 2;
                continue;
            }
            return Some((name, i + 1));
        }
        let ch = source[i..].chars().next()?;
        name.push(ch);
        i += ch.len_utf8();
    }
    None
}

fn read_word(source: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    if start >= bytes.len() || !is_ident_start(bytes[start]) {
        return None;
    }
    let mut end = start + 1;
    while end < bytes.len() && is_ident_continue(bytes[end]) {
        end += 1;
    }
    Some((&source[start..end], end))
}

fn skip_sep(source: &str, mut i: usize) -> Option<usize> {
    loop {
        if i >= source.len() {
            return Some(i);
        }
        match eat_separator(source, i) {
            Eat::Unclosed => return None,
            Eat::Ate(next) if next > i => i = next,
            Eat::Ate(_) | Eat::No => return Some(i),
        }
    }
}

enum Eat {
    No,
    Ate(usize),
    Unclosed,
}

/// Whitespace, comments, strings and dollar quotes. Used inside a statement,
/// where a quoted semicolon must not end it.
fn eat_noise(source: &str, i: usize) -> Eat {
    match eat_separator(source, i) {
        Eat::Unclosed => return Eat::Unclosed,
        Eat::Ate(next) if next > i => return Eat::Ate(next),
        Eat::Ate(_) | Eat::No => {}
    }
    if i < source.len() && source.as_bytes()[i] == b'[' {
        return match read_quoted(source, i, b']', b']') {
            Some((_, end)) => Eat::Ate(end),
            None => Eat::Unclosed,
        };
    }
    if let Some(eaten) = eat_dollar(source, i) {
        return eaten;
    }
    if let Some(eaten) = eat_string(source, i) {
        return eaten;
    }
    Eat::No
}

fn eat_separator(source: &str, i: usize) -> Eat {
    if i >= source.len() {
        return Eat::No;
    }
    let bytes = source.as_bytes();
    if bytes[i].is_ascii_whitespace() {
        let mut j = i + 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        return Eat::Ate(j);
    }
    if source[i..].starts_with("--") {
        let rest = i + 2;
        let end = source[rest..]
            .find('\n')
            .map(|offset| rest + offset + 1)
            .unwrap_or(source.len());
        return Eat::Ate(end);
    }
    if source[i..].starts_with("/*") {
        return match source[i + 2..].find("*/") {
            Some(offset) => Eat::Ate(i + 2 + offset + 2),
            None => Eat::Unclosed,
        };
    }
    Eat::No
}

fn eat_dollar(source: &str, i: usize) -> Option<Eat> {
    if !source[i..].starts_with('$') {
        return None;
    }
    let after = i + 1;
    let tag_len = if source[after..].starts_with('$') {
        0
    } else {
        let len = source[after..]
            .bytes()
            .take_while(|byte| is_ident_continue(*byte))
            .count();
        if len == 0 || !source[after + len..].starts_with('$') {
            return None;
        }
        len
    };
    let closer_len = 1 + tag_len + 1;
    let closer = &source[i..i + closer_len];
    let body = i + closer_len;
    match source[body..].find(closer) {
        Some(offset) => Some(Eat::Ate(body + offset + closer.len())),
        None => Some(Eat::Unclosed),
    }
}

fn eat_string(source: &str, i: usize) -> Option<Eat> {
    let bytes = source.as_bytes();
    if i >= bytes.len() {
        return None;
    }
    let mut quote_at = i;
    if matches!(
        bytes[i],
        b'N' | b'n' | b'X' | b'x' | b'E' | b'e' | b'B' | b'b'
    ) && (i == 0 || !is_ident_continue(bytes[i - 1]))
        && i + 1 < bytes.len()
        && bytes[i + 1] == b'\''
    {
        quote_at = i + 1;
    } else if (bytes[i] == b'U' || bytes[i] == b'u')
        && (i == 0 || !is_ident_continue(bytes[i - 1]))
        && source[i + 1..].starts_with("&'")
    {
        quote_at = i + 2;
    }
    let quote = *bytes.get(quote_at)?;
    if quote != b'\'' && quote != b'"' && quote != b'`' {
        return None;
    }
    // A `"` or `` ` `` here is an identifier when the header reader is in
    // charge. Inside a statement both still hide semicolons, which is what
    // this eater is for. At the header, `read_ident` runs before the body
    // scan, so consuming the name as noise there never happens.
    Some(quoted_span(source, quote_at, quote))
}

fn quoted_span(source: &str, quote_at: usize, quote: u8) -> Eat {
    let bytes = source.as_bytes();
    let mut i = quote_at + 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if i + 1 < bytes.len() && bytes[i + 1] == quote {
                i += 2;
                continue;
            }
            return Eat::Ate(i + 1);
        }
        i = next_char(source, i);
    }
    Eat::Unclosed
}

fn keyword(source: &str, i: usize, word: &str) -> bool {
    let Some(rest) = source.get(i..) else {
        return false;
    };
    let after = i + word.len();
    // `word` is ASCII, but the source bytes under it may split a multibyte
    // character. Slicing there panics; it is also not this keyword.
    if rest.len() < word.len() || !source.is_char_boundary(after) {
        return false;
    }
    if !rest[..word.len()].eq_ignore_ascii_case(word) {
        return false;
    }
    if source[after..]
        .chars()
        .next()
        .is_some_and(|ch| is_ident_char(ch))
    {
        return false;
    }
    if i > 0
        && source[..i]
            .chars()
            .next_back()
            .is_some_and(|ch| is_ident_char(ch))
    {
        return false;
    }
    true
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn next_char(source: &str, i: usize) -> usize {
    let Some(rest) = source.get(i..) else {
        // Mid-character: step one byte so the scan cannot stick or panic.
        return (i + 1).min(source.len());
    };
    match rest.chars().next() {
        Some(ch) => i + ch.len_utf8(),
        None => source.len(),
    }
}

fn same_name(found: &str, wanted: &str) -> bool {
    found == wanted || unquote(found).as_deref() == Some(wanted)
}

/// Strip one pair of `"`, `` ` `` or `[]` quotes. Doubled closers are one
/// character of the name. `None` when `name` is not a quoted identifier.
fn unquote(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let closer = match bytes[0] {
        b'"' | b'`' => bytes[0],
        b'[' => b']',
        _ => return None,
    };
    if *bytes.last()? != closer {
        return None;
    }
    let end = bytes.len() - 1;
    let mut out = String::new();
    let mut i = 1usize;
    while i < end {
        if bytes[i] == closer && i + 1 < end && bytes[i + 1] == closer {
            out.push(closer as char);
            i += 2;
            continue;
        }
        let ch = name.get(i..)?.chars().next()?;
        if i + ch.len_utf8() > end {
            return None;
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    (!out.is_empty()).then_some(out)
}

fn normalize_quoted_names(extraction: &mut Extraction) {
    let file = extraction.file_path.clone();
    let mut renames: Vec<(String, String)> = Vec::new();
    for symbol in &mut extraction.symbols {
        if symbol.kind == SymbolKind::File {
            continue;
        }
        let Some(bare) = unquote(&symbol.name) else {
            continue;
        };
        let old_qualified = symbol.qualified_name.clone();
        let old_name = symbol.name.clone();
        symbol.qualified_name = renamed(&old_qualified, &old_name, &bare, &file);
        symbol.name = bare;
        if old_qualified != symbol.qualified_name {
            renames.push((old_qualified, symbol.qualified_name.clone()));
        }
    }
    for (old, new) in &renames {
        retarget(extraction, old, new);
    }
}

/// Replace the `::{old_name}` component. A bare `strip_suffix(old_name)` would
/// also eat a shorter name off the end of a longer one (`data` / `a`).
fn renamed(qualified: &str, old_name: &str, new_name: &str, file: &str) -> String {
    let suffix = format!("::{old_name}");
    match qualified.strip_suffix(&suffix) {
        Some(prefix) => format!("{prefix}::{new_name}"),
        None => format!("{file}::{new_name}"),
    }
}

fn retarget(extraction: &mut Extraction, old: &str, new: &str) {
    if old == new {
        return;
    }
    for call in &mut extraction.calls {
        if call.caller_symbol.as_deref() == Some(old) {
            call.caller_symbol = Some(new.to_string());
        }
    }
    for reference in &mut extraction.references {
        if reference.enclosing_symbol.as_deref() == Some(old) {
            reference.enclosing_symbol = Some(new.to_string());
        }
    }
}

/// Drop declarations that are not schema objects: a symbol strictly inside a
/// skipped statement (a table written in a trigger or routine body) and any
/// symbol that starts in the unterminated tail. Edges that named the removed
/// symbol as their caller go with it. A symbol that begins at the skipped
/// statement itself — the routine — stays.
fn drop_spurious(extraction: &mut Extraction, scan: &Scan) {
    let mut removed = Vec::new();
    extraction.symbols.retain(|symbol| {
        if symbol.kind == SymbolKind::File {
            return true;
        }
        let inside_skipped = scan
            .skipped
            .iter()
            .any(|&(start, end)| symbol.span.start_byte > start && symbol.span.end_byte <= end);
        let in_tail = scan
            .unterminated_at
            .is_some_and(|at| symbol.span.start_byte >= at);
        if inside_skipped || in_tail {
            removed.push(symbol.qualified_name.clone());
            return false;
        }
        true
    });
    if removed.is_empty() {
        return;
    }
    extraction.calls.retain(|call| match &call.caller_symbol {
        Some(caller) => !removed.iter().any(|name| name == caller),
        None => true,
    });
    extraction
        .references
        .retain(|reference| match &reference.enclosing_symbol {
            Some(enclosing) => !removed.iter().any(|name| name == enclosing),
            None => true,
        });
}
