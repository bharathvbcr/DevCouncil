//! Notebook container failures must survive durable extraction serialization.

use devmap_extract::{extract_file, ParseOutcome};
use serde_json::json;

#[test]
fn notebook_cache_identity_covers_each_kernel_grammar() {
    let identity = devmap_extract::cache::grammar_version_for("notebook");
    for language in ["python", "r", "julia", "typescript", "rust", "scala"] {
        let grammar = devmap_extract::cache::grammar_version_for(language);
        assert!(
            identity.contains(&grammar),
            "notebook key {identity:?} omits selectable {language} grammar {grammar:?}"
        );
    }
}

#[test]
fn independent_cells_cannot_complete_each_others_syntax() {
    let mut failures = Vec::new();
    for (language, sources) in [
        ("python", ["def cross_cell():\n", "    return helper()\n"]),
        ("rust", ["fn cross_cell() {\n", " helper(); }\n"]),
        ("typescript", ["function crossCell() {\n", " helper(); }\n"]),
    ] {
        let raw = json!({"metadata":{"language_info":{"name":language}}, "cells":sources.map(|source| json!({"cell_type":"code","source":source}))}).to_string();
        let extraction = extract_file("units.ipynb", &raw);
        if matches!(extraction.parse_outcome, ParseOutcome::Clean) {
            failures.push(format!(
                "{language} cells borrowed each other's syntax to fabricate Clean"
            ));
        }
        if extraction.calls.iter().any(|call| {
            call.callee_name == "helper"
                && call.caller_symbol.as_deref().is_some_and(|caller| {
                    caller.contains("cross_cell") || caller.contains("crossCell")
                })
        }) {
            failures.push(format!(
                "{language} attributed a cell2 call to a cell1 declaration"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn cell_local_offsets_do_not_alias_other_cells_bindings() {
    let raw = json!({"metadata":{"language_info":{"name":"python"}}, "cells":[
        {"cell_type":"code","source":"def first(callback):\n    callback()\n"},
        {"cell_type":"code","source":"def other(callback):\n    callback()\n"}
    ]})
    .to_string();
    let extraction = extract_file("scopes.ipynb", &raw);
    let calls: Vec<_> = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "callback")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].span.start_byte, calls[1].span.start_byte);
    for call in calls {
        let binding = extraction
            .local_binding_at(call.span.start_byte, "callback")
            .expect("each cell's site binding survives");
        assert_eq!(binding.scope, call.caller_symbol);
    }
    assert_eq!(
        extraction
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == devmap_extract::SymbolKind::File)
            .count(),
        1
    );
}

#[test]
fn python_empty_required_suites_are_partial() {
    for source in [
        "def incomplete():\n",
        "class Incomplete:\n",
        "if ready:\n",
        "for item in values:\n",
        "while ready:\n",
        "with handle:\n",
        "def incomplete():\n    # only a comment\n",
        "async def incomplete():\n",
        "async for item in values:\n",
        "async with handle:\n",
        "try:\n",
        "try:\n    pass\nexcept Exception:\n",
        "try:\n    pass\nfinally:\n",
        "if ready:\n    pass\nelse:\n",
        "if ready:\n    pass\nelif other:\n",
        "match value:\n    case _:\n",
    ] {
        let extraction = extract_file("suite.py", source);
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
            "an empty required suite was reported complete: {source:?}: {:?}",
            extraction.parse_outcome
        );
    }
    for source in [
        "",
        "# only a comment\n",
        "def complete():\n    pass\n",
        "def documented():\n    '''a docstring is a statement'''\n",
        "if ready:\n    pass\nelse:\n    pass\n",
        "async def complete():\n    pass\n",
        "try:\n    pass\nexcept Exception:\n    pass\nfinally:\n    pass\n",
        "match value:\n    case _:\n        pass\n",
    ] {
        assert!(
            matches!(
                extract_file("suite.py", source).parse_outcome,
                ParseOutcome::Clean
            ),
            "valid control must remain Clean: {source:?}"
        );
    }
}

#[test]
fn identical_anonymous_callbacks_keep_distinct_raw_binding_sites() {
    let code = "items.map((callback) => callback());\n";
    let raw = json!({"metadata":{"language_info":{"name":"typescript"}}, "cells":[
        {"cell_type":"code","source":code}, {"cell_type":"code","source":code}
    ]})
    .to_string();
    let extraction = extract_file("callbacks.ipynb", &raw);
    let calls: Vec<_> = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "callback")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].span.start_byte, calls[1].span.start_byte);
    for call in calls {
        assert!(
            extraction
                .local_binding_at(call.span.start_byte, "callback")
                .is_some(),
            "restarted parser offsets lost the callback binding"
        );
    }
}

#[test]
fn a_notebook_with_only_prose_before_the_cell_cap_is_incomplete() {
    let mut cells = vec![json!({"cell_type": "markdown", "source": ["prose"]}); 5_000];
    cells.push(json!({"cell_type": "code", "source": ["def hidden():\n    pass\n"]}));
    let raw =
        json!({"metadata": {"language_info": {"name": "python"}}, "cells": cells}).to_string();
    let durable = extract_file("capped.ipynb", &raw).for_durable_store();
    assert!(
        !matches!(durable.parse_outcome, ParseOutcome::Clean),
        "unread code disappeared behind a Clean prose-only prefix: {:?}",
        durable.parse_outcome
    );
}

