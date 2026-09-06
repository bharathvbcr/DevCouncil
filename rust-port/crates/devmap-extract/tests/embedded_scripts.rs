//! The code inside a template language's `<script>` blocks.
//!
//! Before `devmap-extract::embedded`, a `.svelte`, `.vue`, `.astro` or
//! `.liquid` file was parsed by its outer grammar only. Every one of those
//! grammars models the template and hands the script body back as one opaque
//! leaf, so the measured recovery on the fixtures below was, for all four
//! languages: **1 symbol** (the `File` node), **0 imports**, **0 calls**, and
//! **1 export** (the file itself, synthesised from the `File` node) — reported
//! as `ParseOutcome::Clean`, a complete-looking answer over a file whose entire
//! code half had never been read.
//!
//! Every test here fails against that code.

use devmap_extract::model::{ParseOutcome, SymbolKind};
use devmap_extract::{extract_file, Extraction};

const SVELTE: &str = r#"<script lang="ts">
  import { onMount } from 'svelte';
  import { formatLabel } from '../lib/format';

  export const widgetId: string = 'summary';

  function computeTotal(items: number[]): number {
    return items.reduce((sum, n) => sum + n, 0);
  }

  export function renderSummary(items: number[]): string {
    return formatLabel(computeTotal(items));
  }

  onMount(() => {
    renderSummary([1, 2, 3]);
  });
</script>

<h1>{widgetId}</h1>

<style>
  h1 { color: red; }
</style>
"#;

const VUE: &str = r#"<template>
  <div class="summary">{{ label }}</div>
</template>

<script lang="ts">
import { defineComponent } from 'vue';
import { formatLabel } from '../lib/format';

export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}

function renderSummary(items: number[]): string {
  return formatLabel(computeTotal(items));
}

export default defineComponent({
  name: 'Summary',
});
</script>

<style scoped>
.summary { color: red; }
</style>
"#;

const ASTRO: &str = r#"---
import Layout from '../layouts/Layout.astro';
import { formatLabel } from '../lib/format';

export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}

function renderSummary(items: number[]): string {
  return formatLabel(computeTotal(items));
}

const summary = renderSummary([1, 2, 3]);
---
<Layout>
  <p>{summary}</p>
</Layout>
"#;

const LIQUID: &str = r#"<div class="cart">
  {{ cart.item_count }}
</div>
<script type="module">
  import { formatLabel } from "/assets/format.js";

  function computeTotal(items) {
    return items.reduce((sum, n) => sum + n, 0);
  }

  function renderSummary(items) {
    return formatLabel(computeTotal(items));
  }

  renderSummary([1, 2, 3]);
</script>
"#;

fn declared_names(extraction: &Extraction) -> Vec<&str> {
    extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.name.as_str())
        .collect()
}

fn callees(extraction: &Extraction) -> Vec<&str> {
    extraction
        .calls
        .iter()
        .map(|call| call.callee_name.as_str())
        .collect()
}

fn modules(extraction: &Extraction) -> Vec<&str> {
    extraction
        .imports
        .iter()
        .map(|import| import.module_specifier.as_str())
        .collect()
}

fn exported(extraction: &Extraction) -> Vec<&str> {
    extraction
        .exports
        .iter()
        .map(|export| export.exported_name.as_str())
        .collect()
}

/// Exactly one `File` node per file, whatever the merge does.
fn assert_single_file_node(extraction: &Extraction) {
    let files: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::File)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(
        files,
        vec![extraction.file_path.as_str()],
        "a file has exactly one File node, spanning the whole file"
    );
}

