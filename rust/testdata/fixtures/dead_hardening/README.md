# Dead-code hardening fixtures

Corpus shapes that reproduced false AmbiguousGlobal fan-out and false-dead
rows before the 2026-09 hardening pass. Integration tests under
`devmap-resolve` / `devmap-analyze` / `devmap-extract` own the assertions;
these files are the readable source forms of those shapes.

| Path | Shape |
|------|--------|
| `rust_external_new/` | `String::new` / many local `new` methods — must not AmbiguousGlobal |
| `rust_typed_receiver/` | param-typed and field-typed receivers |
| `python_except/` | `except PatchError` / `isinstance` / annotations |
| `go_field_receiver/` | Go struct field of typed enum, method call via field |
| `powershell_cap/` | `.ps1` beside Rust — must not corpus-cap Rust findings |
| `svelte_runes/` | `$state` / `$derived` host globals |
| `ruleguard/` | `dsl.Matcher` rule as framework entry |
