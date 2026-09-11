<script>
  let count = $state(0);
  let doubled = $derived(count * 2);
  function bump() {
    count += 1;
  }
</script>
<button onclick={bump}>{doubled}</button>