#[test]
fn malformed_code_cell_sources_are_never_clean() {
    for source in [
        json!(null),
        json!(42),
        json!(["def recovered():\n", 42, "    pass\n"]),
    ] {
        let raw = json!({"metadata": {"language_info": {"name": "python"}}, "cells": [{"cell_type": "code", "source": source}]}).to_string();
        let durable = extract_file("invalid.ipynb", &raw).for_durable_store();
        assert!(
            !matches!(durable.parse_outcome, ParseOutcome::Clean),
            "malformed source was silently discarded: {raw}"
        );
    }
}

#[test]
fn a_code_cell_with_missing_source_is_never_clean() {
    let raw = json!({"metadata": {"language_info": {"name": "python"}}, "cells": [{"cell_type": "code"}]}).to_string();
    let durable = extract_file("missing.ipynb", &raw).for_durable_store();
    assert!(
        !matches!(durable.parse_outcome, ParseOutcome::Clean),
        "missing source was silently discarded"
    );
}

#[test]
fn declarations_cannot_relocate_into_markdown_copies() {
    let raw = r#"{"metadata":{"language_info":{"name":"python"}},"cells":[{"cell_type":"markdown","source":["def actual():\n"]},{"cell_type":"code","source":["def actual():\n","    pass\n"]}]}"#;
    let extraction = extract_file("copies.ipynb", raw);
    let symbol = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "actual")
        .expect("declaration extracted");
    assert!(
        symbol.span.start_byte > raw.find("\"cell_type\":\"code\"").expect("code cell"),
        "declaration was attributed to prose: {:?}",
        symbol.span
    );
}

#[test]
fn notebook_imports_errors_and_lexical_bindings_use_raw_offsets() {
    let raw = r#"{"metadata":{"language_info":{"name":"python"}},"cells":[{"cell_type":"code","source":["import sys\n","def run(callback):\n","    callback()\n"]}]}"#;
    let extraction = extract_file("bindings.ipynb", raw);
    assert_eq!(
        &raw[extraction.imports[0].span.start_byte..extraction.imports[0].span.end_byte],
        "import sys"
    );
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "callback")
        .expect("callback call");
    assert!(
        extraction
            .local_binding_at(call.span.start_byte, "callback")
            .is_some(),
        "notebook lost its exact-site lexical binding: {:?}",
        extraction.local_bindings
    );
    assert!(extraction
        .scope_locals
        .iter()
        .any(|(owner, name)| owner == "bindings.ipynb::run" && name == "callback"));

    let broken = raw.replace("def run(callback):", "def run(:");
    let extraction = extract_file("errors.ipynb", &broken);
    let ParseOutcome::Partial { error_ranges } = extraction.parse_outcome else {
        panic!("fixture must parse Partial");
    };
    assert!(!error_ranges.is_empty());
    for range in error_ranges {
        assert!(
            range.start_byte >= broken.find("def run").expect("code"),
            "parse error points into notebook metadata: {range:?}"
        );
    }
}

#[test]
fn unicode_json_escapes_relocate_without_dropping_symbols() {
    let raw = r#"{"metadata":{"language_info":{"name":"python"}},"cells":[{"cell_type":"code","source":["def caf\u00e9():\n","    return '\ud83d\ude00'\n"]}]}"#;
    let extraction = extract_file("unicode.ipynb", raw);
    let symbol = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "café")
        .expect("escaped unicode declaration must survive");
    assert!(
        raw.get(symbol.span.start_byte..symbol.span.end_byte)
            .is_some_and(|text| text.contains("caf\\u00e9")),
        "span must cover actual escaped declaration: {:?}",
        symbol.span
    );
}

#[test]
fn all_copies_outside_the_actual_source_are_ignored() {
    let raw = r#"{"metadata":{"language_info":{"name":"python"},"example":"def actual():\n"},"cells":[{"cell_type":"markdown","source":["def actual():\n"]},{"cell_type":"code","outputs":[{"text":"def actual():\n"}],"source":["def actual():\n","    pass\n"]}]}"#;
    let extraction = extract_file("copies.ipynb", raw);
    let symbol = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "actual")
        .expect("declaration");
    assert_eq!(
        symbol.span.start_byte,
        raw.rfind("def actual").expect("actual source")
    );
}

#[test]
fn identical_code_cells_keep_distinct_source_positions() {
    let raw = r#"{"metadata":{"language_info":{"name":"python"}},"cells":[{"cell_type":"code","source":"def duplicate():\n    pass\n"},{"source":"def duplicate():\n    pass\n","cell_type":"code"}]}"#;
    let extraction = extract_file("duplicate.ipynb", raw);
    let positions: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "duplicate")
        .map(|symbol| symbol.span.start_byte)
        .collect();
    let expected: Vec<_> = raw
        .match_indices("def duplicate")
        .map(|(offset, _)| offset)
        .collect();
    assert_eq!(positions, expected);
    assert_eq!(positions.len(), 2);
}

