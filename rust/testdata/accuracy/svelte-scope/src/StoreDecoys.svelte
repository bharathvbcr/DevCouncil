<script lang="ts">
  import { muted, volume } from "./lib/storedecoys";

  // `muted` is an import, so a store read of it is a real edge. Each spelling below
  // writes `$muted` where it is not a read: a regular expression, a string, the text
  // part of a template literal, an object key, a label, a comment. The one read is
  // `$volume`.
  // $muted in a comment
  $muted: for (;;) {
    break $muted;
  }
  void /^\$muted|\$muted$/;
  void "$muted";
  void `cost $muted and ${1}`;
  void { $muted: 1 };
  void ($volume + 1);
</script>

<p title={"$muted"}>{`$muted`}{/\$muted/ instanceof RegExp}</p>
