<script module lang="ts">
  // Each function is called only from the default value of a binding pattern in the
  // template: `{ a = fromEach() }` runs `fromEach` when `a` is missing. The default is
  // read where the pattern is written, so these are uses of the module functions.
  export function fromEach(): number {
    return 1;
  }
  export function fromConst(): number {
    return 2;
  }
  export function fromThen(): number {
    return 3;
  }
  export function fromLet(): number {
    return 4;
  }
</script>

{#each [{ a: 1 }] as { a = fromEach() }}<b>{a}</b>{/each}

{#if true}{@const { c = fromConst() } = { c: 2 }}<i>{c}</i>{/if}

{#await Promise.resolve({ d: 3 }) then { d = fromThen() }}<p>{d}</p>{/await}

<svelte:fragment slot="body" let:x={{ e = fromLet() }}>{e}</svelte:fragment>
