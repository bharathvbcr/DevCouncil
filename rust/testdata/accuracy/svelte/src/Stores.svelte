<script module lang="ts">
  // Every name here is also written after a `$` somewhere below, where it is not a store
  // read: a regular expression, an object key, a string, a comment, a property, a
  // destructuring key, a label, a rune. No edge of any tier may reach these.
  export function foo(): void {}
  export function bar(): void {}
  export function item(): void {}
  export function zap(): void {}
  export function state(): void {}
  export function derived(): void {}
  export function effect(): void {}
  export function props(): void {}
</script>

<script lang="ts">
  const pattern = /^\$foo|\$bar$/;
  const obj = { $item: 1, $zap: 2 };
  const text = "$foo";
  const template = `cost $foo and ${text}`;
  // $foo in a comment
  const member = obj.$foo;
  const { $foo: renamed } = obj;
  $bar: for (;;) {
    break $bar;
  }
  let count = $state(0);
  let double = $derived(count * 2);
  $effect(() => {
    count = double;
  });
  const { size } = $props();
</script>

<p title={"$foo"}>{`$zap`}{obj.$foo}{/\$bar/.test(text)}{pattern}{template}{member}{renamed}{size}</p>
