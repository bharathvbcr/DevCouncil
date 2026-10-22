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
| 6 | **V6 Java declared types** for formal parameters and call-initialised locals | Java | `treesitter/rust_use.rs:609 param_type_bindings` has arms for `rust`, `go` and C-family only [V]; add `("java","formal_parameter" \| "spread_parameter")`. Its caller guard `extract_node.rs:1610` is `if c_family`, widen to Java (the `parameters` field of `method_declaration`/`constructor_declaration` is already found by `parameter_list_of`, `rust_use.rs:669`). Call-initialised locals (`Repo r = make();`): the Java local declarator needs the same `Type` reference with `assigned_to` [U: confirm where `new`-initialised Java locals get theirs; `langcalls/java.rs` `record`] | No new field. Emits `ReferenceKind::Type` with `assigned_to = param`, which `binding_declared_types` (`scope_locals.rs:440`) already turns into `LocalBinding.declared_type` | `r3` (`attack_resolver_bundle5.rs:265`), `j6` (:116), `e7` (:387), `java_typed_parameter_receiver_binds` (`overload_self_edges.rs:110`) | ft-2c800986, ft-eda9b3a6 |
| 7 | **V7 property and field symbols**: Kotlin properties, Java fields, enum entries, companion constants | Kotlin, Java | Kotlin `langdecl/kotlin.rs:65`: `property_declaration` returns a symbol only for an extension property; add the ordinary-property arm (kind `Field`, signature tag). Java: new `langdecl/java.rs` for `field_declaration` (one symbol per `variable_declarator`) and `enum_constant`; `generic_symbol_kind` (`lang_helpers.rs:1406`) has neither [V]. `is_symbol_binding` (`scope_locals.rs:125`) must treat the new names as symbols, as it does for Swift (`:148`) | New `ExtractedSymbol` rows (`SymbolKind::Field`), signature tags `kotlin property`/`java field`/`enum entry`. No new struct field | 9 probes: `c02b` (`jvm_names_attack2.rs:363`), `c04`:383, `c05`:397, `c09`:460, `o05`:552, `i02`:738, `m03`:885, `c14`:933, `c15`:948; row `kg.V7` (`known_failing.tsv:26`); `v8_a_java_anonymous_class_method_is_not_a_member_of_its_host` (`attack_resolver_bundle5.rs:641`) shares V8. **11** | ft-2c800986 |
| 8 | **V8 Java `record`, `@interface`, anonymous class symbols** | Java | `langdecl/java.rs`: `record_declaration`, `annotation_type_declaration`, `object_creation_expression` with a `class_body` [U]. Neither of the first two is in `generic_symbol_kind` [V] | New symbols: record => `Class`, signature `record`; `@interface` => `Interface`, signature `@interface`; anonymous class gets an owner-scoped synthetic name (open question Q4) | `v8_a_java_record_method_is_not_a_member_of_its_host` (`attack_resolver_bundle5.rs:627`), anon at :641 | ft-2c800986 |
| 9 | **V9 `inner` / `static` modifiers** on nested types | Kotlin, Java | `jvmfacts.rs`; Kotlin `class_modifier` `inner` (`langdecl/kotlin.rs:198 has_modifier`), Java `modifiers` containing `static` | `JvmDeclFact.modifiers` | `a01` (`jvm_names_attack.rs:61`), `a01b`:70 | ft-2c800986 |
| 10 | **V10 companion objects** with their own span, named or not | Kotlin | `jvmfacts.rs`; node `companion_object`. `langdecl/kotlin.rs:44-48` skips it on purpose so members stay `C.f`. **Keep that**: no symbol, no id churn | `JvmDeclFact{kind: Companion, companion_name, body}`, parent `qualified_name` = host | `c06` (`jvm_names_attack2.rs:411`), `c06b`:425, `i03b`:760 | ft-2c800986 |
| 11 | **V11 header and body spans** (supertype list, primary ctor, annotation args, `where`, type-parameter bounds, delegation) | Kotlin, Java | `jvmfacts.rs`; Kotlin `class_declaration` fields (name, `type_parameters`, `primary_constructor`, `delegation_specifiers`, `type_constraints`, `class_body`) [U]; Java `class_declaration` `superclass`/`super_interfaces`/`type_parameters`/`body` | `JvmDeclFact.header`, `.body` | 9 probes: `d01` (`jvm_names_attack2.rs:899`), `h01`:214, `h03`:255, `h03b`:266, `h03c`:958, `h05`:286, `h07`:307, `h08`:318, `h08b`:969 | ft-2c800986 |
| 12 | **V12 block and lambda extents of local bindings** | Kotlin, Java | `treesitter/scope_locals.rs` (`collect_scope_locals`, called `entry.rs:947`): record the extent of the enclosing block or lambda for each local | `Extraction.local_extents` (`scope_locals` stays as is: `Vec<(String,String)>`, `model.rs:1663`) | `a17e` (`jvm_names_attack.rs:456`) | ft-2c800986 |
| 13 | **V13 parse-drop flag**: which declarations the Kotlin grammar lost after an error; some one-line class bodies parse Partial | Kotlin (Java, Scala same shape) | `entry.rs:1076 parse_outcome_of` already yields `Partial{error_ranges, nameless_ranges}`. New `jvmfacts.rs::decl_gaps` records, per error range, the span from the error start to the end of the construct the walker could not enter, plus the recovery described in rows 14 and 15. Recovery step: re-parse the tail after an error range at the next line start, merge its symbols (offset-shifted), mark them recovered | `Extraction.decl_gaps` | `a21b` (`jvm_names_attack.rs:526`), `p01` (`jvm_names_attack2.rs:825`), `p02`:837 | ft-2c800986, ft-5ad86836 |
| 14 | **V14 visibility modifiers** (`internal`, package-private, `protected`) | Kotlin, Java | `jvmfacts.rs`; Kotlin `visibility_modifier` (`langdecl/kotlin.rs:190`); Java `modifiers` (absence of public/protected/private = package-private) | `JvmDeclFact.modifiers`. `is_exported` stays as is: it is a dead-code claim and measured wrong when widened (`langdecl/kotlin.rs:158-184`) | `v03` (`jvm_names_attack2.rs:610`), `v04`:622, `v05`:635 | ft-2c800986 |
| 15 | **Kotlin stacked-annotation silent drop**. `@A\n@B\nprivate fun f() {}` after a property parses **Clean** into an expression node and `f` vanishes; Clean means no `Partial` flag | Kotlin | `langdecl/kotlin.rs::declaration` (`:27`) never sees a `function_declaration`. Recovery arm: accept the expression node (BUILD_REPORT says `infix_expression`, the ignore strings say `annotated_expression`; **[U] dump the tree first**) when its leading children are annotations/modifiers followed by `fun`, and build the declaration from them. Fallback if the shape is irregular: re-parse the node's byte range alone. **Independent loss detector**: for a Clean Kotlin file, a keyword scan (`fun`, `class`, `object`, `interface` at statement start, outside strings and comments) that finds a keyword with no symbol at that offset downgrades the outcome to `Partial` with that range in `error_ranges`. This is what makes the failure loud instead of silent | Symbols re-emitted (ids unchanged); detector uses existing `ParseOutcome::Partial` | `kotlin_stacked_annotations_after_a_property_initializer_keep_the_function` (`rust/devmap-extract/tests/declarations.rs:616`), `..._multiline_initializer_...` :627, `kotlin_annotations_with_arguments_...` :638, `kotlin_stacked_annotations_after_a_blank_line_...` :649. Control `kotlin_annotation_stacks_that_still_parse_as_declarations_keep_the_function` (:663) must stay green | ft-5ad86836 (AC1, HIGH) |
| 16 | **One-line companion object**. `class C { companion object { fun f() = 1 } }` is an ERROR node and drops `C`, `C.f` and the following top-level funs | Kotlin | Same recovery as row 13 plus a targeted rule: an ERROR node whose text starts with `companion object` inside a `class_body` is re-parsed from the class start with the brace structure normalised [U: prefer the generic tail re-parse; a grammar bump is a new dependency, ask first] | Symbols re-emitted; `JvmDeclFact{kind: Companion}` (row 10) | `kotlin_one_line_companion_object_keeps_the_class_and_later_functions` (`declarations.rs:690`), `kotlin_one_line_named_companion_...` :705. Control `kotlin_one_line_object_keeps_the_object_and_later_functions` (:716) | ft-5ad86836 (AC2) |
| 17 | **Distinct overload ids**. Two `add` in one class share `Overloads.kt::Calc.add`, so the resolver sees a self-edge | Kotlin and Java first; Swift, C#, Scala listed in Q3 | `langdecl/mod.rs::Declaration::qualified` (`:97`). Sibling-aware rule in `declaration_of`: when another callable of the same name has the same owner (scan the parent's children, bounded), append the written parameter types: `Calc.add(Int)`, `Calc.add(String)`; identical text gets `#2` by span order. Un-overloaded ids are **unchanged**. Because `langcalls::scope` calls the same function, `caller_symbol` stays consistent [V: `langdecl/mod.rs:43-48`] | `ExtractedSymbol.qualified_name` for overload groups only; `name` unchanged | `overloads_keep_distinct_identities_and_never_self_edge` (`kotlin_receivers_and_packages.rs:62`), `java_typed_parameter_receiver_binds` (`overload_self_edges.rs:110`, also needs row 6) | ft-eda9b3a6 (AC4) |
| 18 | **Same-name nested types under different outers**: `A.Inner.run` and `B.Inner.run` both become `Inner.run`, since `enclosing_owner` keeps only the direct parent (`langdecl/mod.rs:247`) [V]. `enclosing_owner_path` (`:223`) exists but is applied to enum entries only | Kotlin, Java | `langdecl/mod.rs`: use the full owner chain **only when the nested type's name is declared more than once in the file** (a per-file set computed once, cached like the per-scope cache). Widening unconditionally rewrites every nested-type member id (the SC14 open half, `mod.rs:215-222`) | `qualified_name` for colliding nested types only | `ov_java_two_outer_each_with_inner_same_name_one_file` (`attack_resolver_bundle3.rs:279`, a loss recorded, not an ignore); no ignored test: add one | ft-eda9b3a6 |
| 19 | **Z4 expression-body return shape**: `fun f() = Foo()` has no written return type; `return_type` holds written types only (`known_failing.tsv:12`) | Kotlin | `langdecl/kotlin.rs` + `extract_node.rs:1604` seam: record the initializer shape of an expression body (`Foo{..}` or callee name, the same spelling `initializer_shape` uses, `model.rs:1107`) | Reuse `ExtractedSymbol.return_type` is wrong (it would claim a written type). New `JvmDeclFact.body_shape: Option<String>` | `kg.Z4` (`known_failing.tsv:12`) | ft-87b28bb6 |
| 20 | **Z1, Z2, Z6 recall facts** (stdlib String-method return types, lambda parameter typed by the callee's function-type parameter, `T::class.java`) | Kotlin | **Not specified.** The design doc defines no fact for them. `kg.Z1`, `kg.Z2` are resolver or `treesitter.rs` binding work. Left out of v98 unless a ruling says otherwise (Q6) | none | `kg.Z1` :9, `kg.Z2` :10, `kg.Z6` :14 | ft-87b28bb6 |

### 1.2 Python, TS, tooling

| # | Fact | Langs | Emit at | Model field | Unblocks | Cards |
|---|------|-------|---------|-------------|----------|-------|
| 21 | **getattr outer-call record**. `getattr(o, 'cmd_' + n)()` parses as a `call` whose callee is itself a `call`; no extractor code mentions `getattr` [V: `rg getattr rust/devmap-extract/src` finds nothing]. Fact: the dispatch site, its receiver expression, and the literal prefix of the name argument if any | Python | `treesitter/calls.rs` (`is_call_node` at `:111`): new `pyfacts.rs::dynamic_dispatch`. A side table, not an `ExtractedCall` field (that struct has about 25 literal sites [V], and a fake callee name would pollute the resolver) | `Extraction.dynamic_dispatch: Vec<DynamicDispatch{caller_symbol, start_byte, kind: Getattr, receiver: Option<String>, name_prefix: Option<String>, name_is_literal: bool}>` | `py_getattr_module` (`rust/devmap-analyze/tests/attack_script_probe.rs:89`, today a passing guard through the file claim), `p_hasattr_dispatch` (`attack_script_probe2.rs:121`). Narrowing the Python script callable exemption (`liveness.rs:75-88`) is the follow-up the fact enables | ft-1d03bb95 (part 1) |
| 22 | **Python scope-binding facts**: for each (scope, name), how many times and by what construct the name is bound (import, assign, augmented, `for`, `with ... as`, walrus, comprehension, `except as`, `def`, `class`, parameter, `del`, `global`/`nonlocal`, match capture, annotation-only), plus whether any binding is conditional (under `if`/`try`/`TYPE_CHECKING`) and byte extents. One fact serves the alias-rebind family: a resolver can bind a function-local alias only when it has a sole unconditional binding. A bind was tried in resolver code and reverted as unsound (BUILD_REPORT lane D round 1) | Python | `pyfacts.rs::scope_bindings`, reusing `python_bound_names` and `python_pattern_captures` (`treesitter/bindings.rs:117,153`) rather than a third binder | `Extraction.py_bindings: Vec<ScopeBinding{scope: Option<String>, name, kinds: u16, sites: u32, conditional: bool, first_byte, last_byte}>` | `py.AR1` (`known_failing.tsv:15`, `edges.tsv:132`). **[I] mapping, read each assertion before claiming:** ft-572bb69d group `python_field_super_attack.rs:848`, `..attack3.rs:273,280,289`, `..attack4.rs:469`, `..attack5.rs:228,239,524,570,578,584,590`, `python_residue_attack.rs:200`, `python_receiver_residue.rs:373,507,540`; match captures (ft-c0afdfef) `py_receiver_residue_attack2.rs:559,565,571,577,583`. Comment-only (not ignored) markers: `attack_resolver_bundle.rs:508`, `attack_resolver_bundle4.rs:130,235` | ft-1d03bb95, ft-f0af68af, ft-572bb69d, ft-c0afdfef, ft-c666ba7c |
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
| Python scope-binding facts (row 22): `py.AR1` 1, ft-572bb69d 16, match captures 5 | 22 |
| V3 type parameters (row 3): 15 probes plus `kg.Z5` | 16 |
| Swift function-type properties (row 26) | 12 |
| V7 property/field symbols (row 7): 9 probes, `kg.V7`, anon-class probe | 11 |
| Swift ft-915b0a03 residue (row 27) | 11 |
| Swift receiver typing, ft-80c5d100 (row 25) | 10 |
| V11 header/body spans (row 11) | 9 |
| Kotlin stacked annotations (row 15) 4, companion (row 16) 2 | 6 |

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
- `treesitter/rust_use.rs` (`param_type_bindings` Java arm, row 6)
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
   worse than a drop. Gate: recovered symbols are labelled in `decl_gaps`/`jvm_decls`, the keyword
   scan runs against comments and strings, and the corpus gate counts added symbols, not only removed.
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
  15, 16)? Recommendation: recovery code plus the keyword-scan loss detector now.
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
- **[U] first-step verifications** before any lane starts: dump Kotlin tree-sitter shapes for rows 15
  and 16 (which node kind: `infix_expression` per the report, `annotated_expression` per the ignore
  strings); confirm Scala and C# node names for rows 1, 2; read each ft-572bb69d assertion to confirm
  one `py_bindings` fact is enough; confirm the Java local-declarator path for row 6.