/// Every span an extraction publishes must index the file on disk.
///
/// This is the failure mode that would make embedded extraction worse than the
/// blackout it replaces: a symbol whose span is region-relative renders as
/// whatever bytes happen to sit at that offset in the real file, and reads like
/// a right answer.
fn assert_every_span_is_inside(extraction: &Extraction, source: &str) {
    for symbol in &extraction.symbols {
        assert!(
            source
                .get(symbol.span.start_byte..symbol.span.end_byte)
                .is_some(),
            "symbol {:?} span {}..{} is not a character range of the {} byte source",
            symbol.name,
            symbol.span.start_byte,
            symbol.span.end_byte,
            source.len()
        );
    }
    for call in &extraction.calls {
        assert!(
            source
                .get(call.span.start_byte..call.span.end_byte)
                .is_some(),
            "call {:?} span {}..{} is not a character range of the source",
            call.callee_name,
            call.span.start_byte,
            call.span.end_byte
        );
    }
    for import in &extraction.imports {
        assert!(
            source
                .get(import.span.start_byte..import.span.end_byte)
                .is_some(),
            "import {:?} span is not a character range of the source",
            import.module_specifier
        );
    }
    for reference in &extraction.references {
        assert!(
            source
                .get(reference.span.start_byte..reference.span.end_byte)
                .is_some(),
            "reference {:?} span is not a character range of the source",
            reference.name
        );
    }
}

#[test]
fn a_svelte_script_block_declares_symbols_imports_calls_and_exports() {
    let extraction = extract_file("src/Summary.svelte", SVELTE);

    assert_single_file_node(&extraction);
    assert_eq!(extraction.language, "svelte");
    let names = declared_names(&extraction);
    assert!(
        names.contains(&"computeTotal") && names.contains(&"renderSummary"),
        "both functions declared in the <script> block must be symbols, got {names:?}"
    );
    let modules = modules(&extraction);
    assert!(
        modules.contains(&"svelte") && modules.contains(&"../lib/format"),
        "both imports in the <script> block must be recorded, got {modules:?}"
    );
    let callees = callees(&extraction);
    assert!(
        callees.contains(&"computeTotal"),
        "the call from renderSummary to computeTotal must be recorded, got {callees:?}"
    );
    assert!(
        callees.contains(&"formatLabel"),
        "the call to the imported formatLabel must be recorded, got {callees:?}"
    );
    assert!(
        exported(&extraction).contains(&"renderSummary"),
        "`export function renderSummary` must be an export, got {:?}",
        exported(&extraction)
    );

    // The names a script block declares are minted in the outer file's
    // namespace, so a call joins the symbol it names without a second rule.
    let render = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "renderSummary")
        .expect("renderSummary is declared");
    assert_eq!(render.qualified_name, "src/Summary.svelte::renderSummary");
    assert_eq!(
        render.parent_symbol.as_deref(),
        Some("src/Summary.svelte"),
        "a top-level declaration's parent is the real file, never a parse buffer"
    );
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "computeTotal")
        .expect("the inner call is recorded");
    assert_eq!(
        call.caller_symbol.as_deref(),
        Some("src/Summary.svelte::renderSummary"),
        "the caller is named with the outer file's path, so the edge joins a node that exists"
    );

    assert_every_span_is_inside(&extraction, SVELTE);
}

#[test]
fn a_vue_script_block_declares_symbols_imports_and_calls() {
    let extraction = extract_file("src/Summary.vue", VUE);

    assert_single_file_node(&extraction);
    let names = declared_names(&extraction);
    assert!(
        names.contains(&"computeTotal") && names.contains(&"renderSummary"),
        "got {names:?}"
    );
    let modules = modules(&extraction);
    assert!(
        modules.contains(&"vue") && modules.contains(&"../lib/format"),
        "got {modules:?}"
    );
    assert!(
        callees(&extraction).contains(&"computeTotal"),
        "got {:?}",
        callees(&extraction)
    );
    assert!(
        exported(&extraction).contains(&"computeTotal"),
        "got {:?}",
        exported(&extraction)
    );
    assert_every_span_is_inside(&extraction, VUE);
}

