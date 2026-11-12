<script module lang="ts">
  // The same names as Scopes.svelte, each bound by a template construct and then used
  // once after the construct's block has ended, where the name is the function again.
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
</script>

{#each [1, 2] as row}<b>{row}</b>{/each}
{row}

{#each [{ a: 1, b: 2 }] as { a: first, b: second }, index}<i>{first}{second}{index}</i>{/each}
{first}{second}{index}

{#snippet card(cell)}<u>{cell}</u>{/snippet}
{@render card(1)}
{cell}

{#if true}{@const shown = 1 + 1}<s>{shown}</s>{/if}
{shown}

{#await Promise.resolve(1) then loaded}<p>{loaded}</p>{/await}
{loaded}

{#await Promise.resolve(2)}<p>waiting</p>{:catch failed}<p>{failed}</p>{/await}
{failed}

<svelte:fragment slot="body" let:slotted let:thing={renamed}>{slotted}{renamed}</svelte:fragment>
{slotted}{renamed}
