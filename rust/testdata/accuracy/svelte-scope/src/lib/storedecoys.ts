import { writable } from "svelte/store";

// Real stores. StoreDecoys.svelte imports both and reads only `$volume`; `muted` is
// written after a `$` there without once being read, and StoreSibling.svelte is where
// `$muted` is a read. An import is a real store target, which a same-file function
// is not, so only the syntax tree can keep those spellings from reaching it.
export const muted = writable(false);
export const volume = writable(0);
