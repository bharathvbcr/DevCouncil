import { writable } from "svelte/store";

// Read in the component only as `$previewMarkers`: the import is its sole use.
export const previewMarkers = writable<string[]>([]);
