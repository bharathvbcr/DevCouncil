#![no_main]

use std::collections::BTreeSet;
use std::time::Instant;

use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_extract::treesitter::DEFAULT_PARSE_BUDGET;
use devmap_extract::{extract_file, linked_grammar_keys};
use libfuzzer_sys::fuzz_target;

#[allow(dead_code)]
mod stateful_fuzz {
    include!("../../testsupport/stateful_fuzz.rs");
}

use stateful_fuzz::{
    check, generate_micro_repo, lower_damage, mutate_file_elements, MicroLang, Source,
};

/// Cap per-exec input size so coverage-guided campaigns keep high exec/s
/// without burning libFuzzer timeout budgets on multi-megabyte inputs.
const MAX_FUZZ_SOURCE_BYTES: usize = 16_384;

/// Representative file path for every known linked grammar key. Unknown future
/// grammar keys return `None` instead of falling back to a non-source extension
/// (like `.txt`) that would return `Skipped` and false-positive `nul_byte_refused`.
fn path_for_grammar(key: &str) -> Option<&'static str> {
    let path = match key {
        "astro" => "fuzz.astro",
        "c" => "fuzz.c",
        "cfml" => "fuzz.cfm",
        "cpp" => "fuzz.cpp",
        "csharp" => "fuzz.cs",
        "cuda" => "fuzz.cu",
        "dart" => "fuzz.dart",
        "erlang" => "fuzz.erl",
        "go" => "fuzz.go",
        "hcl" => "fuzz.tf",
        "java" => "fuzz.java",
        "javascript" => "fuzz.js",
        "kotlin" => "fuzz.kt",
        "liquid" => "fuzz.liquid",
        "lua" => "fuzz.lua",
        "luau" => "fuzz.luau",
        "nix" => "fuzz.nix",
        "objc" => "fuzz.m",
        "pascal" => "fuzz.pas",
        "php" => "fuzz.php",
        "python" => "fuzz.py",
        "r" => "fuzz.r",
        "ruby" => "fuzz.rb",
        "rust" => "fuzz.rs",
        "scala" => "fuzz.scala",
        "shell" => "fuzz.sh",
        "solidity" => "fuzz.sol",
        "sql" => "fuzz.sql",
        "svelte" => "fuzz.svelte",
        "swift" => "fuzz.swift",
        "tsx" => "fuzz.tsx",
        "typescript" => "fuzz.ts",
        "vue" => "fuzz.vue",
        _ => return None,
    };
    Some(path)
}

/// Seeds that exercise `parse_repair`, `kotlinsemi`, and `sveltetext`
/// same-width ASCII rewrite paths.
const REPAIR_SEEDS: &[(&str, &str)] = &[
    (
        "repair.ts",
        "type A = typeof import(\"./dep\");\ntype B = import(\"./dep\").Widget;\ntype Tuple = [object: string, number?: number];\nfor (let i = 0; i < unique.length; i++) {}\n",
    ),
    (
        "repair.tsx",
        "export type Props = import(\"./types\").ButtonProps;\nexport function Btn(p: [string: number]) { return <div />; }\n",
    ),
    (
        "repair.rs",
        "fn raw_ptr(x: &u8) -> *const u8 { &raw const *x }\nfn pat(s: S) { let S { #[cfg(test)] a, .. } = s; }\n",
    ),
    (
        "repair.sh",
        "val=\"${foo//bar/baz}\"\nother=\"${arr[@]:1:2}\"\necho \"$val $other\"\n",
    ),
    (
        "repair.kt",
        "class Box { val x = 1 fun get() = x }\nvar prop: Int = 0; get() = field\n",
    ),
    (
        "repair.svelte",
        "<script lang=\"ts\">let a = 1; let b = 2;</script>\n<p>a & b > c</p>\n<div {@attach fn}>ok</div>\n",
    ),
];

fn fail_check(outcome: stateful_fuzz::StepOutcome) {
    if let Some(failure) = outcome.into_failure() {
        eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
        panic!("{failure}");
    }
}

fn verify_extraction(action: &'static str, path: &str, source: &str, extraction: &Extraction) {
    fail_check(check(
        extraction.file_path == path,
        "file_path_preserved",
        action,
        format!("expected {:?}, got {:?}", path, extraction.file_path),
    ));

    // NUL bytes must always be refused before grammar parsing.
    if source.as_bytes().contains(&0) && !path.ends_with(".cbl") {
        fail_check(check(
            matches!(extraction.parse_outcome, ParseOutcome::Failed { .. }),
            "nul_byte_refused",
            action,
            format!(
                "source containing NUL was not refused for {path}: {:?}",
                extraction.parse_outcome
            ),
        ));
    }

    for sym in &extraction.symbols {
        fail_check(check(
            sym.span.start_byte <= sym.span.end_byte && sym.span.end_byte <= source.len(),
            "span_in_bounds",
            action,
            format!(
                "{path} symbol {:?} span {}..{} out of {} bytes",
                sym.qualified_name,
                sym.span.start_byte,
                sym.span.end_byte,
                source.len()
            ),
        ));
        fail_check(check(
            source.is_char_boundary(sym.span.start_byte)
                && source.is_char_boundary(sym.span.end_byte),
            "span_char_boundary",
            action,
            format!(
                "{path} symbol {:?} span {}..{} splits UTF-8 codepoint",
                sym.qualified_name, sym.span.start_byte, sym.span.end_byte
            ),
        ));
        fail_check(check(
            !sym.name.is_empty(),
            "symbol_name_non_empty",
            action,
            format!("{path} emitted symbol with empty name"),
        ));
    }

    match &extraction.parse_outcome {
        ParseOutcome::Clean => {}
        ParseOutcome::Partial { error_ranges, .. } => {
            fail_check(check(
                !error_ranges.is_empty(),
                "partial_has_error_ranges",
                action,
                format!("{path} reported Partial with empty error_ranges"),
            ));
            for range in error_ranges {
                fail_check(check(
                    range.start_byte <= range.end_byte && range.end_byte <= source.len(),
                    "error_range_in_bounds",
                    action,
                    format!(
                        "{path} error range {}..{} out of {} bytes",
                        range.start_byte,
                        range.end_byte,
                        source.len()
                    ),
                ));
            }
        }
        ParseOutcome::Failed { reason }
        | ParseOutcome::Fallback { reason }
        | ParseOutcome::Skipped { reason } => {
            fail_check(check(
                !reason.trim().is_empty(),
                "non_clean_reason_non_empty",
                action,
                format!("{path} reported non-clean outcome with empty reason"),
            ));
            fail_check(check(
                extraction.calls.is_empty(),
                "no_fabricated_calls",
                action,
                format!(
                    "{path} emitted {} call(s) on {:?}",
                    extraction.calls.len(),
                    extraction.parse_outcome
                ),
            ));
        }
    }
}

