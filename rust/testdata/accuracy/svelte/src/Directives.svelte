<script lang="ts">
  import { fade } from "svelte/transition";
  import { focusTrap, grow, shrink, reorder, slide } from "./lib/actions";
</script>

<div use:focusTrap transition:fade|local in:grow out:shrink animate:reorder>directives</div>
<p transition:slide>slides</p>
