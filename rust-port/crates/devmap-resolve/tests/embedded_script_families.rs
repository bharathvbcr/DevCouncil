//! A template file's `<script>` is code, and the resolver has to know whose.
//!
//! `crates/devmap-extract/src/embedded.rs` routes the `<script>` region of a
//! `.svelte`, `.vue`, `.astro` or `.liquid` file back through the TypeScript or
//! JavaScript grammar, so those files carry real symbols and real calls. The
//! outer file's `Extraction.language` stays `"svelte"` — and `LangFamily` had
//! no arm for it, so every one of those calls arrived in `LangFamily::Generic`.
//!
//! `Generic` is not a neutral default. The global rung filtered candidates with
//! `*candidate_family == family`, so `Generic` was a single shared namespace,
//! and the two failures below were both measured against the release binary on
//! a four-file corpus before this was fixed:
//!
//! ```text
//! src/Widget.svelte::renderWidget  --Calls 0.9-->  contracts/Vault.sol::Vault.helperOnlyInSolidity
//! ```
//!
//! a Svelte function bound to a Solidity contract method at high confidence —
//! and, in the same build, `src/Widget.svelte::callGlobal`'s call to a real
//! `helpers.ts` export produced **no edge at all**, while the byte-identical
//! call from `src/other.ts` resolved at 0.9.
//!
//! The negative assertion is the one that matters. A missing edge is a gap a
//! reader can see; a fabricated one is a claim that code exists which does not,
//! which is what PLAN.md §3.1 Class C is about.

use std::collections::BTreeSet;

use devmap_extract::extract_file;
use devmap_extract::languages::LANGUAGE_SPECS;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{LangFamily, ResolutionResult};
use devmap_resolve::Resolver;

const SOLIDITY: &str = r#"pragma solidity ^0.8.0;
contract Vault {
    function helperOnlyInSolidity(uint256 x) public pure returns (uint256) {
        return x + 1;
    }
}
"#;

const HELPERS_TS: &str = "export function bareGlobal(y: number): number { return y * 2; }\n\
                          export function tsHelper(y: number): number { return y + 3; }\n";

const OTHER_TS: &str =
    "export function callGlobalFromTs(y: number): number { return bareGlobal(y); }\n";

const WIDGET_SVELTE: &str = r#"<script lang="ts">
  export function renderWidget(x) {
    return helperOnlyInSolidity(x);
  }
  export function callGlobal(y) {
    return bareGlobal(y);
  }
</script>
<div>widget</div>
"#;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions)
}

fn call_edges(result: &ResolutionResult) -> Vec<String> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| {
            format!(
                "{} -> {} @{}",
                edge.source_symbol, edge.target_symbol, edge.confidence.0
            )
        })
        .collect()
}

/// A call in a template file's script must not bind to another language.
///
/// The fixture is the reproduction, verbatim. `helperOnlyInSolidity` is
/// declared in exactly one place in the corpus and nowhere reachable from
/// JavaScript, so the *only* way an edge appears is the shared-`Generic`
/// namespace. Correct behaviour is no edge and an `unresolved` row.
#[test]
fn a_svelte_call_does_not_bind_to_a_solidity_method() {
    let result = resolve(&[
        ("contracts/Vault.sol", SOLIDITY),
        ("src/Widget.svelte", WIDGET_SVELTE),
        ("src/helpers.ts", HELPERS_TS),
        ("src/other.ts", OTHER_TS),
    ]);

    let crossing: Vec<&String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .filter(|edge| edge.source_file.ends_with(".svelte") && edge.target_file.ends_with(".sol"))
        .map(|edge| &edge.target_symbol)
        .collect();
    assert!(
        crossing.is_empty(),
        "a Svelte script bound to a Solidity contract method: {crossing:?}\n\
         all call edges: {:#?}",
        call_edges(&result)
    );

    // Not bought by dropping the call: it must still be *recorded*, because a
    // dropped call is indistinguishable from no call having been made (R5).
    assert!(
        result
            .unresolved
            .iter()
            .any(|reference| reference.callee_name == "helperOnlyInSolidity"),
        "the unresolvable call vanished instead of landing in the ledger: {:#?}",
        result
            .unresolved
            .iter()
            .map(|u| &u.callee_name)
            .collect::<Vec<_>>()
    );
}

