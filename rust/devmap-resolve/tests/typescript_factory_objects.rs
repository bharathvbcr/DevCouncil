//! End-to-end resolution tests for JS/TS factory-returned objects (Svelte store
//! singletons, local factory bindings, direct chained factory calls, aliases,
//! re-exports, namespaces, composed local factory spreads, and Svelte `{@attach}`
//! bindings) plus fail-closed soundness tests.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> devmap_resolve::ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolve_all")
}

fn call_targets(files: &[(&str, &str)], source_file: &str) -> Vec<String> {
    resolve(files)
        .edges
        .into_iter()
        .filter(|e| e.source_file == source_file && e.edge_kind == EdgeKind::Calls)
        .map(|e| e.target_symbol)
        .collect()
}

fn ref_targets(files: &[(&str, &str)], source_file: &str) -> Vec<String> {
    resolve(files)
        .edges
        .into_iter()
        .filter(|e| e.source_file == source_file && e.edge_kind == EdgeKind::References)
        .map(|e| e.target_symbol)
        .collect()
}

#[test]
fn exported_factory_singleton_resolves_arrow_and_shorthand_methods_from_ts_and_svelte() {
    let store = "\
function createInterfaceStore() {
  return {
    applyPreset: (id: string) => id,
    resetZoom() {},
  };
}
export const interfaceStore = createInterfaceStore();
";
    let ts_caller = "\
import { interfaceStore } from './interfaceStore';
export function apply() {
  interfaceStore.applyPreset('nord');
}
";
    let svelte_caller = "\
<script lang=\"ts\">
  import { interfaceStore } from './interfaceStore';
  function onPick() {
    interfaceStore.applyPreset('nord');
  }
</script>
<button onclick={interfaceStore.resetZoom}>Reset</button>
";
    let files = [
        ("src/interfaceStore.ts", store),
        ("src/caller.ts", ts_caller),
        ("src/Gallery.svelte", svelte_caller),
    ];

    let ts_calls = call_targets(&files, "src/caller.ts");
    assert!(
        ts_calls
            .iter()
            .any(|t| t == "src/interfaceStore.ts::createInterfaceStore.applyPreset"),
        "expected TS call to applyPreset, got {ts_calls:?}"
    );

    let svelte_calls = call_targets(&files, "src/Gallery.svelte");
    assert!(
        svelte_calls
            .iter()
            .any(|t| t == "src/interfaceStore.ts::createInterfaceStore.applyPreset"),
        "expected Svelte call to applyPreset, got {svelte_calls:?}"
    );

    let svelte_refs = ref_targets(&files, "src/Gallery.svelte");
    assert!(
        svelte_refs
            .iter()
            .any(|t| t == "src/interfaceStore.ts::createInterfaceStore.resetZoom"),
        "expected Svelte member reference to resetZoom, got {svelte_refs:?}"
    );
}

#[test]
fn shorthand_forwarded_inner_functions_resolve_while_factory_locals_abstain() {
    let toast = "\
function createToastStore() {
  const { subscribe } = writable([]);
  function add(msg: string) { return msg; }
  return {
    subscribe,
    add,
    error: (msg: string) => add(msg),
  };
}
export const toastStore = createToastStore();
";
    let unrelated = "export function subscribe() {}\n";
    let caller = "\
import { toastStore } from './toastStore';
export function notify() {
  toastStore.add('hi');
  toastStore.error('oops');
  toastStore.subscribe(() => {});
}
";
    let calls = call_targets(
        &[
            ("src/toastStore.ts", toast),
            ("src/unrelated.ts", unrelated),
            ("src/caller.ts", caller),
        ],
        "src/caller.ts",
    );
    assert!(
        calls
            .iter()
            .any(|t| t == "src/toastStore.ts::createToastStore.add"),
        "got {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|t| t == "src/toastStore.ts::createToastStore.error"),
        "got {calls:?}"
    );
    assert!(
        !calls.iter().any(|t| t.ends_with("subscribe")),
        "factory-local destructured subscribe must abstain and never bind unrelated subscribe: {calls:?}"
    );
}

#[test]
fn local_factory_binding_and_chained_call_resolve_without_confusing_function_properties() {
    let factory = "\
export function createPanelTabCounts() {
  return {
    report(path: string, n: number) {},
    forget(path: string) {},
  };
}
export const panelTabCounts = createPanelTabCounts();
";
    let caller = "\
import { createPanelTabCounts, panelTabCounts } from './panelTabCounts';
export function useLocal() {
  const counts = createPanelTabCounts();
  counts.report('/repo', 2);
}
export function useChained() {
  createPanelTabCounts().forget('/repo');
}
export function useSingleton() {
  panelTabCounts.report('/repo', 1);
}
";
    let bad_caller = "\
import { createPanelTabCounts } from './panelTabCounts';
export function useDottedLocal() {
  const counts = createPanelTabCounts();
  counts.nested.report('/repo', 1);
}
";
    let files = [
        ("src/panelTabCounts.ts", factory),
        ("src/caller.ts", caller),
        ("src/bad.ts", bad_caller),
    ];

    let good = call_targets(&files, "src/caller.ts");
    assert!(
        good.iter()
            .any(|t| t == "src/panelTabCounts.ts::createPanelTabCounts.report"),
        "got {good:?}"
    );
    assert!(
        good.iter()
            .any(|t| t == "src/panelTabCounts.ts::createPanelTabCounts.forget"),
        "got {good:?}"
    );

    let bad = call_targets(&files, "src/bad.ts");
    assert!(
        !bad.iter().any(|t| t.contains("createPanelTabCounts.report")),
        "property access on function object or dotted local must not falsely bind factory method: {bad:?}"
    );
}

