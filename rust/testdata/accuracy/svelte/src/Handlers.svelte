<script lang="ts">
  import { savePost, archivePost } from "./lib/posts";

  function reset(): void {}
</script>

<!-- Each function below is handed to an event, not called: a lone identifier in markup is a
     value read, the way an identifier in JSX attribute position is. -->
<button onclick={savePost}>save</button>
<button on:click={archivePost}>archive</button>
<button onclick={reset}>reset</button>
