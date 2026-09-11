//! Four surfaces that read part of a file and reported a whole one.
//!
//! Each is the same rule — R7, counts never lie, and its Class A parent: a
//! check that could not run must never report what a check that ran and passed
//! reports. They are grouped because the fix for each is the same move, and
//! because `cache.rs:150` already records this exact class being fixed once for
//! the fallback path: **the case was fixed, the class was not.**
//!
//! * A notebook past the 5,000-cell cap recorded the cap in `diagnostics`, and
//!   `for_durable_store()` — the payload actually written to the store and the
//!   extract cache — clears `diagnostics`. The stored outcome read `Clean`, and
//!   `cache_admits(Clean)` is true, so a capped sample was persisted as complete
//!   coverage under a real content hash.
//! * The pattern scanner skips a line over `MAX_LINE_BYTES` and counts nothing,
//!   so its "N declaration(s) recovered" is a count of what it kept, presented
//!   as a count of what is there. Generated `.proto` and `.ps1` routinely carry
//!   long lines.
//! * `c_declaration_head` sliced a 256-byte window with `.get()` and fell back
//!   to `""` when the boundary split a multi-byte character — so every question
//!   asked of that head answered "no", and a CUDA kernel lost the entry-point
//!   exemption that keeps it out of the dead-code report.

use devmap_extract::extract_file;
use devmap_extract::model::ParseOutcome;

/// E-5. The head window is 256 bytes; an em-dash is 3. Byte 256 lands inside
/// the 81st one, so the naive slice is not on a char boundary.
#[test]
fn a_declaration_head_survives_a_multibyte_boundary() {
    let padding = "\u{2014}".repeat(100); // em-dashes, 3 bytes each
    let cuda = format!("__global__ /* {padding} */ void kern(int* p) {{}}\n");
    assert!(
        !cuda.is_char_boundary(256),
        "this fixture only tests what it claims if byte 256 splits a character"
    );

    let extraction = extract_file("k.cu", &cuda);
    assert!(
        !extraction.wiring.is_empty(),
        "`__global__` is at bytes 0-10, well inside the head window, but a comment \
         further along made the whole head unreadable. The kernel loses its \
         entry-point exemption and becomes a dead-code candidate. wiring={:?}",
        extraction.wiring
    );
}

/// The control: the same declaration with ASCII padding already worked, so a
/// fix must not be "always return a head" or "never look at the head".
#[test]
fn an_ascii_declaration_head_still_works() {
    let cuda = format!(
        "__global__ /* {} */ void kern(int* p) {{}}\n",
        "x".repeat(400)
    );
    let extraction = extract_file("k.cu", &cuda);
    assert!(
        !extraction.wiring.is_empty(),
        "the ASCII control must keep finding the entry point: {:?}",
        extraction.wiring
    );
}

/// E-3. A notebook past the cell cap must say so where the store can see it.
#[test]
fn a_capped_notebook_says_so_in_its_durable_outcome() {
    const CELLS: usize = 5_100;
    let mut cells = Vec::with_capacity(CELLS);
    for i in 0..CELLS {
        cells.push(format!(
            r#"{{"cell_type":"code","source":["def fn_{i}():\n","    return {i}\n"]}}"#
        ));
    }
    let notebook = format!(
        r#"{{"metadata":{{"language_info":{{"name":"python"}}}},"cells":[{}]}}"#,
        cells.join(",")
    );

    let extraction = extract_file("big.ipynb", &notebook);
    let durable = extraction.for_durable_store();

    assert!(
        !matches!(durable.parse_outcome, ParseOutcome::Clean),
        "{CELLS} cells were present and 5000 were read, but the durable outcome is \
         `Clean` — indistinguishable from a notebook that was read in full. \
         `for_durable_store()` clears `diagnostics`, so the only record of the cap \
         is erased on the way to the store, and `cache_admits(Clean)` then pins it \
         under a real content hash. durable.diagnostics={:?}",
        durable.diagnostics
    );

    let reason = match &durable.parse_outcome {
        ParseOutcome::Partial { .. } => "partial".to_string(),
        ParseOutcome::Failed { reason }
        | ParseOutcome::Fallback { reason }
        // A notebook is never skipped — the rule matches minified bundles by
        // name — but if one ever were, its reason is the string this asserts
        // on just as much as the other two.
        | ParseOutcome::Skipped { reason } => reason.clone(),
        ParseOutcome::Clean => String::new(),
    };
    assert!(
        reason.contains(&CELLS.to_string()) || reason.contains("5000") || reason.contains("cell"),
        "the outcome must name the cap so a caller can tell a prefix from a set, got {reason:?}"
    );
}

/// The control: a notebook under the cap is read completely and must stay
/// `Clean`. Without this, marking every notebook degraded would pass above.
#[test]
fn a_small_notebook_is_still_clean() {
    let notebook = r##"{"metadata":{"language_info":{"name":"python"}},"cells":[
        {"cell_type":"code","source":["def alpha():\n","    return 1\n"]},
        {"cell_type":"markdown","source":["# heading\n"]},
        {"cell_type":"code","source":["def beta():\n","    return 2\n"]}
    ]}"##;
    let extraction = extract_file("small.ipynb", notebook);
    let durable = extraction.for_durable_store();
    assert!(
        matches!(durable.parse_outcome, ParseOutcome::Clean),
        "a notebook read in full must stay Clean: {:?}",
        durable.parse_outcome
    );
    let names: Vec<&str> = durable.symbols.iter().map(|s| s.name.as_str()).collect();
    for expected in ["alpha", "beta"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
}

/// E-4. A declaration skipped for line length is still a declaration that is
/// there and is not in the list.
#[test]
fn the_pattern_scanner_counts_the_lines_it_skipped() {
    // `.proto` has no grammar, so this goes down the tier-2 pattern path.
    let long_name = "L".repeat(2_500);
    let proto = format!(
        "syntax = \"proto3\";\n\
         message Short {{}}\n\
         message {long_name} {{}}\n\
         message Tail {{}}\n"
    );

    let extraction = extract_file("p.proto", &proto);
    let names: Vec<&str> = extraction.symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"Short") && names.contains(&"Tail"),
        "the two short declarations must still be recovered: {names:?}"
    );

    let reason = match &extraction.parse_outcome {
        ParseOutcome::Fallback { reason } | ParseOutcome::Failed { reason } => reason.clone(),
        other => panic!("a grammarless file must not report {other:?}"),
    };
    assert!(
        reason.contains("never scanned"),
        "the file declares three messages and two were recovered; the reason must \
         disclose that a line was never pattern-matched at all, or a caller reads the \
         count as a set. reason={reason:?}"
    );
    assert!(
        !reason.contains("2 of 2"),
        "the scanner never matched the third line, so it cannot know the total is 2; \
         stating one is the same overclaim in a smaller font. reason={reason:?}"
    );
}

/// The control: with no over-long line, the reason must NOT claim anything was
/// skipped. A fix that always appends "some lines were skipped" is useless.
#[test]
fn a_scan_that_skipped_nothing_does_not_claim_it_did() {
    let proto = "syntax = \"proto3\";\nmessage Short {}\nmessage Tail {}\n";
    let extraction = extract_file("p.proto", proto);
    let reason = match &extraction.parse_outcome {
        ParseOutcome::Fallback { reason } | ParseOutcome::Failed { reason } => reason.clone(),
        other => panic!("a grammarless file must not report {other:?}"),
    };
    assert!(
        !reason.contains("skipped") && !reason.contains("too long"),
        "nothing was skipped in this file; claiming otherwise makes the signal \
         meaningless. reason={reason:?}"
    );
}