#[test]
fn an_astro_frontmatter_fence_declares_symbols_imports_and_calls() {
    let extraction = extract_file("src/pages/index.astro", ASTRO);

    assert_single_file_node(&extraction);
    let names = declared_names(&extraction);
    assert!(
        names.contains(&"computeTotal") && names.contains(&"renderSummary"),
        "got {names:?}"
    );
    let modules = modules(&extraction);
    assert!(
        modules.contains(&"../layouts/Layout.astro") && modules.contains(&"../lib/format"),
        "got {modules:?}"
    );
    let callees = callees(&extraction);
    assert!(
        callees.contains(&"computeTotal") && callees.contains(&"renderSummary"),
        "got {callees:?}"
    );
    assert_every_span_is_inside(&extraction, ASTRO);
}

/// A Liquid `<script>` is not in the Liquid parse tree at all.
///
/// The grammar models Liquid tags; the surrounding markup arrives as one opaque
/// `template_content` leaf. Recovering this block is a text scan over that
/// leaf's own byte range, so it is the case most likely to get offsets wrong.
#[test]
fn a_liquid_script_element_declares_symbols_imports_and_calls() {
    let extraction = extract_file("sections/cart.liquid", LIQUID);

    assert_single_file_node(&extraction);
    let names = declared_names(&extraction);
    assert!(
        names.contains(&"computeTotal") && names.contains(&"renderSummary"),
        "got {names:?}"
    );
    assert_eq!(
        modules(&extraction),
        vec!["/assets/format.js"],
        "the module import inside the <script> block must be recorded"
    );
    let callees = callees(&extraction);
    assert!(
        callees.contains(&"computeTotal") && callees.contains(&"renderSummary"),
        "got {callees:?}"
    );

    // The Liquid grammar's own output must survive the merge: `{{ cart.item_count }}`
    // sits before the script block and is the outer extraction's contribution.
    assert!(
        extraction
            .references
            .iter()
            .any(|reference| reference.name == "cart"),
        "the outer grammar's own references must not be lost when regions merge"
    );
    assert_every_span_is_inside(&extraction, LIQUID);
}

/// The failure mode that would make this worse than the blackout.
///
/// A span is a byte range into the file on disk. Every consumer that renders a
/// symbol slices the file with it, and `Span::line_range` counts newlines in
/// the same bytes — so a region-relative offset does not produce a coarser
/// answer, it produces a wrong one that reads like a right one. Multi-byte text
/// before the block is what separates a byte shift from a character shift.
#[test]
fn embedded_spans_index_the_outer_file_after_multibyte_text() {
    let source = "<h1>\u{1F600}\u{1F600} caf\u{e9} na\u{ef}ve \u{4e2d}\u{6587}</h1>\n\
                  <script lang=\"ts\">\n\
                  \x20 export function afterUtf8(): number { return 41 + 1; }\n\
                  </script>\n";
    let extraction = extract_file("src/Wide.svelte", source);

    let symbol = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "afterUtf8")
        .unwrap_or_else(|| {
            panic!(
                "the declaration after multi-byte text must be recovered, got {:?}",
                declared_names(&extraction)
            )
        });
    assert_eq!(
        source.get(symbol.span.start_byte..symbol.span.end_byte),
        Some("function afterUtf8(): number { return 41 + 1; }"),
        "the span must slice the declaration out of the outer file exactly"
    );
    assert_eq!(
        symbol.span.line_range(source),
        (3, 3),
        "the declaration is on line 3 of the outer file"
    );
    assert_every_span_is_inside(&extraction, source);
}