#[test]
fn aliases_reexports_default_exports_namespaces_and_concise_arrows_resolve() {
    let factory = "\
export const mkStore = () => ({
  ping: () => 1,
});
export const store = mkStore();
export default store;
";
    let barrel = "export { store } from './store';\n";
    let caller = "\
import defStore from './store';
import * as mod from './store';
import { store as reStore } from './barrel';
const alias = reStore;
export function run() {
  defStore.ping();
  mod.store.ping();
  reStore.ping();
  alias.ping();
}
";
    let res = resolve(&[
        ("src/store.ts", factory),
        ("src/barrel.ts", barrel),
        ("src/caller.ts", caller),
    ]);
    let ping_calls = res
        .edges
        .iter()
        .filter(|e| {
            e.source_file == "src/caller.ts"
                && e.edge_kind == EdgeKind::Calls
                && e.target_symbol == "src/store.ts::mkStore.ping"
        })
        .count();
    assert_eq!(
        ping_calls, 4,
        "expected all 4 call forms to resolve, edges={:?}",
        res.edges
    );
}

#[test]
fn composed_factory_via_local_factory_spread_resolves_both_spread_and_prior_methods() {
    let tab_actions = "\
export function createTabActions() {
  function openRepo(p: string) {}
  return {
    openRepo,
    activateTab: (id: string) => id,
  };
}
";
    let repo_store = "\
import { createTabActions } from './tabActions';
export function createRepoStore() {
  const tabActions = createTabActions();
  const store = {
    trustRepo: (p: string) => p,
    ...tabActions,
  };
  return store;
}
export const repoStore = createRepoStore();
";
    let caller = "\
import { repoStore } from './repoStore';
export function run() {
  repoStore.openRepo('/a');
  repoStore.activateTab('1');
  repoStore.trustRepo('/a');
}
";
    let calls = call_targets(
        &[
            ("src/tabActions.ts", tab_actions),
            ("src/repoStore.ts", repo_store),
            ("src/caller.ts", caller),
        ],
        "src/caller.ts",
    );
    assert!(
        calls
            .iter()
            .any(|t| t == "src/tabActions.ts::createTabActions.openRepo"),
        "got {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|t| t == "src/tabActions.ts::createTabActions.activateTab"),
        "got {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|t| t == "src/repoStore.ts::createRepoStore.trustRepo"),
        "trustRepo before unmodified local factory spread must resolve: {calls:?}"
    );
}

#[test]
fn shadowing_multi_return_tainted_spread_and_unreadable_spread_abstain() {
    let multi = "\
export function makeDynamic(flag: boolean) {
  if (flag) return { a() {} };
  return { b() {} };
}
export const dynStore = makeDynamic(true);
";
    let tainted = "\
import { makeDynamic } from './multi';
function makeClean() { return { clean() {} }; }
export function createWithDynamicSpread() {
  const d = makeDynamic(true);
  return { before() {}, ...d };
}
export function createWithMutatedLocal() {
  const store = { run() {} };
  patch(store);
  return store;
}
export const dynSpreadStore = createWithDynamicSpread();
export const mutatedStore = createWithMutatedLocal();
export const cleanStore = makeClean();
";
    let caller = "\
import { dynStore } from './multi';
import { dynSpreadStore, mutatedStore, cleanStore } from './tainted';
export function shadowed(cleanStore: any) {
  cleanStore.clean();
}
export function checkAbstain() {
  dynStore.a();
  dynSpreadStore.before();
  mutatedStore.run();
}
";
    let calls = call_targets(
        &[
            ("src/multi.ts", multi),
            ("src/tainted.ts", tainted),
            ("src/caller.ts", caller),
        ],
        "src/caller.ts",
    );
    assert!(
        calls.is_empty(),
        "shadowed/multi-return/tainted/dynamic-spread calls must abstain, got {calls:?}"
    );
}

#[test]
fn svelte5_attach_expressions_resolve_to_script_functions() {
    let svelte = "\
<script lang=\"ts\">
  function trackPill(node: HTMLElement) { return () => {}; }
  function watchActivity(key: string) { return (node: HTMLElement) => {}; }
  let activityKey = 'abc';
</script>
<div {@attach trackPill} {@attach watchActivity(activityKey)}>ok</div>
";
    let files = [("src/Pill.svelte", svelte)];
    let refs = ref_targets(&files, "src/Pill.svelte");
    let calls = call_targets(&files, "src/Pill.svelte");
    assert!(
        refs.iter().any(|t| t == "src/Pill.svelte::trackPill"),
        "expected References edge to trackPill, got {refs:?}"
    );
    assert!(
        calls.iter().any(|t| t == "src/Pill.svelte::watchActivity"),
        "expected Calls edge to watchActivity, got {calls:?}"
    );
}
