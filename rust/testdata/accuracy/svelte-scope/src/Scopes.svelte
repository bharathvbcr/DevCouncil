<script module lang="ts">
  // Every name here is also bound by a template construct below, and every use of that
  // name sits inside the construct's own block. The block's binding is the name, not
  // this function: no edge of any tier may reach these.
  export function row(): void {}
  export function first(): void {}
  export function second(): void {}
  export function index(): void {}
  export function cell(): void {}
  export function shown(): void {}
  export function loaded(): void {}
  export function failed(): void {}
  export function slotted(): void {}
  export function renamed(): void {}
  export function item(): void {}
</script>

{#each [1, 2] as row}<b>{row}</b>{/each}

{#each [{ a: 1, b: 2 }] as { a: first, b: second }, index (index)}<i>{first}{second}{index}</i>{/each}

{#snippet card(cell)}<u>{cell}</u>{/snippet}
{@render card(1)}

{#if true}{@const shown = 1 + 1}<s>{shown}</s>{/if}

{#await Promise.resolve(1) then loaded}<p>{loaded}</p>{/await}

{#await Promise.resolve(2)}<p>waiting</p>{:catch failed}<p>{failed}</p>{/await}

<svelte:fragment slot="body" let:slotted let:thing={renamed}>{slotted}{renamed}</svelte:fragment>

{#each [{ Comp: 1 }] as item}<item.Comp />{/each}
