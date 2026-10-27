# extract-v98: the extractor-output bundle

Status: DESIGN ONLY. No code, no build. Base: main `f796dd8c`. `EXTRACTION_SCHEMA_VERSION` stays
`"97"` (`rust/devmap-extract/src/cache.rs:798`) until the consumer sweep (ft-3018ab76) closes and
DevCouncil main is pushed. This document is written so implementation can start the day it does.

Labels: **[V]** verified by reading code or running a command this session; **[I]** inferred;
**[U]** unverified, to be settled by the first implementation step named beside it.

Inputs read: `docs/devmap/design/jvm-scope-import-model.md` (all ruling sections and "v98 JVM
declaration facts", V1-V14), `BUILD_REPORT.md` ("Waiting on extract-v98"), every `#[ignore]` that
cites extract-v98 or "schema frozen at 97", and `rust/testdata/accuracy/known_failing.tsv`.

Where the evidence lives. The 45 JVM acceptance probes are **not on main**. They are on the
unmerged, paused branch `lane/jvm-scope-model` @ `a247f398` [V: `git grep` on that ref], in
`rust/devmap-resolve/src/resolver/jvm_names_attack.rs` and `jvm_names_attack2.rs`. Every
`file:line` below for those two files, and for `attack_resolver_bundle3/5.rs`,
`kotlin_receivers_and_packages.rs`, `overload_self_edges.rs`, is on that ref. Everything else is on
main. GitPulse card bodies could not be read (`gitpulse_get_task` returned not_found for
ft-80c5d100), so card text is taken from BUILD_REPORT.md and the `#[ignore]` strings.

## 0. Ground rules this design relies on

1. **No new field on `ExtractedSymbol` unless a symbol is genuinely new.** That struct is built by
   literal at about 35 sites in about 9 files (`rg -c 'ExtractedSymbol \{'`: `extract_node.rs` 21,
   `model.rs` 4, plus `markup.rs`, `langdecl/ruby.rs`, `liveness.rs`, and four test files) [V].
   A field there makes every lane touch `extract_node.rs`. Declaration facts go in **one side table
   on `Extraction`**, `jvm_decls`, joined to a symbol by `(qualified_name, span.start_byte)`. This
   is the shape `scope_receivers`, `js_objects` and `swift_type_aliases` already use.
2. **Every new `Extraction` field is `#[serde(default, skip_serializing_if = ...)]`, sorted and
   deduplicated at emission.** The determinism gate digests this payload
   (`model.rs:1657-1662`). Every new walk checks `walk_deadline_passed()` and has a hard cap
   (count and nesting) that is recorded, never silently applied. A capped fact is `None` or flagged,
   never an empty list that reads as "none exist".
3. **One canonical owner per fact.** Declaration identity stays in `langdecl` (`declaration_of`,
   `langdecl/mod.rs:127`), which `langcalls::scope` also asks, so a call's `caller_symbol` and a
   symbol's `qualified_name` cannot drift [V: module doc, `langdecl/mod.rs:43-48`]. JVM gets its own
   `langdecl/java.rs` (Java today falls to `generic`, `mod.rs:140`) beside `kotlin.rs`.
4. **Resolver adoption is not part of this bundle.** v98 is extractor output plus extraction-level
   tests. Un-pausing `lane/jvm-scope-model` and un-ignoring the resolver probes is a serial Phase R
   after v98 merges (section 3).

## 1. Inventory

Columns: **Fact**, **Langs**, **Emit at** (file, function, grammar node), **Model field**,
**Unblocks** (ignored tests and known_failing rows, with `file:line`), **Cards**.
"Probe count" counts ignored tests plus known_failing rows.

### 1.1 JVM declaration facts (V1-V14) and Kotlin loss

New structs (all in `rust/devmap-extract/src/model.rs`, new, no existing field changes):

```text
Extraction.jvm_decls:      Vec<JvmDeclFact>      // V3 V5 V9 V10 V11 V14, keyed (qualified_name, start_byte)
Extraction.package_scopes: Vec<PackageScope>     // V1 (Scala) and V2 (C#): {name, start_byte, end_byte}
Extraction.local_extents:  Vec<LocalExtent>      // V12: {scope, name, from_byte, to_byte}
Extraction.decl_gaps:      Vec<TextRange>        // V13: ranges where declarations may be missing, Kotlin/Java/Scala
Extraction.recovered_decl_ranges: Vec<TextRange> // Kotlin row 15: statements the grammar read as an expression and the extractor repaired
ExtractedImport.kind:      ImportKind            // V4 (serde default = Unspecified)

struct JvmDeclFact {
    qualified_name: String, start_byte: usize,
    kind: JvmDeclKind,                  // Type | Callable | Companion | TypeAlias
    modifiers: u16,                     // bitset: INNER STATIC INTERNAL PROTECTED PACKAGE_PRIVATE PRIVATE ABSTRACT OPEN DATA ...
    type_params: Vec<String>,           // V3, source order, names only
    header: Option<TextRange>,          // V11: declaration start .. body start
    body: Option<TextRange>,            // V11
    alias_target: Option<String>,       // V5, trimmed RHS text, TypeAlias only
    companion_name: Option<String>,     // V10, Companion only; None = the default "Companion"
}
```

| # | Fact | Langs | Emit at | Model field | Unblocks | Cards |
|---|------|-------|---------|-------------|----------|-------|
| 1 | **V1 Scala package scopes**: `package a.b`, chained `package a` / `package b` => `a.b`, block forms | Scala | New `jvmfacts.rs::scala_scopes`, called from `entry.rs:925` region. Today `jvm_package_name` (`treesitter/rust_use.rs:896`) accepts only `package_header`/`package_declaration` and `entry.rs:925` gates on `kotlin\|java`, so Scala gets nothing [V]. Scala nodes `package_clause` (header and block) [U: confirm node names with a tree dump] | `Extraction.package_scopes`; header form also fills `Extraction.jvm_package` | `r4_scala_repo_import_same_name_binds` (`attack_resolver_bundle5.rs:275`), `j8` guard stays on stem allow-list `importpath.rs:54` until it lands; kg Scala stem rows | ft-2c800986 |
| 2 | **V2 C# namespace per type**: `namespace N { }` and file-scoped `namespace N;` | C# | `jvmfacts.rs::csharp_scopes`; nodes `namespace_declaration` (already a `Module` symbol via `generic_symbol_kind`, `lang_helpers.rs:1415` [V]) and `file_scoped_namespace_declaration` (file-scoped extends to EOF) [U] | `Extraction.package_scopes` (same struct as V1: one field for both languages) | `ov_csharp_partial_classes_across_files_sibling_call_not_self` (`attack_resolver_bundle3.rs:299`) | ft-2c800986 |
| 3 | **V3 type-parameter names** on classes, callables, typealiases | Kotlin, Java | `jvmfacts.rs`; Kotlin `type_parameters` child of `class_declaration`/`function_declaration`/`type_alias`; Java `type_parameters` of `class_declaration`/`method_declaration`/`interface_declaration`/`constructor_declaration` [U] | `JvmDeclFact.type_params` | 15 probes in `jvm_names_attack.rs`: `a02`:80, `a02b`:89, `a02c`:99; `jvm_names_attack2.rs`: `t01`:66, `t01j`:80, `t05`:108, `t06`:119, `t07`:130, `t08`:141, `t08b`:154, `t09`:167, `t10`:179, `t11`:192, `t12`:202, `d02`:910. Plus `kg.Z5` (`known_failing.tsv:13`, `kotlin_written_types.rs`) | ft-2c800986, ft-87b28bb6 |
| 4 | **V4 import kind flags** `{static, wildcard, member}` so the resolver stops parsing `raw_import` | Kotlin, Java, Scala | `langimports/` per-language `extract_import` (`langimports/mod.rs:109` registry, java/kotlin modules) | `ExtractedImport.kind` (new field, 15 literal sites [V: rg -c]; use `Default` and `..` at sites) | None (hardening). Replaces the source-text Java `import static` parser added at commit 2 | ft-2c800986 |
| 5 | **V5 Kotlin typealias target** | Kotlin | `langdecl/kotlin.rs`: add `type_alias` arm. Verified none exists today: no symbol, only `Name` refs [V: Phase 0 note, BUILD_REPORT.md "Q2"] | `JvmDeclFact{kind: TypeAlias, alias_target}`. Not a symbol (no id churn) | `j10_kotlin_import_of_package_declared_only_by_other_file_binds_stem` (`attack_resolver_bundle5.rs:155`); `t08` (`jvm_names_attack2.rs:141`) shares V3 | ft-2c800986 |
| 6 | **V6 Java declared types** for formal parameters and call-initialised locals | Java | **Corrected (the first draft named the wrong seam).** The seam is `langfields.rs`, not `param_type_bindings` (`rust_use.rs:609` has arms for `rust`, `go` and C-family only [V], and widening its caller guard `extract_node.rs:1610` would add a second Java type-binding path beside the one `langfields` owns). `langfields.rs:89` is where a Kotlin parameter or typed local takes its enclosing callable (`None if lang == "kotlin"`), and `field_node_kinds` (`langfields.rs:180`) is the per-language node-kind registry (`"java" => &["field_declaration"]` today [V: read]). Java takes `formal_parameter` there and the same enclosing-callable fallback. **Refuse two shapes** [U: from the lane brief, not re-run]: `var` (a Java `var x = make();` local already emits a `Type` reference whose name is the keyword `var`, which would bind a declared type named `var`) and `spread_parameter` (`Type... xs` is an array, not `Type`). **Deferred in the Kotlin-loss lane**: `langfields.rs` is owned by another session's Swift lane, so row 6 is not touched until that lane merges. Call-initialised locals (`Repo r = make();`): the Java local declarator needs the same `Type` reference with `assigned_to` [U: confirm where `new`-initialised Java locals get theirs; `langcalls/java.rs` `record`] | No new field. Emits `ReferenceKind::Type` with `assigned_to = param`, which `binding_declared_types` (defined `scope_locals.rs:715`, called `:440` [V]) already turns into `LocalBinding.declared_type` | `r3` (`attack_resolver_bundle5.rs:265`), `j6` (:116), `e7` (:387), `java_typed_parameter_receiver_binds` (`overload_self_edges.rs:110`) | ft-2c800986, ft-eda9b3a6 |
| 7 | **V7 property and field symbols**: Kotlin properties, Java fields, enum entries, companion constants | Kotlin, Java | Kotlin `langdecl/kotlin.rs:65`: `property_declaration` returns a symbol only for an extension property; add the ordinary-property arm (kind `Field`, signature tag). Java: new `langdecl/java.rs` for `field_declaration` (one symbol per `variable_declarator`) and `enum_constant`; `generic_symbol_kind` (`lang_helpers.rs:1406`) has neither [V]. `is_symbol_binding` (`scope_locals.rs:125`) must treat the new names as symbols, as it does for Swift (`:148`) | New `ExtractedSymbol` rows (`SymbolKind::Field`), signature tags `kotlin property`/`java field`/`enum entry`. No new struct field | 9 probes: `c02b` (`jvm_names_attack2.rs:363`), `c04`:383, `c05`:397, `c09`:460, `o05`:552, `i02`:738, `m03`:885, `c14`:933, `c15`:948; row `kg.V7` (`known_failing.tsv:26`); `v8_a_java_anonymous_class_method_is_not_a_member_of_its_host` (`attack_resolver_bundle5.rs:641`) shares V8. **11** | ft-2c800986 |
| 8 | **V8 Java `record`, `@interface`, anonymous class symbols** | Java | `langdecl/java.rs`: `record_declaration`, `annotation_type_declaration`, `object_creation_expression` with a `class_body` [U]. Neither of the first two is in `generic_symbol_kind` [V] | New symbols: record => `Class`, signature `record`; `@interface` => `Interface`, signature `@interface`; anonymous class gets an owner-scoped synthetic name (open question Q4) | `v8_a_java_record_method_is_not_a_member_of_its_host` (`attack_resolver_bundle5.rs:627`), anon at :641 | ft-2c800986 |
| 9 | **V9 `inner` / `static` modifiers** on nested types | Kotlin, Java | `jvmfacts.rs`; Kotlin `class_modifier` `inner` (`langdecl/kotlin.rs:198 has_modifier`), Java `modifiers` containing `static` | `JvmDeclFact.modifiers` | `a01` (`jvm_names_attack.rs:61`), `a01b`:70 | ft-2c800986 |
| 10 | **V10 companion objects** with their own span, named or not | Kotlin | `jvmfacts.rs`; node `companion_object`. `langdecl/kotlin.rs:44-48` skips it on purpose so members stay `C.f`. **Keep that**: no symbol, no id churn | `JvmDeclFact{kind: Companion, companion_name, body}`, parent `qualified_name` = host | `c06` (`jvm_names_attack2.rs:411`), `c06b`:425, `i03b`:760 | ft-2c800986 |
| 11 | **V11 header and body spans** (supertype list, primary ctor, annotation args, `where`, type-parameter bounds, delegation) | Kotlin, Java | `jvmfacts.rs`; Kotlin `class_declaration` fields (name, `type_parameters`, `primary_constructor`, `delegation_specifiers`, `type_constraints`, `class_body`) [U]; Java `class_declaration` `superclass`/`super_interfaces`/`type_parameters`/`body` | `JvmDeclFact.header`, `.body` | 9 probes: `d01` (`jvm_names_attack2.rs:899`), `h01`:214, `h03`:255, `h03b`:266, `h03c`:958, `h05`:286, `h07`:307, `h08`:318, `h08b`:969 | ft-2c800986 |
| 12 | **V12 block and lambda extents of local bindings** | Kotlin, Java | `treesitter/scope_locals.rs` (`collect_scope_locals`, called `entry.rs:947`): record the extent of the enclosing block or lambda for each local | `Extraction.local_extents` (`scope_locals` stays as is: `Vec<(String,String)>`, `model.rs:1663`) | `a17e` (`jvm_names_attack.rs:456`) | ft-2c800986 |
| 13 | **V13 parse-drop flag**: which declarations the Kotlin grammar lost after an error; some one-line class bodies parse Partial | Kotlin (Java, Scala same shape) | `entry.rs:1076 parse_outcome_of` already yields `Partial{error_ranges, nameless_ranges}`. `decl_gaps` records, per lost region, the range the grammar could not turn into a declaration. **Corrected: "re-parse the tail after an error range at the next line start" cannot work for the shape that matters.** When the error is a whole-file ERROR (row 16: the ERROR swallows `class C { ... }` and every top-level declaration after it), there is no later line start that lies outside the ERROR, so there is no tail to re-parse. [I] Recovery for an ERROR region is a structured walk of the ERROR node's children (a child that is a real `function_declaration`/`class_declaration` is read by the ordinary `declaration_of`), or a same-length `;` rewrite of the offending bytes followed by a full re-parse (offsets are preserved, so nothing is shifted). Row 16's same-length `;` rewrite is built (`kotlinsemi.rs`, see row 16); the structured walk is not. **Ruling (extract-v98 Kotlin loss lane): removal of what the outer tree emitted applies to detector-charged regions only (row 15), never to a whole-file ERROR range.** A whole-file ERROR range keeps the output the grammar's own error recovery gave it, because that recovery puts correct top-level declarations inside the ERROR beside the phantoms and nothing at the tree level tells them apart; dropping the range would lose true symbols. An ERROR range is already `Partial`, so it is loud. This row does not reopen the ruling: a recovery for an ERROR region may add what it proves (as row 16's repair does) and must not delete what the grammar produced | `Extraction.decl_gaps` | `a21b` (`jvm_names_attack.rs:526`), `p01` (`jvm_names_attack2.rs:825`), `p02`:837 | ft-2c800986, ft-5ad86836 |
| 14 | **V14 visibility modifiers** (`internal`, package-private, `protected`) | Kotlin, Java | `jvmfacts.rs`; Kotlin `visibility_modifier` (`langdecl/kotlin.rs:190`); Java `modifiers` (absence of public/protected/private = package-private) | `JvmDeclFact.modifiers`. `is_exported` stays as is: it is a dead-code claim and measured wrong when widened (`langdecl/kotlin.rs:158-184`) | `v03` (`jvm_names_attack2.rs:610`), `v04`:622, `v05`:635 | ft-2c800986 |
| 15 | **Kotlin silent declaration drop (widened from "stacked annotations")**. [V: tree dumps from the locked grammar, tree-sitter-kotlin-ng 1.1.0] At the top level of a Kotlin file, two or more annotations followed by exactly one modifier-like word and then a declaration that also reads as a valid expression parse **Clean**: the tree is a nested `annotated_expression` chain (one level per annotation) ending in an `infix_expression` whose children are `[identifier(modifier), identifier "fun", call_expression(call_expression(identifier NAME, value_arguments), annotated_lambda(lambda_literal = BODY))]`. `private fun f() = 1` is a third wrapper: `(assignment left: (annotated_expression ... (infix_expression ...)) right: ...)`. The loss is **not specific to `fun`**: behind the same two-annotation prefix, `private class Q {}`, `private object O {}`, `private interface I {}`, `internal class Q`, `private val y = 1`, `private var y = 1`, `private typealias T = Int`, `open class O`, `sealed interface S`, `enum class E { X }`, `abstract class AC {}`, `const val c = 1` and the modifier-less `class Plain {}` all parse Clean (`val`, `var`, `typealias` and `fun ... = expr` as an `assignment` wrapper, the rest as `infix_expression`) and emit no symbol [V: dumped this session; without the annotations `private class Q {}`, `private fun f() = 1` and `private val y = foo(1)` parse as ordinary declarations]. It is not specific to "after a property" either: the first statement of a file and the statement after a function lose the same way, and a stack nested in a function body parses the same wrong shape. Class bodies are immune. Survivors [V]: `data class D(val a: Int)` and `private suspend fun s() {}` (two modifiers) behind two annotations, `fun f(a: Int) {}`, `fun f(): Unit {}`, `fun f(): Int = 1`, generics, block bodies with parameters, and a single annotation. Example: `private val X = 1\n@Preview\n@Composable\nprivate fun f() {}\nfun after() {}\n` emits `File` and `after` only. "Clean" means no `Partial` flag, so nothing downstream knows. | `langdecl/kotlin.rs::declaration` (`:27`) never sees a `function_declaration` (or class, object, property, alias node). **Resolved**: BUILD_REPORT's `infix_expression` and the ignore strings' `annotated_expression` are both right: the chain is `annotated_expression` outermost with the `infix_expression` inside. **Detector (the tree check, not a text scan)**: in a Clean Kotlin tree, an `identifier` node whose exact text is a hard keyword (`fun`, `class`, `interface`, `object`, `val`, `var`, `typealias`) cannot occur in valid Kotlin. Strings and comments are not `identifier` nodes, and a backtick-escaped `` `fun` `` is an `identifier` whose text includes the backticks, so it does not match. **One legitimate exception, found by running the first draft**: `class` after `::` (`String::class`, `this::class`) is an `identifier` in a `navigation_expression`, so an identifier whose previous sibling is `::` is skipped; without that rule every class reference in the corpus would have been flagged. A statement at the end of a file with nothing after it parses as a declaration; the loss needs a following declaration [V: dumped], so the test inputs end with one. `treesitter/entry.rs::parse_outcome_of` (the one owner of the outcome) downgrades the file to `ParseOutcome::Partial` and adds the enclosing top-level statement (the outermost `annotated_expression`/`assignment` at `source_file` level) to `error_ranges`; the walk is deadline-checked and node-capped, and a capped walk is recorded (a capped check is never Clean by default). This is what makes the failure loud instead of silent, and it covers every affected declaration kind. **Recovery** (second commit, after the detector): re-parse the flagged statement's text wrapped as `class Z {\n` + text + `\n}` and subtract the prefix length from offsets [V: parsed to the right `function_declaration`/`class_declaration`/`object_declaration`/`property_declaration`/`type_alias` in all 8 shapes tried]. Re-parsing the node's byte range **alone does not work** [V: it still parses as `annotated_expression`], which retires the draft's fallback. **Limits found by running it** [V]: a swallowed class, interface or object whose body is on one line and holds a member (`private class Q { fun m() {} }`) is detected but not recoverable, because the wrapper itself is then a grammar ERROR (see row 16); it stays `Partial` with nothing emitted. A swallowed statement that is the last thing in a file is not swallowed at all. Recovery applies only to a region that is a direct child of the file's root; a swallowed statement inside a function body is charged, not recovered. The wrapper parse must itself be Clean, and free of the detector's keyword, before anything is emitted. Names are qualified as top-level by `langdecl` (the wrapper class is not an owner), so a `caller_symbol` and a `qualified_name` cannot drift; calls inside a recovered body replace the file-scope calls the original tree made. A recovered range is listed in the new `Extraction.recovered_decl_ranges` (repaired: a resolver need not abstain inside). A charged region that is **not** recovered is replaced by nothing (the settling in section 1.4, with an empty result), so the expression reading's symbols, calls, references, exports, wiring, local bindings, scope locals and scope receivers inside it are not published (a top-level `m` for the member of a class is worse than none); it stays in `error_ranges` and is listed in `decl_gaps`, per V13, so a resolver abstains. A charged range nested inside a recovered range is not a separate loss. When nothing is left charged the outcome returns to Clean. The region cap (64 per file) is recorded only in `Extraction.diagnostics`, which the stored payload drops; the durable record is the unrecovered ranges in `error_ranges` and `decl_gaps` | Symbols re-emitted (ids unchanged); `recovered_decl_ranges` (a new `TextRange` list in the same v98 bump) carries the repaired ranges and `decl_gaps` the unrecovered ones; a recovered marker in `jvm_decls` would have needed a field another lane owns; detector uses existing `ParseOutcome::Partial` | `kotlin_stacked_annotations_after_a_property_initializer_keep_the_function` (`rust/devmap-extract/tests/declarations.rs:616`), `..._multiline_initializer_...` :627, `kotlin_annotations_with_arguments_...` :638, `kotlin_stacked_annotations_after_a_blank_line_...` :649. Control `kotlin_annotation_stacks_that_still_parse_as_declarations_keep_the_function` (:663) must stay green | ft-5ad86836 (AC1, HIGH) |
| 16 | **One-line companion object**. `class C { companion object { fun f() = 1 } }` is an ERROR node and drops `C`, `C.f` and the following top-level funs | Kotlin | **Corrected.** The ERROR is whole-file, so "the generic tail re-parse" has no tail to start from (row 13). [I] Recovery is a structured walk of the ERROR node's children, or a same-length `;` rewrite of the offending bytes and a re-parse that keeps every offset; a grammar bump is a new dependency, ask first. **Recovery: the same-length rewrite (`kotlinsemi.rs`)** [V: built and measured]. For a Kotlin tree with a *visible* ERROR or MISSING node, a `;` is written over one existing space or tab directly before each `}` that follows code on its line and is a child of a `class_body`, `enum_class_body` or ERROR (strings and comments skipped). The length is unchanged, so every offset is the file's own, and the extractor reads the original text through the rewritten tree. The tokens are not all visible to the first parse (a nested `}` hides inside the ERROR), so it repeats until the re-parse is clean, over the top-level statements that still hold an error, with at most 512 separators per file (every pass that goes on writes at least one, so at most 513 parses), against the file's deadline; a fixed six-pass limit had left seven plain sibling one-liners `Partial`. The result is accepted only if the re-parse is fully clean; anything else leaves the file as it parsed (`Partial`). A `}` with no space before it (`class A { fun f() {}}`) cannot take a `;` without moving offsets; the rewrite does not map offsets back and leaves such a file `Partial`. Repaired statements go to `recovered_decl_ranges`. **Two modes, found while building it** [V: dumped]: the grammar reports the shape either as a visible ERROR (the file loses the class and everything after it) or recovers it itself with a hidden zero-width `_class_member_semi` that no node shows; `root.has_error()` is true in both, and `parse_outcome_of` used to see only the first and read the second `Clean`; that was wrong, because the hidden token can make an `if` branch a lambda (`class A { fun f() = if (c) { a() } else { b() } }` gave phantom `$0` and `it` locals and a wrong `assigned_to`). `parse_outcome_of` now charges the range of a node whose own error cost is on none of its children (the token is hidden, so no ERROR or MISSING node is there to find), the file is `Partial`, and the rewrite repairs it like the first mode. Which mode a shape gets depends on the file (`class A { val x = 1 }` first in a file is an ERROR; `class Box { val x = 1 }` is not), so no shape list predicts it. **Unrecovered ERROR ranges keep today's output**: removing what the outer tree emitted inside a whole-file ERROR would also drop correct declarations the error recovery put inside it, so the row-15 removal applies only to detector regions **Measured while building row 15** [V: dumped, tree-sitter-kotlin-ng 1.1.0]: the one-line loss is not specific to `companion object`. Any one-line class, interface or object body that holds a member is a whole-file ERROR: `class A { val x = 1 }`, `interface Cb { fun call() }` and `val o = object { val x = 1 }` all have `has_error`, while the same declarations spread over several lines parse cleanly, and `object O { fun g() = 2 }` is clean at the start of a file but an ERROR after a `package`/`import` header. The companion case is one instance of that rule, so a structured walk of the ERROR's children or a `;` rewrite recovers all of them at once, and `fun interface Cb { .. }` and `val o = object : Runnable { .. }` are also ERRORs [V], whether by the same cause is [I] | Symbols re-emitted; `JvmDeclFact{kind: Companion}` (row 10) | `kotlin_one_line_companion_object_keeps_the_class_and_later_functions` (`declarations.rs:690`), `kotlin_one_line_named_companion_...` :705. Control `kotlin_one_line_object_keeps_the_object_and_later_functions` (:716) | ft-5ad86836 (AC2) |
| 17 | **Distinct overload ids**. Two `add` in one class share `Overloads.kt::Calc.add`, so the resolver sees a self-edge | Kotlin and Java first; Swift, C#, Scala listed in Q3 | `langdecl/mod.rs::Declaration::qualified` (`:97`). Sibling-aware rule in `declaration_of`: when another callable of the same name has the same owner (scan the parent's children, bounded), append the written parameter types: `Calc.add(Int)`, `Calc.add(String)`; identical text gets `#2` by span order. Un-overloaded ids are **unchanged**. Because `langcalls::scope` calls the same function, `caller_symbol` stays consistent [V: `langdecl/mod.rs:43-48`] | `ExtractedSymbol.qualified_name` for overload groups only; `name` unchanged | `overloads_keep_distinct_identities_and_never_self_edge` (`kotlin_receivers_and_packages.rs:62`), `java_typed_parameter_receiver_binds` (`overload_self_edges.rs:110`, also needs row 6) | ft-eda9b3a6 (AC4) |
| 18 | **Same-name nested types under different outers**: `A.Inner.run` and `B.Inner.run` both become `Inner.run`, since `enclosing_owner` keeps only the direct parent (`langdecl/mod.rs:247`) [V]. `enclosing_owner_path` (`:223`) exists but is applied to enum entries only | Kotlin, Java | `langdecl/mod.rs`: use the full owner chain **only when the nested type's name is declared more than once in the file** (a per-file set computed once, cached like the per-scope cache). Widening unconditionally rewrites every nested-type member id (the SC14 open half, `mod.rs:215-222`) | `qualified_name` for colliding nested types only | `ov_java_two_outer_each_with_inner_same_name_one_file` (`attack_resolver_bundle3.rs:279`, a loss recorded, not an ignore); no ignored test: add one | ft-eda9b3a6 |
| 19 | **Z4 expression-body return shape**: `fun f() = Foo()` has no written return type; `return_type` holds written types only (`known_failing.tsv:12`) | Kotlin | `langdecl/kotlin.rs` + `extract_node.rs:1604` seam: record the initializer shape of an expression body (`Foo{..}` or callee name, the same spelling `initializer_shape` uses, `model.rs:1107`) | Reuse `ExtractedSymbol.return_type` is wrong (it would claim a written type). New `JvmDeclFact.body_shape: Option<String>` | `kg.Z4` (`known_failing.tsv:12`) | ft-87b28bb6 |
| 20 | **Z1, Z2, Z6 recall facts** (stdlib String-method return types, lambda parameter typed by the callee's function-type parameter, `T::class.java`) | Kotlin | **Not specified.** The design doc defines no fact for them. `kg.Z1`, `kg.Z2` are resolver or `treesitter.rs` binding work. Left out of v98 unless a ruling says otherwise (Q6) | none | `kg.Z1` :9, `kg.Z2` :10, `kg.Z6` :14 | ft-87b28bb6 |

### 1.2 Python, TS, tooling

| # | Fact | Langs | Emit at | Model field | Unblocks | Cards |
|---|------|-------|---------|-------------|----------|-------|
| 21 | **getattr outer-call record**. `getattr(o, 'cmd_' + n)()` parses as a `call` whose callee is itself a `call`; no extractor code mentions `getattr` [V: `rg getattr rust/devmap-extract/src` finds nothing]. Fact: the dispatch site, its receiver expression, and the literal prefix of the name argument if any | Python | `treesitter/calls.rs` (`is_call_node` at `:111`): new `pyfacts.rs::dynamic_dispatch`. A side table, not an `ExtractedCall` field (that struct has about 25 literal sites [V], and a fake callee name would pollute the resolver) | `Extraction.dynamic_dispatch: Vec<DynamicDispatch{caller_symbol, start_byte, kind: Getattr, receiver: Option<String>, name_prefix: Option<String>, name_is_literal: bool}>` | `py_getattr_module` (`rust/devmap-analyze/tests/attack_script_probe.rs:89`, today a passing guard through the file claim), `p_hasattr_dispatch` (`attack_script_probe2.rs:121`). Narrowing the Python script callable exemption (`liveness.rs:75-88`) is the follow-up the fact enables | ft-1d03bb95 (part 1) |
| 22 | **Python scope-binding facts**: for each (scope, name), how many times and by what construct the name is bound (import, assign, augmented, `for`, `with ... as`, walrus, comprehension, `except as`, `def`, `class`, parameter, `del`, `global`/`nonlocal`, match capture, annotation-only), plus whether any binding is conditional (under `if`/`try`/`TYPE_CHECKING`) and byte extents. One fact serves the alias-rebind family: a resolver can bind a function-local alias only when it has a sole unconditional binding. A bind was tried in resolver code and reverted as unsound (BUILD_REPORT lane D round 1) | Python | Export the walk that `python_read_file_facts` (`treesitter/bindings.rs:301`) already performs: one walk per scope, each name recorded as `(name, at, is_rebinding)`, with `python_bound_names`/`python_pattern_captures` (`:117,:153`) doing the target reading. `pyfacts.rs::scope_bindings` calls that walk rather than writing a third binder. [I: lane brief tally] about 11 of the 22 probes are satisfied by the fact as specified and 4 need more than the specified fields, so read each assertion before claiming it | `Extraction.py_bindings: Vec<ScopeBinding{scope: Option<String>, name, kinds: u16, sites: u32, conditional: bool, first_byte, last_byte}>` | `py.AR1` (`known_failing.tsv:15`, `edges.tsv:132`). **[I] mapping, read each assertion before claiming:** ft-572bb69d group `python_field_super_attack.rs:848`, `..attack3.rs:273,280,289`, `..attack4.rs:469`, `..attack5.rs:228,239,524,570,578,584,590`, `python_residue_attack.rs:200`. **Not ignores**: `python_receiver_residue.rs:373` (py.Z6e), `:507` (py.Z4) and `:540` are comment-only pins of honest misses [V: `rg ignore` on that file is empty; the comments say the extractor records no attribute-read initializer, no return-statement or tuple-element facts, and no decorator-to-function link], so one scope-binding fact does not unblock them and they leave this row; match captures (ft-c0afdfef) `py_receiver_residue_attack2.rs:559,565,571,577,583`. Comment-only (not ignored) markers: `attack_resolver_bundle.rs:508`, `attack_resolver_bundle4.rs:130,235` | ft-1d03bb95, ft-f0af68af, ft-572bb69d, ft-c0afdfef, ft-c666ba7c |
| 23 | **TS namespace `scope_locals`**. `is_symbol_binding` (`scope_locals.rs:125`) has no `internal_module` arm (its parent-kind list is `:156-174`), so the namespace's own name is filed as a file-scope local and `name_is_shadowed_by_local` drops every non-call file-scope read of it. The symbol is emitted at `extract_node.rs:535` [V] | TS, TSX, JS | `scope_locals.rs::is_symbol_binding`: add `"internal_module"` (and `"module"` for the `module X {}` keyword form) to the list, with the `name` field test. The `name` may be a `nested_identifier` for `A.B.C` [U: handle it] | none (corrects `scope_locals` and the references that survive) | `a_namespace_read_at_file_scope_is_not_confidently_dead` (`rust/devmap-analyze/tests/ts_namespace_module_scope_read.rs:41`) and its siblings pin the *interim cap*; they must be **inverted** (namespace read at file scope is live at full confidence) and the cap `NAMESPACE_READ_UNEXTRACTED` (`liveness.rs:1362`, used `:1678`) deleted in the same change. `attack_script_probe3.rs:34` control | ft-260158ce |
| 24 | **tsconfig `parent_dir` backslash**: `tsconfig.rs:181` is `rsplit_once('/')` only. The resolver twin (`resolver/candidates.rs:349`) was fixed and has a test (`resolver/tests.rs:15`) [V] | TS | `rust/devmap-extract/src/tsconfig.rs:181` | none. Output changes only for inputs containing `\` | New test needed (no ignored one exists): a `tsconfig` whose `extends`/`baseUrl` is written with `\`. Boundary hardening, but output-changing, so it rides the bump | ft-a43bfbc6 |
| 25 | **Swift extract residue, receiver typing** (ft-80c5d100): the closure parameter `$0`, `v!`, `first`, subscripts, `Set.contains` and similar do not get a receiver type. The typing lives in `swiftscope.rs`, reached from `scope_locals.rs:509` `crate::swiftscope::bindings` [V] | Swift | `swiftscope.rs` (3291 lines): element-type propagation through `compactMap`, `map`, `contains`, `first`, subscripts, `!`, generic `where` constraints, contextual `.init`/`.shared` | none (more `LocalBinding.declared_type`) | 10 ignores citing ft-80c5d100 in `rust/devmap-resolve/tests/swift_properties_attack.rs`: `:236`, `:246`, `:471`, and macro rows `:813`, `:818`, `:821`, `:823`, `:824`, `:825`, `:855` | ft-80c5d100 |
| 26 | **Swift function-type property facts**: a stored or computed property whose type is a closure/function type (annotated, optional, `typealias`, `@escaping`, protocol requirement) or whose initializer is a closure literal, so `x.handler(..)` binds to the property | Swift | `extract_node.rs:1604` writes `return_type: None` for every symbol. Property symbols get `return_type = written type annotation` (the field is the written type of the declaration, verbatim; the resolver decides what is a function type). Unannotated closure literal: sentinel constant `SWIFT_CLOSURE_LITERAL_TYPE`, precedent `OPAQUE_BASE` (`model.rs:1061`). Seam: pre-wired `langdecl::return_type_of(lang,node,source)`, Swift arm in `langdecl/swift.rs` (`property_signature` is the sibling) | `ExtractedSymbol.return_type` (existing); one new const | **12**: `swift_resolver_residue.rs:141,167`; `swift_resolver_residue_attack2.rs:130,137,144,151,158,165`; `swift_resolver_residue_attack.rs:208,279,318`; `swift_close_attack.rs:201` | ft-915b0a03 follow-up |
| 27 | **Swift ft-915b0a03 residue**: top-level `let` emits no symbol; leading-dot member through optional/array/ternary result emits no reference; `\.count` key path has no marker; `let count = self.count` emits no read; a read after a shadowing nested `do` emits no reference; tuple-pattern member properties emit no symbol; nested-type members parent to the bare inner name; `StructuralExempt` wiring for a private property of a type naming a non-synthesising supertype | Swift | `langdecl/swift.rs`, `langcalls/swift.rs`, `swiftscope.rs` | symbols and `references` only (a `ReferenceKind::Name` with a key-path marker needs one: Q5) | **11**: `swift_close_attack.rs:64,249,257,272,296` and `rust/devmap-analyze/tests/swift_close_attack.rs:56,72`; `swift_properties_attack.rs:139,180,526,539` | ft-915b0a03 |
| 28 | **ft-f0af68af residue**: a Python name read only in a string annotation; `global X; X += 1`. (JS object-literal members and `T[k]()` dispatch are named on the card but have no ignored test; not specified here) | Python | `pyfacts.rs`: string-annotation names as `ReferenceKind::Name`; `global` rebinding feeds row 22 | `references`, `py_bindings` | `attack_script_probe3.rs:77`, `:86`; `attack_script_probe2.rs:80`, `:98` | ft-f0af68af |

Inventory rows: **28** (rows 1-20 JVM and Kotlin, 21-28 other).

### 1.3 Probe tally (ignored tests plus known_failing rows; mapping [I] where stated)

| Group | Count |
|---|---|
| Python scope-binding facts (row 22): `py.AR1` 1, ft-572bb69d 13 ignores (the 3 `python_receiver_residue.rs` markers are comment-only pins, not ignores), match captures 5 | 19 |
| V3 type parameters (row 3): 15 probes plus `kg.Z5` | 16 |
| Swift function-type properties (row 26) | 12 |
| V7 property/field symbols (row 7): 9 probes, `kg.V7`, anon-class probe | 11 |
| Swift ft-915b0a03 residue (row 27) | 11 |
| Swift receiver typing, ft-80c5d100 (row 25) | 10 |
| V11 header/body spans (row 11) | 9 |
| Kotlin stacked annotations (row 15) 4, companion (row 16) 2 | 6 |

### 1.4 Settling a charged Kotlin region (rows 15 and 16) [V: built and measured]

The first version of the recovery settled derived tables **by qualified name** after the fact (wiring by the
target of a dropped phantom, scope locals by the scope of one, exports once per name). A phantom and a real
symbol that share a name collided: a real `@Preview fun m()` lost its wiring to a phantom `m`, a real `m`
declared after the region lost its export, phantom locals survived under a real scope, a recovered member was
exported twice. The rule that replaced it: **a table is derived from what the file declares, never patched by a
name.**

- **The walks that derive a table from the tree leave a charged statement out.** The detector's regions are
  set as a thread-local list of byte ranges before the walk (`treesitter/budget.rs`, `set_charged_regions`,
  `in_charged_region`); `BudgetGuard` saves and restores it, so a nested extraction (a recovery's re-parse)
  neither sees nor clobbers the outer file's. The walks that consult it: the declaration arm of `extract_node`
  (no symbol and no wiring is emitted inside a region), `collect_scope_locals` and `collect_non_symbol_locals`
  (no scope and no local, in the region or in the real function around it), and
  `langcalls::kotlin::scope_receivers`. The list is empty for every file but a Kotlin file the detector charged.
- **A recovered region contributes the rows of its re-parse**, shifted to the file's offsets by
  `Staged::of`: symbols, calls, references, wiring, scope locals, local bindings, scope receivers, `jvm_decls`
  and `local_extents`. A refusal names the table that still held the wrapper's name. What carries a span is
  replaced by span in `Staged::apply`.
- **`exports` is derived afresh from the settled symbols** by `publish_exports`, the one function the ordinary
  pass also calls, after recovery and removal. The Kotlin walk publishes no export of its own, so this is the
  whole table; no phantom can hold the slot of a real symbol and nothing is published twice.

Every table of `Extraction` was classified (the same list is on `Staged::apply`):

| Table | How it settles |
|---|---|
| `symbols`, `calls`, `references`, `local_bindings`, `scope_receivers`, `jvm_decls`, `local_extents` | have spans or start bytes: replaced by span (and `symbols`, `scope_receivers` never emitted inside a region) |
| `wiring`, `scope_locals` | keyed by name, so never produced inside a region; a recovered region contributes its re-parse's |
| `exports` | derived from the settled `symbols` |
| `imports`, `routes`, `js_objects`, `rust_*`, `go_*`, `dynamic_dispatch` | other languages, or nothing a region holds (Kotlin imports are header lines) |
| `package_scopes` | file-level headers, not region contents |
| `literals` | Kotlin has no literal decoder today; the day it has, the walk that reads literals must skip charged regions |
| `decl_gaps`, `recovered_decl_ranges` | written by `recover` |

A lane that fills `jvm_decls`, `local_extents` or `package_scopes` must emit them through a walk that skips
charged regions (or, for `package_scopes`, from file-level headers only); the settling above then needs no
change. The tests that hold the principle compare every table of a file with the control (the same file
without the statement, or its one-annotation twin): `tests/kotlin_settled_tables.rs`.

**Follow-ups from the third adversarial round** [V: built and measured; `tests/kotlin_followups.rs`]:

- **A hidden zero-width token is an error.** `has_error()` counts it and no node shows it, so the walk in
  `parse_outcome_of` that looks for ERROR and MISSING nodes came back empty and the file was `Clean`. The walk
  now also charges a node that has an error and has no child that does (and is neither ERROR nor MISSING
  itself). This is in the one shared outcome function; no other language's outcome in the `devmap-extract`,
  `devmap-analyze` or `devmap-resolve` suites moved, so it is not restricted to Kotlin. The charged file then
  takes the row 16 rewrite, whose gate is a `Partial` parse; the repaired tree is kept only when fully clean.
- **The row 15 wrapper is repaired too.** `@Ann(args) class A { val x = 1 }` and a stacked-annotation
  one-liner read as an expression with no error of their own, and the wrapper re-parse was a raw parse that saw
  the one-line body as a syntax error. `check_wrapper` now runs the row 16 repair on the wrapper and checks the
  repaired tree. A one-line stacked class with no space before its last `}` is still unrecoverable.
- **The rewrite is bounded by separators, not passes** (above), starts at the root when the root is the ERROR
  node, and tests a member's end by the last character (any script, postfix `++` and `--`) of the last node
  before the brace that is not a comment.

**A known resolver consequence** [V: Graft, five edges]. Reading a file that was a whole-file ERROR makes its
private nested classes visible. Graft's `CallMirrorTest.FakeScheduler` now shares a name with
`GraftLinkWatchdogTest.FakeScheduler`, and the receiver-type rung abstains on two candidates, so five calls in
`GraftLinkWatchdogTest` to its own `FakeScheduler.tick` moved from edges to the unresolved ledger
(`uninferred_receiver`). None bound to the other file's class. The probe is
`devmap-resolve/tests/private_nested_namesakes_across_files.rs` (ignored, ft-2c800986): V9/V14 (inner/static
and visibility) are the facts that let the resolver prefer the same-file private class, as it already does for
Swift.

## 2. Schema plan

### 2.1 The bump

- `EXTRACTION_SCHEMA_VERSION` `"97"` -> `"98"` at `cache.rs:798`, with a v98 paragraph in the doc
  comment above it. `embedded_scripts.rs:678-679` says the exact pin is a tripwire that forces a
  rationale paragraph, so write one.
- The cache identity is `{ANALYZER_VERSION}:extract-v98` (`cache.rs:859`). The store keys
  `extraction_cache` on it, so a v97 row is never served to a v98 key.
- Exact-pin tests to update in the same commit [V: `rg EXTRACTION_SCHEMA_VERSION`]:
  - `rust/devmap-extract/tests/extract_bindings.rs:14-16`: prepend a v98 clause to the long message.
  - `rust/devmap-extract/tests/embedded_scripts.rs:884`.
  - `rust/devmap-cli/tests/a_v96_swift_extraction_is_not_served_after_the_bump.rs:23` and
    `a_v94_python_extraction_is_not_served_after_the_bump.rs:21` assert `== "97"` and will fail.
    Fix the class: change those two asserts to a numeric `>= 97` so the next bump does not break
    them again. Their real claim is the stored-identity behaviour, not the number.

### 2.2 The proof test (new): `rust/devmap-cli/tests/a_v97_extraction_is_not_served_after_the_bump.rs`

Copy the shape of `a_v96_swift_extraction_is_not_served_after_the_bump.rs` [V, read in full]:

1. `the_current_extraction_carries_the_v98_facts`: extract a Kotlin file with a stacked-annotation
   function (row 15) and a Java file with a typed parameter (row 6); assert the function is a symbol
   and the Java `LocalBinding.declared_type` is set; assert `EXTRACTION_SCHEMA_VERSION == "98"`.
2. `a_payload_stored_under_the_v97_identity_is_not_served_to_a_v98_key`: extract, strip the v98 facts
   (drop the `f` symbol, clear `jvm_decls`, `package_scopes`, `local_extents`, `decl_gaps`,
   `dynamic_dispatch`, `py_bindings`), store under `format!("{ANALYZER_VERSION}:extract-v97")` with
   `admit_cached_extraction`, assert the v97 row is stored (control) and
   `try_get_cached_extraction(&current)` is `None`.
3. `the_stored_identity_names_v98`: `key.analyzer_version.ends_with(":extract-v98")`.

Add one test per row that **fails against the pre-fix code** (CLAUDE.md: no test, no fix): the ignored
tests in the inventory are those tests; un-ignore them in the lane that lands the fact.

### 2.3 Store schema impact

- **None required** [I]. `devmap-store` `CURRENT_SCHEMA_VERSION` is 27, `MIN_READER_SCHEMA_VERSION`
  25 (`schema.rs:1372,1399`). The store persists the serde `Extraction` payload with `source_code`
  stripped (`db/write.rs:517`), and every new field is `serde(default)`, so old rows still parse.
  New symbols reuse `SymbolKind::Field`, `Class`, `Interface`.
- To verify before the bump [U]: that no `CHECK` constraint or kind table in the store rejects a
  Java `Field` row, and that a v27 store opens and rebuilds under v98 without `devmap repair`.
  Run `devmap build --full` on a v97-built store as a test.
- If any lane decides it needs a column (it should not), that is a store schema change, which the
  report (`ft-f6abefd9`, `ft-3bdf1450`, `ft-38ef7393`) says needs the owner. Stop and ask.

### 2.4 Consumers that must re-vendor

CLAUDE.md rule: a schema change is carried into every reader in the same piece of work, and done
means every installed consumer reports the new number. This bundle changes **devmap-extract only**
(98). `dc-store` 10, `WORKBENCH_SCHEMA` 14 and devmap-store 27 are unchanged [I].

| Consumer | Action | Done when |
|---|---|---|
| GitPulse (vendors `devmap-extract`, `scripts/vendor-crates.mjs:143`) | `node scripts/vendor-crates.mjs`, `bun run check:vendor-schema`, commit; `bun run mcp:install && bun run mcp:doctor` (all three verdicts OK); `bun run tauri build`, reinstall GitPulse.app | `bun run check:schema-parity` passes with no stale row |
| Manvi (links DevCouncil crates live, pins in `scripts/module-pins.txt`) | bump the DevCouncil pin to the v98 merge commit; `manvi doctor` | doctor reports devmap-extract 98 |
| Jarvis (pins in `upstream.lock.json` and `go.mod`; gusset must agree in all three places) | bump the DevCouncil pin; `dev check-pins` and doctor | pins agree, doctor OK |
| Installed `devmap` | rebuild and reinstall; `devmap --version`; `devmap build --full` so warm stores pick up v98 | version line shows the v98 build |

**A pin bump changes what a repo claims to build against: report the deltas and ask before bumping.**
From a session rooted in this repo, hand over the commands; do not report it as done. The pin
values in the BUILD_REPORT handover (`1ae3bc15`) disagree with `Manvi/scripts/module-pins.txt`
(`3cfe283c...`) [V: read]; take the HEAD at sweep time.

### 2.5 Order of work

0. **Gate**: the v97 consumer sweep is closed, `check:schema-parity` passes, DevCouncil main is pushed
   once, and the owner unfreezes v98.
1. **Commit 0, scaffold (one owner, small, lands first).** Add all new model structs and empty
   `Extraction` fields (updating every `Extraction` literal), the version bump, the `a_v97` test and
   pin updates, and **pre-wire the dispatch seams** so lanes touch only leaf files:
   `jvmfacts::collect`, `pyfacts::collect` (both return empty), `langdecl::return_type_of` and
   `langdecl::symbol_extras` (return `None`), the `langdecl/java.rs` registry arm (delegates to
   `generic`). No behaviour change; the corpus edge set must be byte-identical to main except the
   version stamp.
2. **Phase E, two parallel lanes** (section 3), each rebased on commit 0.
3. **Merge**, then full suites on the combined tree (the report merged lanes without building the
   combined tree once; do not repeat that).
4. **Phase R, serial, one lane**: rebase `lane/jvm-scope-model` onto v98, wire `jvm_tables.rs` and
   `names.rs` to read `jvm_decls`/`package_scopes`/`local_extents`/`decl_gaps`, delete the Scala stem
   allow-list (`importpath.rs:54`) and the `jvm_package_symbols` callers, un-ignore the 45 probes.
   Two consumers will be un-ignored here: `ov_csharp_...` and `j10`.
5. **Consumer sweep for v98** (2.4), then `devmap build --full`, then push once.

## 3. Lanes

Two lanes, no file overlap. Commit 0 owns `model.rs`, `cache.rs`, `entry.rs` (the one dispatch line
per fact family), `langdecl/mod.rs` seams and the version tests; neither lane edits those afterwards.

### Lane A: JVM declaration facts, Kotlin loss, TS namespace (rows 1-20, 23)

Files (all under `rust/devmap-extract/src/` unless noted):

- new `jvmfacts.rs`
- new `langdecl/java.rs`; edits to `langdecl/kotlin.rs`; the `generic` fallback in `langdecl/mod.rs`
  is read only, its seams were wired in commit 0
- `langimports/` java and kotlin modules (row 4)
- `langfields.rs` (Java parameter bindings, row 6; **deferred**, another session's Swift lane owns the
  file; the first draft named `treesitter/rust_use.rs::param_type_bindings`, which is not the seam)
- `treesitter/entry.rs::parse_outcome_of` and a new `kotlinloss.rs` (row 15 detector and recovery)
- `treesitter/scope_locals.rs` (V12 extents, `is_symbol_binding` for new symbols and TS `internal_module`)
- `treesitter/lang_helpers.rs` (only if a Java kind must be added to `generic_symbol_kind`; prefer
  `langdecl/java.rs` instead and leave this file untouched)
- `langcalls/kotlin.rs`, `langcalls/java.rs`, `langcalls/scala.rs` (scope receivers, overload callee names)
- `rust/devmap-analyze/src/liveness.rs` (delete `NAMESPACE_READ_UNEXTRACTED`, `:1362,:1678`) and
  `rust/devmap-analyze/tests/ts_namespace_module_scope_read.rs` (invert)
- tests: `rust/devmap-extract/tests/declarations.rs`, new `jvm_facts.rs`, `rust/devmap-resolve/tests/kotlin_receivers_and_packages.rs`,
  `overload_self_edges.rs`, new fixtures under `rust/testdata/accuracy/{java,scala,csharp}/`

**Corpus diff gate A**, 12 repos (AppLock, Blitztree, ChronosFlow, Curio, DevType, Graft, M5Blade,
MarkDev, Meridian, scholarlm, Strait, Vani; the same set as the report), `devmap build --full`, edge
and node dumps against main:
- zero lost symbols (recovery only adds; a removed symbol must be a renamed overload id, mapped);
- zero lost true edges: every removed edge is (a) a same-file overload self-edge or (b) an id rename
  from row 17/18 whose target appears under the new id; no other removal;
- new `Field` symbols (row 7): `devmap dead` rows at the confident tier must not grow except for
  genuinely unread properties, sampled and classified (Kotlin/Java property symbols are the largest
  output change in the bundle; Swift's v97 precedent is `is_member_property`, `model.rs:851`);
- the corpus has **no Java or Scala** (report: "the corpus has neither language"), so the Java, Scala
  and C# fixtures are the only evidence for rows 1, 2, 6, 8; they land before the lane merges;
- bench: `devmap build rust --full`, release, instructions retired, min of 3, interleaved, within the
  5% gate the report uses; and the 30k-method file (`resolve_bench.rs`, +12% at commit 1) re-measured.

### Lane B: Python, Swift, tooling (rows 21, 22, 24, 25, 26, 27, 28)

Files (all under `rust/devmap-extract/src/` unless noted):

- new `pyfacts.rs`; edits to `treesitter/calls.rs` (getattr) and `treesitter/bindings.rs` (binder reuse
  only; do not edit `scope_locals.rs`, Lane A owns it)
- `swiftscope.rs`, `langdecl/swift.rs`, `langcalls/swift.rs`
- `tsconfig.rs` (row 24)
- tests: `rust/devmap-resolve/tests/swift_*`, `python_*`, `py_receiver_residue_attack2.rs`,
  `rust/devmap-analyze/tests/attack_script_probe*.rs`, `python_script_entry_liveness.rs`,
  `rust/devmap-cli` bindings, new `rust/devmap-extract/tests/tsconfig_backslash.rs`

**Corpus diff gate B**, same 12 repos: zero lost true edges and zero lost symbols; any added Swift
property type must not remove a true edge; for `py_bindings`, **no resolver behaviour changes in
this bundle** (the facts are recorded, adoption is Phase R or a follow-up), so the edge set is
byte-identical except where a Swift `declared_type` or key-path reference is added, and every added
edge is sampled. `py.AR1` stays in `known_failing.tsv` until the resolver adopts the fact.

**No file overlap, checked** [V by reading]: A touches `scope_locals.rs` and `rust_use.rs`; B touches
neither. Both call `swiftscope.rs` only from `scope_locals.rs:509`, which is not edited. `extract_node.rs`
is edited by commit 0 only (the Java param loop at `:1610` and Swift `return_type` at `:1604` are
adjacent hunks, which is why both are routed through seams).

Phase R (serial, after both): `rust/devmap-resolve/src/resolver/{jvm_tables,names,index,imports,dispatch}.rs`,
`importpath.rs`, plus `known_failing.tsv`. Not parallel with Phase E, so it does not count against
the two-lane limit.

## 4. Risks and open questions

Risks:

1. **Largest output change is V7**: every Kotlin property and Java field becomes a `Field` symbol.
   Node count, build time and dead-code rows all move. Mitigation: the dead-row gate in A; if rows
   explode, ship V7 with the properties non-exported-exempt tagged first and tighten later.
2. **Id churn** from rows 17, 18 and the property-versus-callable collision (Q2). Joins keyed by id
   in consumers (GitPulse ledger, Manvi) will see renames. Mitigated by renaming only collision
   groups, but consumers must be told in the sweep report.
3. **Recovery can hallucinate** (rows 13, 15, 16). A re-parse that emits a symbol that is not there is
   worse than a drop. Gate: repaired ranges are recorded in `recovered_decl_ranges` and unrecovered ones in `decl_gaps`, the loss detector is a tree
   check (an `identifier` node whose text is a hard keyword; strings and comments are not identifier
   nodes, so there is no text scan to run against them), the wrapper parse must be Clean before
   anything is emitted, and the corpus gate counts added symbols and edges, not only removed.
4. **The Kotlin grammar is the root cause of rows 15 and 16.** A grammar bump might fix both without
   recovery code, but `languages.rs:1236` pins the grammar versions and a bump is a new dependency:
   ask first (Q1).
5. **Python `py_bindings` size.** One record per (scope, name) on every Python file is large. Cap per
   file, record the cap, and drop single-site never-rebound names unless they are import aliases (the
   only consumer). [I] sizing needs a measurement on `scholarlm`.
6. **Wall-clock tests fail under load** (report, "Not verified"; 5 s parse budget refuses large
   generated files). Run lane suites at low load, and re-run a failure alone before blaming a lane.
7. **Combined tree untested** was a prior-wave gap. Build and test the merged A+B tree.
8. **The devmap MCP is misconfigured** (points at a deleted `tmp/fix-wb` binary); this design used the
   CLI, `rg` and source reads. DevMap impact answers were not used and none are claimed.
9. Windows: `\` handling (row 24) cannot be tested on this Mac beyond string inputs.

Open questions (each needs a ruling or a measurement):

- **Q1** Grammar bump for tree-sitter-kotlin-ng (new dependency, ask) versus recovery code (rows
  15, 16)? Recommendation: recovery code plus the loss detector now. **The detector is the tree
  check, not a text scan** (an `identifier` node spelling a hard keyword, row 15), and it covers every
  affected declaration kind (function, class, object, interface, property, alias), not only `fun`.
  Row 15 recovery is code; row 16 recovery is a separate piece (structured ERROR walk or `;` rewrite).
- **Q2** Id rule for a Kotlin property and a same-named function (`c05`, `c04`): property keeps the bare
  id and callables in the group take `(params)`? That renames an existing function id when a property
  is added. Recommendation: yes, collision groups only.
- **Q3** Distinct overload ids for Swift, C# (partial classes across files) and Scala as well as
  Kotlin/Java? `ov_swift_extensions_across_files...` (`attack_resolver_bundle3.rs:309`) and the C#
  partial probe need cross-file knowledge that a per-node rule does not have (V2 covers C#).
  Recommendation: Kotlin and Java in v98; the others after.
- **Q4** Anonymous-class symbol name (row 8): `Owner.<anon@byte>`? Byte offsets change on every edit
  above them; an ordinal among siblings is steadier. Recommendation: ordinal.
- **Q5** Key-path marker (row 27): a new `ReferenceKind` ripples through every `match`; a `KeyPath`
  receiver sentinel in `receiver_expr` does not. Recommendation: the sentinel.
- **Q6** Scope: rows 19, 20 (Z1, Z2, Z4, Z6) have no fact defined by the design doc. In or out of v98?
  Recommendation: out, except Z4's `body_shape` if cheap.
- **Q7** Should rows 22, 25, 26, 27, 28 (Python and Swift, from ignores citing "schema frozen at 97"
  but not named in the V1-V14 doc) ride this bump? They need one bump anyway (the freeze is global),
  and a second bump costs another consumer sweep. Recommendation: yes, lane B.
- **Q8** `ImportKind` (row 4) is optional hardening with no probe. Keep or cut? It adds a field on a
  15-site struct. Recommendation: cut unless Phase R shows the text parser failing.
- **[U] first-step verifications** before any lane starts: ~~dump Kotlin tree-sitter shapes for rows 15
  and 16~~ (row 15 done: `annotated_expression` chain around an `infix_expression`, or an `assignment`
  for `fun f() = 1`; row 16 is a whole-file ERROR, see rows 13 and 16); confirm Scala and C# node names for rows 1, 2; read each ft-572bb69d assertion to confirm
  one `py_bindings` fact is enough; confirm the Java local-declarator path for row 6.