/// R6 — failure is never emptiness.
///
/// A `<script>` whose contents do not parse must not leave the file reporting
/// `Clean`. Before this module the file reported `Clean` no matter what the
/// script contained, because nothing ever looked at it.
#[test]
fn a_script_block_that_does_not_parse_makes_the_file_partial() {
    let source =
        "<script lang=\"ts\">\n  function ok() {}\n  function broken( { { {\n</script>\n<p>x</p>\n";
    let extraction = extract_file("Broken.svelte", source);

    match &extraction.parse_outcome {
        ParseOutcome::Partial { error_ranges } => {
            assert!(
                !error_ranges.is_empty(),
                "a Partial outcome must name the bytes that were not understood"
            );
            for range in error_ranges {
                assert!(
                    source.get(range.start_byte..range.end_byte).is_some(),
                    "an error range must index the outer file: {}..{} of {} bytes",
                    range.start_byte,
                    range.end_byte,
                    source.len()
                );
                assert!(
                    range.start_byte >= 18,
                    "the error range must be relocated into the outer file, not left at the \
                     region's own offset ({}..{})",
                    range.start_byte,
                    range.end_byte
                );
            }
        }
        other => panic!("a script block that does not parse must not report Clean, got {other:?}"),
    }
    assert!(
        declared_names(&extraction).contains(&"ok"),
        "what did parse is still recovered"
    );
}

/// The registry is the single authority for what may be embedded where.
///
/// Liquid's `LanguageSpec.embedded` names `html`, `javascript` and `css`; it
/// does not name TypeScript. A `<script lang="ts">` in a `.liquid` file is
/// therefore refused — visibly, with its byte range recorded — rather than
/// parsed by a grammar the registry does not permit there. The same block in a
/// `.svelte` file, whose spec *does* name TypeScript, is extracted. Nothing but
/// the registry distinguishes the two.
#[test]
fn the_permitted_embedded_languages_come_from_the_registry() {
    let block =
        "<script lang=\"ts\">\nexport function typed(n: number): number { return n; }\n</script>\n";

    let svelte = extract_file("a.svelte", block);
    assert!(
        declared_names(&svelte).contains(&"typed"),
        "Svelte's registry entry permits TypeScript, so the block is extracted: {:?}",
        declared_names(&svelte)
    );

    let liquid = extract_file("a.liquid", block);
    assert!(
        !declared_names(&liquid).contains(&"typed"),
        "Liquid's registry entry does not permit TypeScript; the block must not be parsed as it"
    );
    assert!(
        matches!(liquid.parse_outcome, ParseOutcome::Partial { .. }),
        "a region the registry forbids is unread text, and the file must say so: {:?}",
        liquid.parse_outcome
    );
    assert!(
        liquid
            .diagnostics
            .iter()
            .any(|note| note.contains("typescript") && note.contains("not permit")),
        "the refusal must name the language and say the registry refused it: {:?}",
        liquid.diagnostics
    );
}

/// A `type` that names data rather than code is a correct answer, not a failure.
///
/// `type="application/json"` and `type="importmap"` are not executed as script
/// by a browser either. Recording them as unparsed regions would put a `Partial`
/// outcome on every page that carries an import map.
#[test]
fn a_non_script_type_is_not_an_error() {
    let source = "<script type=\"application/json\">{\"a\": 1}</script>\n<p>x</p>\n";
    let extraction = extract_file("Data.svelte", source);
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "a JSON payload is not a script that failed to parse: {:?}",
        extraction.parse_outcome
    );
    assert_eq!(declared_names(&extraction), Vec::<&str>::new());
    assert!(extraction.diagnostics.is_empty());
}

/// An empty `<script src="…">` declares nothing and that is the whole answer.
#[test]
fn a_source_only_script_element_is_a_clean_empty_answer() {
    let extraction = extract_file("Ref.svelte", "<script src=\"./x.js\"></script>\n<p>x</p>\n");
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "got {:?}",
        extraction.parse_outcome
    );
    assert!(extraction.diagnostics.is_empty());
}

/// Two script blocks in one file are one namespace, so a call in the second
/// joins a declaration in the first.
#[test]
fn declarations_and_calls_join_across_two_script_blocks() {
    let source = "<script context=\"module\">\nexport function moduleHelper() { return 1; }\n</script>\n\
                  <script>\n  function instance() { return moduleHelper(); }\n</script>\n<p>hi</p>\n";
    let extraction = extract_file("Two.svelte", source);

    assert_single_file_node(&extraction);
    let names = declared_names(&extraction);
    assert!(
        names.contains(&"moduleHelper") && names.contains(&"instance"),
        "both blocks contribute declarations, got {names:?}"
    );
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "moduleHelper")
        .expect("the call from the second block to the first is recorded");
    assert_eq!(
        call.caller_symbol.as_deref(),
        Some("Two.svelte::instance"),
        "both blocks mint names in the same file namespace"
    );
    assert_every_span_is_inside(&extraction, source);
}