fn run_and_check(action: &'static str, path: &str, source: &str) {
    let started = Instant::now();
    let first = extract_file(path, source);
    let elapsed = started.elapsed();
    let ceiling = DEFAULT_PARSE_BUDGET * 2;
    fail_check(check(
        elapsed <= ceiling,
        "bounded_extraction_time",
        action,
        format!("{path} took {elapsed:?} > {ceiling:?}"),
    ));
    verify_extraction(action, path, source, &first);

    // Wall-clock parse-budget refusals embed the measured elapsed duration in
    // `ParseOutcome::Failed { reason }` and already consumed `DEFAULT_PARSE_BUDGET`.
    // Skip the second parse so ASan jitter cannot cause false-positive timing-text
    // mismatches or trip libFuzzer's `-timeout=25` ceiling.
    if first.is_budget_refusal() {
        return;
    }

    // Byte-for-byte determinism: incremental builds rely on identical bytes
    // producing identical extractions.
    let second = extract_file(path, source);
    if second.is_budget_refusal() {
        return;
    }
    verify_extraction(action, path, source, &second);
    let first_json = serde_json::to_string(&first).expect("serialize first extraction");
    let second_json = serde_json::to_string(&second).expect("serialize second extraction");
    fail_check(check(
        first_json == second_json,
        "deterministic_extraction",
        action,
        format!("{path} produced non-deterministic extractions on identical bytes"),
    ));
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    let mut src = Source::from_bytes(data);
    let mode = src.usize_below(3);

    match mode {
        // Mode 0: Arbitrary bytes routed to any of the 33 linked grammars
        // (plus Metal, ArkTS, and refused COBOL).
        0 => {
            let keys = linked_grammar_keys();
            let mut paths: Vec<&'static str> = keys
                .iter()
                .filter_map(|key| path_for_grammar(key))
                .collect();
            paths.extend(["fuzz.metal", "fuzz.ets", "fuzz.cbl"]);
            let unique_paths: Vec<&'static str> = paths
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let path = *src.choose(&unique_paths).unwrap_or(&"fuzz.rs");
            let raw = src.bytes(MAX_FUZZ_SOURCE_BYTES);
            let text = String::from_utf8_lossy(&raw);
            run_and_check("raw_grammar", path, &text);
        }
        // Mode 1: Structure-aware MicroRepo generation + element mutation +
        // lower_damage syntax breakage.
        1 => {
            let langs = [
                MicroLang::Rust,
                MicroLang::TypeScript,
                MicroLang::Go,
                MicroLang::Python,
            ];
            let lang = *src.choose(&langs).unwrap_or(&MicroLang::Rust);
            let repo = generate_micro_repo(&mut src, &[lang], 1);
            let Some((path, mut body)) = repo.into_iter().next() else {
                return;
            };
            while src.more(3) {
                body = mutate_file_elements(&mut src, lang, &body);
            }
            if src.bool() {
                body = lower_damage(&mut src, &body);
            }
            if body.len() > MAX_FUZZ_SOURCE_BYTES {
                let mut cut = MAX_FUZZ_SOURCE_BYTES;
                while cut > 0 && !body.is_char_boundary(cut) {
                    cut -= 1;
                }
                body.truncate(cut);
            }
            run_and_check("micro_repo_mutated", &path, &body);
        }
        // Mode 2: Targeted `parse_repair` / `kotlinsemi` / `sveltetext`
        // stress with fuzz-spliced payloads and UTF-8 / NUL / delimiter damage.
        _ => {
            let &(path, seed) = src.choose(REPAIR_SEEDS).unwrap_or(&REPAIR_SEEDS[0]);
            let mut body = seed.to_string();
            let splice_raw = src.bytes(1024);
            if !splice_raw.is_empty() {
                let splice = String::from_utf8_lossy(&splice_raw);
                let mut at = src.usize_below(body.len() + 1);
                while at > 0 && !body.is_char_boundary(at) {
                    at -= 1;
                }
                body.insert_str(at, &splice);
            }
            if src.bool() {
                body = lower_damage(&mut src, &body);
            }
            if body.len() > MAX_FUZZ_SOURCE_BYTES {
                let mut cut = MAX_FUZZ_SOURCE_BYTES;
                while cut > 0 && !body.is_char_boundary(cut) {
                    cut -= 1;
                }
                body.truncate(cut);
            }
            run_and_check("parse_repair_stress", path, &body);
        }
    }
});