/// The same call from a template script and from a `.ts` file resolves the same.
///
/// This is the half the fix has to buy back. Refusing the Solidity edge is easy
/// to get by refusing everything, and a Svelte file that resolves nothing is
/// not a fix — it is the K2 shape, a language silently absent from the graph.
#[test]
fn a_svelte_script_resolves_against_its_typescript_siblings() {
    let result = resolve(&[
        ("contracts/Vault.sol", SOLIDITY),
        ("src/Widget.svelte", WIDGET_SVELTE),
        ("src/helpers.ts", HELPERS_TS),
        ("src/other.ts", OTHER_TS),
    ]);

    let targets_of = |caller: &str| -> BTreeSet<String> {
        result
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == caller)
            .map(|edge| edge.target_symbol.clone())
            .collect()
    };

    // The control: the identical call from a plain `.ts` file.
    let from_ts = targets_of("src/other.ts::callGlobalFromTs");
    assert!(
        from_ts.contains("src/helpers.ts::bareGlobal"),
        "the control call did not resolve; the fixture proves nothing: {from_ts:?}"
    );

    let from_svelte = targets_of("src/Widget.svelte::callGlobal");
    assert_eq!(
        from_svelte,
        from_ts,
        "the same call resolves from a .ts file and not from a .svelte script; \
         all call edges: {:#?}",
        call_edges(&result)
    );
}

/// A relative import in a template script resolves like one in a `.ts` file.
///
/// `resolve_import_path`'s JS/TS arm was gated on
/// `matches!(lang, "javascript" | "typescript" | "tsx")`, a hand-listed set that
/// had drifted from `LangFamily::from_lang` — it omitted `jsx` as well as every
/// embedded-script host. The consequence was not a missing edge but a *wrong
/// classification*: `./helpers` resolved to no indexed file, so the binding
/// landed in `external_imports` and the corpus was told that a file inside it
/// came from outside it.
#[test]
fn a_relative_import_in_a_template_script_is_not_classified_as_external() {
    const IMPORTING_SVELTE: &str = r#"<script lang="ts">
  import { tsHelper } from "./helpers";
  export function useHelper(x) {
    return tsHelper(x);
  }
</script>
<p>x</p>
"#;
    let result = resolve(&[
        ("src/Importer.svelte", IMPORTING_SVELTE),
        ("src/helpers.ts", HELPERS_TS),
    ]);

    let imports: Vec<(&str, &str)> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Imports)
        .map(|edge| (edge.source_file.as_str(), edge.target_file.as_str()))
        .collect();
    assert!(
        imports.contains(&("src/Importer.svelte", "src/helpers.ts")),
        "a relative import from a Svelte script produced no import edge to the \
         file it names: {imports:?}"
    );

    let external = result
        .unresolved
        .iter()
        .filter(|reference| reference.callee_name == "tsHelper")
        .count();
    assert_eq!(
        external, 0,
        "the imported call stayed unresolved even though its module is indexed"
    );
}

/// `import("./CloneModal.svelte")` and extensionless `./CloneModal` both name
/// the component file. Until `.svelte` was on the JS candidate list, the
/// second form resolved to nothing.
#[test]
fn a_relative_import_resolves_a_svelte_module() {
    const APP: &str = r#"<script lang="ts">
  import Modal from "./CloneModal";
  const load = () => import("./CloneModal.svelte");
</script>
<p>x</p>
"#;
    const MODAL: &str = "<script>export let open = false;</script>\n";
    let result = resolve(&[("src/App.svelte", APP), ("src/CloneModal.svelte", MODAL)]);
    let imports: Vec<(&str, &str)> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Imports)
        .map(|edge| (edge.source_file.as_str(), edge.target_file.as_str()))
        .collect();
    assert!(
        imports.contains(&("src/App.svelte", "src/CloneModal.svelte")),
        "a Svelte module specifier must resolve to the .svelte file: {imports:?}"
    );
}

