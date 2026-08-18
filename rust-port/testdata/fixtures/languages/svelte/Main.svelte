<script lang="ts">
  export let name: string;
  function greet(): string { return `Hello ${name}`; }
</script>
<button on:click={greet}>{name}</button>
