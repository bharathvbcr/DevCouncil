<template>
  <div>{{ value }}</div>
</template>

<script lang="ts">
import { helper } from "./helper";

export function render(name: string): string {
  return helper(name);
}

const value = render("x");
</script>