/// Every language whose script lives inside another language shares that
/// script's family.
///
/// Derived from `LanguageSpec::embedded` rather than from a list written here,
/// which is the whole point: the four hosts that exist today were all `Generic`
/// because nothing forced the question when embedded extraction landed. A fifth
/// template language will fail this the moment it is registered.
///
/// `css` and `html` are excluded because no grammar for either is linked, so
/// they contribute no symbols and no calls — that exclusion is derived too, via
/// `LangFamily::from_lang` returning `Generic` for them.
#[test]
fn an_embedded_script_host_shares_its_script_family() {
    let mut checked = 0;
    for spec in LANGUAGE_SPECS {
        if spec.embedded.is_empty() {
            continue;
        }
        let script_families: BTreeSet<LangFamily> = spec
            .embedded
            .iter()
            .map(|lang| LangFamily::from_lang(lang))
            .filter(|family| *family != LangFamily::Generic)
            .collect();
        if script_families.is_empty() {
            continue;
        }
        assert_eq!(
            script_families.len(),
            1,
            "{} embeds scripts from more than one resolution family ({:?}); which \
             one a call belongs to is a decision, not something this test may guess",
            spec.grammar,
            script_families
        );
        let script_family = *script_families.iter().next().expect("one family");
        assert_eq!(
            LangFamily::from_lang(spec.grammar),
            script_family,
            "{} embeds {:?} scripts but resolves as {:?}; a call extracted from \
             its <script> block is a {script_family:?} call and must resolve in \
             that family — as `Generic` it both fabricated cross-language edges \
             and lost real ones",
            spec.grammar,
            spec.embedded,
            LangFamily::from_lang(spec.grammar),
        );
        checked += 1;
    }
    assert!(
        checked >= 4,
        "only {checked} embedded-script hosts were checked; the registry lists \
         svelte, vue, astro and liquid, so this test has stopped covering them"
    );
}

/// `Generic` is inert: it never admits a cross-file resolution, in either
/// direction.
///
/// The backstop behind the two gates above. Both of those state a property over
/// a *list* — the languages that extract calls, the languages that host
/// scripts — and both lists have gone stale at least once. This one is stated
/// over the bucket itself, so a language that lands in `Generic` by mistake
/// produces a missing edge and an honest `unresolved` row rather than a
/// confident wrong one.
#[test]
fn the_generic_bucket_never_admits_a_resolution() {
    assert!(
        !LangFamily::Generic.admits(LangFamily::Generic),
        "two languages sharing the catch-all bucket must not resolve into each \
         other; that is exactly how a Svelte call reached a Solidity method"
    );

    // Every real family still admits itself, or the guard has simply turned
    // resolution off.
    for family in [
        LangFamily::Python,
        LangFamily::JsTs,
        LangFamily::Go,
        LangFamily::Rust,
        LangFamily::CStyle,
        LangFamily::Swift,
        LangFamily::Kotlin,
        LangFamily::Ruby,
        LangFamily::Php,
        LangFamily::Scala,
        LangFamily::Lua,
        LangFamily::R,
        LangFamily::Dart,
        LangFamily::Erlang,
        LangFamily::Nix,
        LangFamily::Pascal,
        LangFamily::Shell,
        LangFamily::Solidity,
        LangFamily::Sql,
    ] {
        assert!(
            family.admits(family),
            "{family:?} stopped resolving against itself"
        );
        assert!(
            !family.admits(LangFamily::Generic) && !LangFamily::Generic.admits(family),
            "{family:?} and the catch-all bucket must not resolve into each other"
        );
    }
}