/// Liquid tags inside a script body split the element across two
/// `template_content` runs. The honest answer is that this build cannot read
/// the block — not a body that stops at the tag.
#[test]
fn an_unterminated_liquid_script_is_reported_rather_than_guessed() {
    let source =
        "<script>\n  var cart = {{ cart | json }};\n  function go() { return cart; }\n</script>\n";
    let extraction = extract_file("split.liquid", source);

    assert!(
        !declared_names(&extraction).contains(&"go"),
        "a block whose end was never found must not yield a truncated body's declarations"
    );
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
        "unread bytes must not report Clean: {:?}",
        extraction.parse_outcome
    );
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|note| note.contains("unterminated <script>")),
        "got {:?}",
        extraction.diagnostics
    );
}

/// R7 — the count travels with the truncation, computed where the truncating
/// happens, and the bytes nobody read are recorded on the outcome.
#[test]
fn regions_past_the_cap_are_counted_and_their_range_recorded() {
    let mut source = String::new();
    let blocks = 300;
    for index in 0..blocks {
        source.push_str(&format!(
            "<script>\n  function fn{index}() {{ return {index}; }}\n</script>\n"
        ));
    }
    let extraction = extract_file("Many.svelte", &source);

    let declared = declared_names(&extraction).len();
    assert_eq!(
        declared, 256,
        "exactly the capped number of regions may contribute declarations"
    );
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|note| note.contains("300") && note.contains("256") && note.contains("44")),
        "the diagnostic must carry located, extracted and dropped counts: {:?}",
        extraction.diagnostics
    );
    match &extraction.parse_outcome {
        ParseOutcome::Partial { error_ranges } => {
            assert!(
                error_ranges
                    .iter()
                    .all(|range| source.get(range.start_byte..range.end_byte).is_some()),
                "the dropped regions' range must index the outer file"
            );
        }
        other => panic!("dropped regions are unread bytes and must not report Clean: {other:?}"),
    }
    assert_every_span_is_inside(&extraction, &source);
}

/// R4 — two extractions of one file are byte-identical.
#[test]
fn embedded_extraction_is_deterministic() {
    for (path, source) in [
        ("src/Summary.svelte", SVELTE),
        ("src/Summary.vue", VUE),
        ("src/pages/index.astro", ASTRO),
        ("sections/cart.liquid", LIQUID),
    ] {
        let first = serde_json::to_string(&extract_file(path, source)).expect("serialisable");
        let second = serde_json::to_string(&extract_file(path, source)).expect("serialisable");
        assert_eq!(first, second, "{path} must extract identically twice");
    }
}