#[test]
fn reordered_and_duplicate_json_fields_follow_the_deserialized_value() {
    let raw = r#"{"cells":[{"cell_type":"code","source":"def obsolete():\n pass\n"}],"metadata":{"language_info":{"name":"python"}},"cells":[{"source":"def stale():\n pass\n","so\u0075rce":["def active():\n","    pass\n"],"cell_type":"code"}]}"#;
    let extraction = extract_file("keys.ipynb", raw);
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "{:?}",
        extraction.parse_outcome
    );
    assert!(!extraction
        .symbols
        .iter()
        .any(|symbol| matches!(symbol.name.as_str(), "obsolete" | "stale")));
    let active = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "active")
        .expect("last source wins");
    assert_eq!(
        active.span.start_byte,
        raw.find("def active").expect("selected source")
    );
}

#[test]
fn chunks_need_not_align_with_lines_and_escapes_preserve_site_bindings() {
    for chunks in [
        json!([
            "def caf",
            "é(callback):\n",
            "    text = '\\\"😀'\n",
            "    call",
            "back()\n"
        ]),
        json!("def café(callback):\n    text = '\\\"😀'\n    callback()\n"),
    ] {
        let raw = json!({"cells": [{"cell_type":"code", "source":chunks}], "metadata":{"language_info":{"name":"python"}}}).to_string();
        let extraction = extract_file("chunks.ipynb", &raw);
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{:?}",
            extraction.parse_outcome
        );
        assert!(extraction
            .symbols
            .iter()
            .any(|symbol| symbol.name == "café"));
        let call = extraction
            .calls
            .iter()
            .find(|call| call.callee_name == "callback")
            .expect("callback");
        assert!(extraction
            .local_binding_at(call.span.start_byte, "callback")
            .is_some());
        for span in extraction
            .symbols
            .iter()
            .map(|symbol| &symbol.span)
            .chain(extraction.calls.iter().map(|call| &call.span))
        {
            assert!(
                raw.get(span.start_byte..span.end_byte).is_some(),
                "invalid UTF8 span: {span:?}"
            );
        }
        let durable = extraction.for_durable_store();
        let restored: devmap_extract::Extraction =
            serde_json::from_str(&serde_json::to_string(&durable).expect("serialize"))
                .expect("deserialize");
        assert!(restored
            .local_binding_at(call.span.start_byte, "callback")
            .is_some());
        assert!(!format!("{restored:?}").contains("chunks.ipynb.py"));
    }
}

#[test]
fn notebook_exports_and_symbol_wiring_survive_projection() {
    let raw = json!({"cells": [{"cell_type":"code", "source":"export function active() {}\n"}], "metadata":{"language_info":{"name":"typescript"}}}).to_string();
    let extraction = extract_file("exports.ipynb", &raw);
    let export = extraction
        .exports
        .iter()
        .find(|export| export.exported_name == "active")
        .expect("notebook export");
    assert!(raw
        .get(export.span.start_byte..export.span.end_byte)
        .is_some_and(|text| text.contains("active")));
    assert!(!format!("{extraction:?}").contains("exports.ipynb.ts"));

    let raw = json!({"cells": [{"cell_type":"code", "source":"#[no_mangle]\npub extern \"C\" fn entry() {}\n"}], "metadata":{"language_info":{"name":"rust"}}}).to_string();
    let extraction = extract_file("wiring.ipynb", &raw);
    assert!(
        extraction
            .wiring
            .iter()
            .any(
                |annotation| annotation.target_symbol == "wiring.ipynb::entry"
                    && annotation.kind == devmap_extract::WiringKind::RuntimeEntryPoint
            ),
        "symbol wiring was dropped: {:?}",
        extraction.wiring
    );

    let raw = json!({"cells": [{"cell_type":"code", "source":"@app.get('/health')\ndef health():\n    return 1\n"}], "metadata":{"language_info":{"name":"python"}}}).to_string();
    let extraction = extract_file("routes.ipynb", &raw);
    assert!(!extraction.routes.is_empty(), "routes were dropped");
    for route in &extraction.routes {
        assert!(raw
            .get(route.span.start_byte..route.span.end_byte)
            .is_some_and(|text| text.contains("app.get")));
    }
}

#[test]
fn the_public_notebook_entry_point_enforces_the_input_limit() {
    let raw = json!({"metadata":{"language_info":{"name":"python"}, "padding":"x".repeat(devmap_extract::MAX_SOURCE_BYTES as usize)}, "cells":[{"cell_type":"code", "source":"def present():\n    pass\n"}]}).to_string();
    let called = std::cell::Cell::new(false);
    let extraction = devmap_extract::notebook::extract_notebook(
        "large.ipynb",
        &raw,
        |path, language, source| {
            called.set(true);
            devmap_extract::extract_treesitter(path, language, source)
        },
    );
    assert!(
        !called.get(),
        "oversized input reached parsing through the public notebook API"
    );
    assert!(matches!(
        extraction.parse_outcome,
        ParseOutcome::Failed { .. }
    ));
}