/// The cache key must change when what a cached payload *means* changes.
///
/// Two things changed at once. The schema version is bumped because a stored
/// v29 payload for any of these four languages is the outer grammar's answer
/// alone — one `File` node under a `Clean` outcome — and reusing it would leave
/// every component in the tree symbol-less while looking freshly indexed. And a
/// `.svelte` payload now depends on the *TypeScript* grammar, so the identity
/// has to name it: keying on `tree-sitter-svelte-ng` alone would serve those
/// rows back unchanged across a TypeScript grammar bump that changes all of
/// them.
#[test]
fn the_cache_identity_covers_the_embedded_grammars() {
    use devmap_extract::cache::{grammar_version_for, EXTRACTION_SCHEMA_VERSION};

    // An exact pin, deliberately: it is the tripwire that makes somebody read
    // `EXTRACTION_SCHEMA_VERSION`'s doc comment and write a rationale paragraph
    // before changing what a cached payload contains. It has fired three times
    // — 30 -> 31 for reading `<script>` blocks, 31 -> 32 for W1.2's heritage
    // references and W3.3's wiring annotations, and 32 -> 33 for W0.3 move 2's
    // import extraction across nineteen grammar keys. All three are additive,
    // which is exactly why each needed the bump: an additive change leaves a
    // stale row looking perfectly complete.
    assert_eq!(
        EXTRACTION_SCHEMA_VERSION, "33",
        "reading <script> blocks changes what a cached payload means, and so does \
         every later addition to it"
    );

    for language in ["svelte", "vue", "astro"] {
        let identity = grammar_version_for(language);
        assert!(
            identity.contains("tree-sitter-typescript"),
            "{language} embeds TypeScript, so its payload identity must name that grammar: \
             {identity}"
        );
        assert!(
            identity.contains("tree-sitter-javascript"),
            "{language} embeds JavaScript too: {identity}"
        );
    }
    let liquid = grammar_version_for("liquid");
    assert!(
        liquid.contains("tree-sitter-javascript"),
        "liquid embeds JavaScript: {liquid}"
    );
    assert!(
        !liquid.contains("tree-sitter-typescript"),
        "liquid's registry entry does not permit TypeScript, so its identity must not claim it: \
         {liquid}"
    );

    // A language that embeds nothing keeps a bare identity.
    let python = grammar_version_for("python");
    assert!(
        !python.contains("+embedded"),
        "a language with no embedded list gains no suffix: {python}"
    );
}

/// A Vue SFC may be written in TSX, and only Vue may.
///
/// `<script lang="tsx">` is a Vue single-file-component form its own compiler
/// accepts: Vue render functions are routinely written as JSX/TSX. The reader
/// already maps `lang="tsx"` to the registry's `tsx`, but `tsx` was in no
/// spec's `embedded` list, so the region was refused and its bytes recorded
/// unparsed — honest, and still a blackout over the whole script.
///
/// The negative half is the half worth having. Svelte and Astro are *not*
/// given `tsx`: Svelte's template is its own language and not JSX, and an
/// Astro `<script>` is plain JS/TS — Astro components that use JSX are `.jsx`
/// or `.tsx` files, which the registry already claims by extension. Permitting
/// `tsx` there would parse a region under a grammar its framework never
/// compiles it with, which is how a confidently-wrong symbol gets made.
///
/// `lang="jsx"` needs no entry: tree-sitter-javascript parses JSX, so it
/// already routes through `javascript`.
#[test]
fn a_vue_component_written_in_tsx_is_read_and_only_vue_gets_tsx() {
    const VUE_TSX: &str = r#"<template>
  <div>{{ label }}</div>
</template>

<script lang="tsx">
import { defineComponent } from 'vue';

function badge(count: number) {
  return <span class="badge">{count}</span>;
}

export default defineComponent({
  render() {
    return badge(3);
  },
});
</script>
"#;

    let extraction = extract_file("src/Badge.vue", VUE_TSX);
    let names: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert!(
        names.contains(&"badge"),
        "a TSX render helper in a .vue script block must be a symbol, got {names:?}"
    );
    assert!(
        extraction
            .calls
            .iter()
            .any(|call| call.callee_name == "badge"),
        "the call to the TSX helper must be recovered, got {:?}",
        extraction
            .calls
            .iter()
            .map(|c| &c.callee_name)
            .collect::<Vec<_>>()
    );
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "a TSX block Vue itself compiles must not leave the file Partial, got {:?}",
        extraction.parse_outcome
    );

    // The negative: the same script block in a Svelte file stays refused.
    const SVELTE_TSX: &str = r#"<script lang="tsx">
function badge(count: number) {
  return <span class="badge">{count}</span>;
}
</script>

<h1>badge</h1>
"#;
    let svelte = extract_file("src/Badge.svelte", SVELTE_TSX);
    assert!(
        !svelte.symbols.iter().any(|symbol| symbol.name == "badge"),
        "Svelte must not be given TSX: its template is not JSX and its compiler \
         never treats a script block as TSX"
    );
}
