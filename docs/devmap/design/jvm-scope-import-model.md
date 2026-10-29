# One scope and import model for the JVM family (and Python's bare names)

Status: Paused at commit 2. The extraction facts ("v98 JVM declaration facts":
`jvm_decls`, `package_scopes`, `local_extents`, `decl_gaps`, `recovered_decl_ranges`) have
landed on local `main` with `EXTRACTION_SCHEMA_VERSION = "98"` (`docs/devmap/design/extract-v98.md`).
The resolver branch `lane/jvm-scope-model` remains unmerged pending Phase R adoption and
un-ignoring of the 45 probes after the consumer sweep. Approved with changes (Fable, 2026-10-21);
see "Review ruling" at the end; where it conflicts with the body, the ruling wins.

Card: ft-2c800986. (Original status: design for review, no code until approved.)
Base: `b888043d` (main). Author's evidence labels: **[V]** verified in code at the cited
`file:line`, **[I]** inferred from code reading, **[U]** unverified.

Scope of the card: Kotlin nominal factory typing, Kotlin/Java/Scala import resolution,
Python function-scoped imports, and the cross-file split-type identity (Swift extension,
C# partial) that round 2 patched by spelling. The six round-2 seams are replaced by one
owner, `resolve_name`, rather than patched again.

## 1. Current model (how a name resolves today)

All paths are under `rust/devmap-resolve/src/resolver/` unless stated.

### 1.1 Facts the extractor gives for JVM files

- `Extraction.jvm_package` (`devmap-extract/src/model.rs:1588`): the first `package`
  header, **Kotlin and Java only** (`treesitter/entry.rs:925-927`, reader
  `treesitter/rust_use.rs:896`). Scala gets `None` [V]. One package per file, so a Scala
  `package a { package b { ... } }` has no representation at all [V].
- `ExtractedImport` (`model.rs:877`): `module_specifier` is the whole dotted path
  (`a.b.Repo`, wildcard as `a.b.*`), `alias` only for Kotlin `as`, and `imported_names`
  is **empty for every JVM import** (`langimports/mod.rs:184-213`) [V]. A Java `import
  static a.B.c` drops the member: the specifier is `a.B` (`langimports/mod.rs:335-342`);
  only `raw_import` still carries it [V]. Scala selector lists become `pkg.*`
  (`langimports/scala.rs:61-72`) [V].
- Symbols: `qualified_name`, `parent_symbol`, `return_type` (verbatim, generics kept,
  `model.rs:836`). A nested type is `Outer.Inner` but **its members are parented to the
  bare `Inner`** (`tests/attack_resolver_bundle2.rs:158`, Swift twin
  `tests/self_in_a_type_is_that_type.rs:198-220`) [V]. A Kotlin companion's members belong
  to the enclosing class (`devmap-extract/src/langdecl/kotlin.rs:45-48`); an extension
  function is filed under its receiver spelling (`fun Task.f()` is `Task.f`)
  (`kotlin.rs:105-130`) [V].
- References (Type, Heritage, Constructor, Call with `assigned_to`), `LocalBinding`
  (`declared_type`, `initializer`, `scope`), `scope_locals`, Kotlin `scope_receivers` [V].
- Absent: Scala package, a per-symbol package, import kind flags, type-parameter names,
  C# namespace [V, by grep of `model.rs`].

### 1.2 Index tables built from them (`index.rs`)

- `jvm_package_by_file` and `jvm_package_symbols: (package, simple name) -> files`
  (`index.rs:410-421`). The second table holds **every** non-file symbol (members, nested
  types, methods), not only top-level ones [V].
- `type_methods: (family, bare type name, member) -> [(file, symbol)]`
  (`index.rs:190-200`), `supertypes`/`subtypes` by bare name (`index.rs:1321`),
  `return_types` by `(file, symbol)`. **The key is the bare simple name**: `a.Repo`,
  `b.Repo` and `Outer.Repo` share one row [V].
- Java's family is `CStyle`, shared with C, C++, C#, ObjC, CUDA (`model.rs:70`); Kotlin
  and Scala have their own families (`model.rs:95,98`). `Family::admits` is equality
  (`model.rs:127`). So `type_methods` cannot join a Java caller to a Kotlin class;
  `jvm_qualified_member` (`imports.rs:217-265`) is a hand bridge for that [V].

### 1.3 Import table

`index.rs:1014-1174` builds `import_bindings[file][local] = (target file, declared name)`:

- Explicit `import a.b.C`: `resolve_import_targets` (`imports.rs:160-201`) first tries
  `resolve_import_path`, and only when that returns nothing, for Kotlin/Java,
  `resolve_jvm_import` (`imports.rs:338-355`), which reads the `(package, name)` table.
  Alias: Kotlin `as` keeps the declared name (`index.rs:1146-1150`).
- **Star import**: `import a.*` is expanded by *directory*, not package header
  (`imports.rs:188`, `resolve_package_wildcard` `imports.rs:673-704`), and then bound
  under the local name `"*"` because the alias is not `Some("*")` and
  `import_local_name("a.*")` is `"*"` (`scope.rs:22-28`, `index.rs:1134-1172`). The
  `STAR_IMPORT` branch (`index.rs:1057`) only fires for Python `from m import *` [V].
  A star import therefore binds **no** repo name; an unindexed `import ext.*` lands in
  `external_imports["*"]`, which `names_a_non_local_import` (`index.rs:1863-1870`) reads
  as "any bare name may come from there" [V].
- Default imports (`kotlin.*`, `java.lang.*`, `scala._`): not modelled for types. Only a
  list of bare *function* names exists (`builtins.rs:1260-1300`) [V].

### 1.4 The stem-bind rung

`import_path.rs:344-390`. For every table-driven language (Java, Kotlin, Scala, C, PHP,
Ruby, Dart ...) the specifier becomes candidate paths; if none is indexed, **the last
rung binds a candidate whose basename names exactly one indexed file anywhere**
(`import_path.rs:382-389`, `unique_basename`, built `index.rs:78-90`). It was written for
C includes and Erlang `-include_lib`. For Kotlin, `import ext.Repo` makes the candidate
`ext/Repo.kt`, whose basename `Repo.kt` is unique, so the binding goes to `a/Repo.kt`
although package `ext` is a library's [V by reading; the probes j9/s6 assert the result].
Gradle module roots (`import_path.rs:358-366`) are Kotlin/Java only [V].

### 1.5 Typing: declared types, factory returns, receivers

- A declared type (`r: Repo`) becomes the **bare string** `Repo`
  (`receiver_types.rs:737-764`, `admissible_nominal_type` `:766`). Nothing records which
  `Repo`.
- Factory return (`val r = make()`): `factory_return_type` (`value_types.rs:247-344`).
  Callee: for a bare `make` it is `lookup_in_package(file, "make")` (`:307`), which
  accepts only a **file-level declaration of the same file** (`dispatch.rs:194-223`;
  cross-file only for Go), else an import binding (`:315`). The return text is reduced to
  a bare name and accepted when `unique_indexed_type` (`value_types.rs:10-21`: exactly
  one Class/Struct of that spelling in the family, **nested ones included**) or
  `names_one_type_in_both` (`:377-396`, an in-package lookup that is file-local for
  JVM).
- `scope_shadows` (`:350-365`) asks only whether the **callable** binds the name as a
  local; it never asks whether an enclosing *type* declares or inherits a member of that
  name.
- Receiver dispatch: `receiver_type_method` (`dispatch.rs:589-628`): for any non-Swift,
  non-Go family, if all `type_methods` hits are one identity it is returned. **There is
  no import, package or visibility test on that path.** (The abstain that tested for an
  external import was added in round 2 at `dispatch.rs:614` and reverted.) The stricter
  `type_method_target` (`implicit_receiver.rs:462-520`) is used by rung 2d, the Kotlin
  `invoke`/scope-receiver rungs and references; it corroborates a hit by *simple-name*
  presence in `import_bindings` (`:497-501`), by equal `jvm_package_by_file`
  (`:511-517`), or by a same-file hit [V].
- Bare calls: `resolve_all.rs` ladder: Kotlin invoke/scope receiver (`:272-336`),
  `local_extracted_callee` (`unresolved.rs:73`), 2a import binding (`:847-868`), 2a'
  `jvm_qualified_member` (`:876-888`), 2c `lexical_target` same-file (`:1085-1141`),
  2d type-qualified (`:1214-1290`), 2d'' `nested_type_target` (`:1334`), then the global
  rung (`:1561-1606`), vetoed only by `bare_name_has_other_evidence`
  (`candidates.rs:238-248`).

### 1.6 Python

`python_scope_lookup` (`pytypes.rs:1192-1270`) walks local, class-body and enclosing
function scopes using `python_scoped_import_bindings`. When it finds nothing,
`python_import_binding` falls back to the **flat** `import_bindings` table
(`pytypes.rs:1297-1306`). That table also receives every function-level import
(`index.rs:1000-1002`, `1154-1169`, `or_insert`), so a function with no import reads
another function's [V]. `bare_name_has_other_evidence` (`candidates.rs:238`) sees only
external/unindexed imports, not resolved function-scoped ones, so even without the flat
leak the global unique-name rung (`resolve_all.rs:1561`, `UniqueGlobal` = 0.9,
`model.rs:217`) binds the sibling's import target [V].

## 2. Failure inventory

Every shape below is pre-existing on main. "Probe" is the `#[ignore = "ft-2c800986..."]`
test (or `-` for the round-3 guards that pass today and must keep passing).

| # | Shape | Mechanism in today's code | Probes |
|---|---|---|---|
| F1 | `import ext.Repo` loses to a repo `a.Repo` (alone, or beside `import a.*`) | (a) stem-bind rung (`import_path.rs:382-389`) turns the external import into a binding to `a/Repo.kt`, which then corroborates `type_method_target` (`implicit_receiver.rs:497`); (b) the typed-receiver path has no import/package test at all (`dispatch.rs:614-628`); (c) a star import binds nothing, so the *explicit-beats-star* rank does not exist | j9, s5, s6, x3, base_star_import_after_explicit_nominal, z3_explicit_ext_import_with_star_import_of_repo_package |
| F2 | Repo class wrongly typed when an explicit repo import should win or when two same-named repo classes exist | `type_methods` row is shared by name; several hits => abstain, one hit => bind regardless of import | r6, s1, r7 (same FQN in two modules) |
| F3 | Nested `Outer.Repo` answers a top-level bare `Repo` | `unique_indexed_type` counts nested classes (`value_types.rs:10`); identity has no container chain visible at the use site | base_nested_class_nominal_factory |
| F4 | Own member `make()` is read as top-level factory `make()` | `lookup_in_package` finds only file-level decls (`dispatch.rs:197`), never asks the enclosing type chain (`value_types.rs:302-318`) | base_member_shadows_toplevel_nominal_factory |
| F5 | Inherited or companion member `make()` is invisible | enclosing-type test (round-2 `enclosing_type_declares_member`) keyed on bare type name; inherited members need `supertypes` plus identity | e1, e5 (e6 is **not** an instance: Phase 0 shows main already edges `Host.use -> Other.find`; its assertion is satisfied by file+member name, see "Phase 0 results") |
| F6 | Imports of members and aliases do not bind | `import a.Util.go` binds the object `Util`'s file then demands a *file-level* `go` (`resolve_all.rs:855`, `dispatch.rs:197`); alias `F` is not a `type_methods` key; Java caller of a Kotlin class crosses families | j2, j5, s3 (as stated); **j6, r3, e7 have a second, prior cause**: the Java extractor emits no `declared_type` for a formal parameter or for a local initialised by a call, so the receiver is untyped before any resolver rung runs (see "Phase 0 results"). `resolve_name` alone cannot fix them |
| F7 | Scala imports resolve only by path or basename | no package fact, `resolve_jvm_import` is Kotlin/Java only (`imports.rs:197`) | r4 (j1, j8 are guards) |
| F8 | Kotlin generic return types are not types | `admissible_nominal_type` refuses `<` (`receiver_types.rs:806`); lifting that without identity widens F1-F5 | the 4 ignored z3_* probes and `kg.Z3` |
| F9 | Swift extension / C# partial overloads across files look like recursion | `shared_identities` knows only same-file duplicates (`index.rs:156-170`); a cross-file sibling is invisible | ov_swift_extensions_across_files..., ov_csharp_partial_classes_across_files... |
| F10 | Python: a function without the import binds the name | flat table leak (`index.rs:1000`, `pytypes.rs:1297`) plus global rung at 0.9 (`resolve_all.rs:1561`) | py_from_import_value_function_only(+_and_global_unique_name), py_init_reexport_function_level_import, py_value_reference_function_import_other_function, p1-p5, and the other 8 ignored py_* |

Round-3 constraints and what each rules out (all guards run today unless noted):

| Guard | What it forbids | Why round 2 broke it |
|---|---|---|
| ChronosFlow `Task.toDayQuickItem`, `Habit.statusLabel` (the **x1, x2 probes are not guards today: they fail with no edge**, see "Phase 0 results") | abstaining because "an external import names a class that also exists in the repo" | the test was by simple name: an unrelated nested `UndoableAction.Task` exists. The true edge is an *extension declared on the external* `Task` (owner = the external type) |
| j1, j8 | a package-gated stem rule for Scala | `jvm_package_by_file` has no Scala rows, so every sbt multi-module import was dropped |
| p9 | a function-local import suppressing a module star-import bind in another function | rule (f) keyed on "name imported by *some* function" |
| t1, t2, t3, e3 | cross-file split types keyed on `(language, spelling)`, bare type name rows | merged distinct `Node` / `X` types and top-level functions; `type_methods` key is the bare name |
| r1, r2, r5, r8, s2, s4, s7, e2, e4 | recall that exists today | must not regress |

## 3. Proposed model

### 3.1 One owner

`Resolver::resolve_name(site, namespace) -> NameVerdict`, new module `resolver/names.rs`.
`site` is `(file, enclosing symbol, byte offset)`. `namespace` is `Type` or `Value`
(callable/property). It replaces the six seams of section 4. Per family it dispatches:
JVM (Kotlin, Java, Scala) implements the order in 3.3; Python wraps `python_scope_lookup`
plus the *module-only* table (3.6); every other family returns `Unmodelled` and keeps
today's code.

```
NameVerdict =
    Local                       // a parameter/local/closure: never a repo declaration
  | Decl(DeclId)                // one declaration, DeclId = (file, qualified_name)
  | Overloads(Vec<DeclId>)      // same qualified name, several files/overloads
  | External(Fqn)               // an import or default import naming a type outside the index
  | MaybeExternal               // an unindexed star import could supply it: abstain
  | Ambiguous(Vec<DeclId>)      // two declarations at the same rank: abstain
  | Unbound                     // no scope names it
  | Unmodelled
```

Callers consume the verdict, never a simple name: `Decl` binds, `External` is classified
`External{module}` (and may still match an extension owned by the same `Fqn`), `Ambiguous`
and `MaybeExternal` write a ledger row, `Unbound` falls to the rest of the ladder,
`Unmodelled` keeps the legacy path.

### 3.2 Qualified identity

- `Fqn = package + "." + qualified_name` (`a.Outer.Repo`). Kotlin/Java package from
  `jvm_package`; Scala from v98 (3.5). A new `jvm_decl_by_fqn: Fqn -> [DeclId]` replaces
  `jvm_package_symbols`, and records `top_level: bool` (parent is the file) so a nested
  type is never answered for a bare package-level spelling.
- **Container chain computed at index time from spans.** `ExtractedSymbol.parent_symbol`
  loses the outer for nested members (1.1), so the chain is rebuilt from containment of
  the extraction's own type symbols (the extraction is in hand during
  `index_extractions`). No extractor change; `Outer.A.Inner` vs `Outer.B.Inner` stay
  distinct.
- **Member ownership**: new `jvm_members: (Owner, member name) -> [(file, symbol)]` where
  `Owner = Decl(DeclId) | External(Fqn)`. A member's owner is the smallest enclosing type
  symbol by span. An extension function's owner is `resolve_name(receiver spelling, Type)`
  evaluated **at the extension's own declaration site**; an extension on an imported
  external type is therefore owned by `External(ext.Task)` and matches only values of that
  same external type. `type_methods` loses its JVM rows (other families unchanged).
- **Supertypes**: each heritage reference is resolved at the subtype's declaration site,
  giving `supertypes_by_decl: DeclId -> [Owner]`. An `External` or `Unbound` supertype
  that could supply a name makes the lookup `MaybeExternal` for that name, not `Unbound`.
- **Module** (r7): among several `DeclId`s with one `Fqn`, the one in the importer's own
  Gradle module (`gradle_module_dirs`, else the `.../src/<set>/<lang>/` path prefix)
  wins; otherwise `Ambiguous`.
- **Cross-language**: the JVM realm is `{kotlin, java, scala}`; `jvm_members` is not
  keyed by `LangFamily`, which is what lets a Java call reach a Kotlin class (j6) and
  deletes the hand bridge.

### 3.3 Decision procedure, JVM (first rank that answers wins)

For a simple name `N`, namespace, at site `S` in file `F`:

1. **Qualified spelling** `A.B.C`: resolve the head `A` by steps 2-8; if it resolves to a
   package prefix known to the index, walk `Fqn` segments; if it resolves to a type, walk
   nested types/members. (Subsumes `jvm_qualified_member` and `nested_type_target`.)
2. **Locals**: `LocalBinding` at `S`, `scope_locals`, local functions/classes of the
   enclosing callables. Verdict `Local`.
3. **Enclosing types, innermost to outermost** (chain by span): for each type `T`: its
   own members (including Kotlin companion members and enum entries, already owned by
   `T`) and its nested types; then inherited members through `supertypes_by_decl`, bounded
   by `HERITAGE_WALK_MAX_DEPTH` (`scope.rs:147`); then the Kotlin `scope_receivers`
   owner (`apply`/`with`). A hit is `Decl`; an unresolved or external supertype that may
   supply `N` gives `MaybeExternal`.
4. **Explicit imports of `F`**: `import p.N`, `import p.N as M`, `import p.Outer.N`,
   `import p.Util.go` (member of an object/class), Java `import static p.U.m`. The
   import's `Fqn` is looked up in `jvm_decl_by_fqn`; found is `Decl`, **not found is
   `External(fqn)` whatever file stems exist** (this is the F1 fix; the stem rung never
   participates). Two explicit imports of one simple name with different `Fqn` is
   `Ambiguous`.
5. **Same package**: top-level declarations whose `Fqn` is `package(F) + "." + N`, in
   every file of that package (module tie-break). Today's file-local `lookup_in_package`
   becomes package-wide for JVM, which also fixes the recorded LOW 7 recall
   (`z3_two_files_different_directories_same_package_recall`).
6. **Star imports**: `import p.*`, Java `p.*` and `static p.U.*`, Scala `p._`, Kotlin
   `p.Outer.*`. Candidates `Fqn(p.N)`; exactly one source with exactly one repo decl and
   **no unindexed star** gives `Decl`; a repo decl plus an unindexed star gives
   `MaybeExternal`; two repo decls give `Ambiguous` (several callables, or one class and its
   factory functions, give `Overloads`); and **no repo decl but an unindexed star** also gives
   `MaybeExternal`, before the default imports are consulted, because the library package may
   supply even a default-imported name. A non-static `p.Outer.*` imports nested types only.
7. **Default imports**: a per-language table of default-imported *type and function*
   names (`kotlin.*`, `kotlin.collections.*`, `kotlin.text.*`, `java.lang.*`, `scala._`,
   `Predef`). A name in the table is `External(fqn)`. Names not in the table are never
   assumed external.
8. `Unbound`. There is **no repo-wide "unique class of that spelling" fallback** for typed
   receivers and factory returns. (Open question Q1: whether a guarded last-resort tier is
   wanted back.) The unindexed-star rule of step 6 replaces the earlier wording of this step
   (Fable's ruling on commit 2; section 5, `base_nested_class_nominal_factory`).

Order per language, from the language specifications [I; Kotlin, Java and Scala specs to
be re-read when implementing, I have not re-fetched them in this session]: Kotlin explicit
> same package > star > default; Java single-type import and same-compilation-unit
declaration > same package > on-demand import > `java.lang`; Scala local/inherited/same-file
package members > explicit import > wildcard import > same-package members of other files
> `scala._`/`Predef`. Scala ties at equal rank are `Ambiguous`.

Namespace `Value` adds: top-level functions/properties in the same package across files;
extension functions are candidates for `x.f()` only if (a) `Owner(f) == type of x` (or a
supertype of it) and (b) `resolve_name("f", Value, F)` yields that declaration, which is
exactly "same package, explicit import by name, or star import of its package". A member
of the receiver's type always wins over an extension (Kotlin).

### 3.4 What each failing shape becomes

- Typed receiver `r: Repo`: `resolve_name("Repo", Type, S)` gives `TypeRef`
  (`Decl`/`External`). Members come from `jvm_members[owner]`, filtered by `Owner ==
  TypeRef`. External owner plus no extension of that owner: no edge, class `External`.
- Factory `val r = make()`: callee = `resolve_name("make", Value, S)`. `Decl` of a
  top-level function: read its `return_type`, resolve that text **at the callee's
  declaration site** (callee-side spelling decides), strip an outermost simple generic
  argument list (Z3), refuse `?`, `*`, `!` and unresolved names. Callee verdict `Local`,
  or `Decl` of a member (own, inherited, companion), means the *member's* return type is
  used (or abstain), never the top-level function's (F4, F5).
- Generic return types (F8) are safe only because the *identity* is now resolved: `List<String>`
  resolves to `External(kotlin.collections.List)` via the default table and the extension
  `List<String>.total()` is owned by the same `External`.

### 3.5 Facts: today vs extract-v98

Available today (no schema change): `jvm_package` (Kotlin/Java), symbol spans and
`qualified_name`, `return_type`, `LocalBinding`, `scope_locals`, `scope_receivers`,
heritage references, import `raw_import` (static/wildcard recoverable), extension
receiver spelling in `qualified_name`, `swift_module_by_file`, `gradle_module_dirs`.

Needs **extract-v98** (schema frozen at 97 per the wave rules, so this is a separate
approved step, with the cross-app consumer sweep):

- **V1 Scala package scopes.** `jvm_package` for Scala header forms (`package a.b`,
  chained `package a` / `package b` => `a.b`) and `package_scopes: Vec<(String, Span)>`
  for the block forms. Needed for the principled Scala rule; j8's nested blocks need V1
  block scopes.
- **V2 C# namespace per type** (`csharp` `namespace N`/file-scoped), the Swift module
  analogue. Needed to tell `A.X`/`B.X` (t3) from partials of one `N.X`
  (ov_csharp_partial...).
- V3 (not needed here, listed because Z5 shares the card chain): type-parameter names on
  callable and class symbols, so `fun <T> f(): T` is not read as a class `T`. Until V3,
  a type-parameter name that coincides with a repo class in the same package is a
  documented residual risk of F8.
- V4 (optional hardening): import kind flags `{is_static, is_wildcard, member}` so the
  resolver stops parsing `raw_import`.
- V5 Kotlin `typealias` target fact (j10).
- V6 Java declared types for formal parameters and call-initialised locals (r3, j6, e7).
- **V7 property and field symbols.** The extractor emits none, so `val o = object { fun anon() }`
  leaves `anon` looking like a member of its class. No fact names the object expression, so no
  label is set; the mis-ownership is the characterization row `kg.V7` (see "Commit 1 review").
- **V8 Java `record` and `@interface` symbols.** Neither emits a symbol, so their members are
  attributed to the enclosing class, or look top-level when the record is top-level. Same: no
  fact, no label; pinned by an ignored probe.

**Scala until V1 lands**: the unique-basename rung (`import_path.rs:382`) stays enabled
for `scala` only (explicit allow-list), so j1, j8 and r4 keep passing; the Scala analogue
of j9 (`import ext.Repo` beside a unique `Repo.scala`) remains a known wrong stem-bind and
gets its own `#[ignore]`d probe `j9s_scala_library_import_does_not_bind_repo_stem`. With V1
Scala joins the Kotlin rule and the allow-list entry is deleted. This is the card's
"either has facts or does not apply" criterion: it does not apply to Scala until the facts
exist, and the gap is recorded.

### 3.6 Python

One lexical table per file, as the existing scope walk already models it:

1. locals, parameters, nested defs/classes of enclosing callables;
2. class-body scope **only for reads in that body** (never in its methods);
3. enclosing function scopes;
4. **module scope**: module-level imports, module-level declarations, module-level star
   imports (`python_import_binds["*"]`, `python_star_may_define`);
5. builtins.

Changes: (i) function-level imports stop being inserted into the flat `import_bindings`
(`index.rs:1000-1002`, `1154-1169`), so the flat table is the module table; the
`python_ambiguous_scoped_imports` bookkeeping (`index.rs:1176-1192`) and the flat fallback
semantics in `pytypes.rs:1297-1306` go away; (ii) every direct reader of
`import_bindings.get(file)` for Python goes through `import_binding_at`; (iii) the global
unique-name rung (`resolve_all.rs:1561`) applies to a Python bare name only when the
verdict is `Unbound` **and** the file imports the name in no scope (a name some function
imports is the file's own claim about it). p9 holds because module-level `from helper
import *` is rank 4 and is not "imported in a function". Overlap with ft-0e6cc90f
(UniqueGlobal precision, in review): this design only decides *which scopes feed the
verdict*; whether the rung should exist for Python at all is that card's. Q4.

### 3.7 Split types (Swift extension, C# partial)

`shared_identities` (`index.rs:156-170`, read `unresolved.rs:137-162`) becomes "the
`jvm_members`-style owner table has more than one declaration of `(Owner, name)` with
different `DeclId`s". Swift owner = `(module, resolve_name(receiver spelling))` using
`swift_module_by_file`; distinct modules are distinct owners (t1), top-level functions have
no owner (t2). C# needs V2.

## 4. Migration plan

One coherent concern, five commits, each green (`cargo test -p devmap-resolve
-p devmap-analyze`) and each un-ignoring its probes. Replace, do not accumulate: a seam is
deleted in the commit that moves its last caller.

1. **Tables** (additive, no behaviour change): `jvm_decl_by_fqn` (+top_level), container
   chain by span, `jvm_members`, `supertypes_by_decl`, `jvm_imports` (explicit/star lists
   per file from `ExtractedImport`), module id, default-import table. Unit tests on the
   tables.
2. **Owner** `names.rs::resolve_name` for JVM with table-driven tests per rank. No caller
   yet.
3. **Move JVM callers** (typed receivers, factories, bare calls), deleting as each moves:
   - `dispatch.rs:589-628 receiver_type_method` (JVM arm), `resolve_all.rs:454-679` typed
     receiver rung for JVM, `receiver_types.rs:737-764` (JVM returns a `TypeRef`);
   - `value_types.rs:247-344 factory_return_type` JVM arm; delete JVM use of
     `unique_indexed_type` (`:10`) and `names_one_type_in_both` (`:377`);
   - `resolve_all.rs:272-336` (Kotlin invoke/scope receiver), `:847-868` (2a),
     `:876-888` (2a'), `:1085-1141` (2c), `:1334-1345` (2d''), global rung `:1561` gating;
   - `implicit_receiver.rs:462-520 type_method_target` JVM corroboration arms
     (`import_bindings.contains_key`, equal package) removed; Swift arms stay;
   - `candidates.rs:238 bare_name_has_other_evidence` reads the verdict for JVM and
     Python;
   - `scope.rs:53-95 bare_name_is_in_scope` JVM branch.
   **Deleted**: `jvm_qualified_member`, `nested_type_target`, `resolve_jvm_import`
   (`imports.rs:217-355`), `jvm_package_symbols`, the junk `"*"` binding for JVM
   (`index.rs:1134-1172`), and the JVM and Kotlin/Java gradle branch of the stem rung.
   `Imports` edge targets for JVM come from `jvm_decl_by_fqn` / package files, replacing
   directory expansion in `resolve_package_wildcard` (kept for Scala until V1).
4. **Python**: index flat-insert removal, reader migration list (`value_types.rs:431`,
   `references.rs:206,301,397,615`, `resolve_all.rs:1373`, `receiver_types.rs:520,659`),
   global-rung gating, deletion of `python_ambiguous_scoped_imports`.
5. **Split types**: owner-based overload sets for Swift now, C# after V2.

Net size: expected to grow by roughly the owner (~600 lines) against ~350 deleted; the
PR reports both numbers. Blast radius: `lookup_in_package` has ~25 callers across
languages; only the JVM/Python call sites listed above change. DevMap `impact` on
`type_method_target` and `lookup_in_package` returned only transitive test->`resolve_all`
rows (967 hidden), so direct callers above come from `rg`, not the graph. Recorded as a
gap, see section 6.

## 5. Fixture mapping

"Now" is **[I]** from code reading: I could not run the suite (all cargo slots were busy
for the whole session; the one queued run never started and was stopped). Phase 0 of the implementation
re-runs every ignored probe on main and records the true "now" before any change.

45 ignored probes citing ft-2c800986, with the expected result under the model:

| Probe | Expected under the model | Decided by |
|---|---|---|
| base_star_import_after_explicit_nominal | no edge to `a/Repo.kt` | `ext.Repo` explicit => `External`; star never reached |
| z3_explicit_ext_import_with_star_import_of_repo_package (not ignored; must keep) | no edge | same |
| s6, s5 | no edge | explicit external beats star / own-package |
| j9 | no edge (`import ext.Repo`, repo `a.Repo`) | explicit => `External`, stem rung removed for Kotlin |
| x3 | no edge to `other/Task.kt` | `External(ext.Task)`; no extension owned by it |
| x1, x2 | edge `build -> Task.toItem` in `F.kt` | extension owned by `External(ext.Task)` == receiver type; the lambda parameter's type comes from the existing lambda/element typing (not re-verified here) and is resolved at F |
| s1 | edge to `a/Repo.kt`, none to `b/Repo.kt`, both import orders | explicit `a.Repo` rank 4 beats star `b.*` rank 6 |
| r6 | edge to `b/Result.kt` | explicit `b.Result` selects one `DeclId` |
| r7 | edge to `m1/.../Result.kt` | same `Fqn` in two modules, importer's module wins |
| r3 | Java `import a.Task;` binds `a/Task.java` | explicit import resolves via `Fqn`; Java realm |
| r4 | Scala `a.Task` binds `a/Task.scala` | pre-V1: path-bound import via Scala-only stem allowance; after V1: package rule |
| j2 | edge `run -> Util.go` | explicit import of an object member (`Fqn` prefix `a.Util` is a type, tail is a member) |
| j5 | edge `run -> Foo.go` for `f: F` | alias import `F` => `Decl(a.Foo)` |
| j6 | Java caller reaches `Foo.go` in a `.kt` file | JVM realm, `jvm_members` not family-keyed |
| s3 | edge `Helper.help` | star import of indexed package `a` resolves `Helper`; the external *static member* import names no type |
| e7 | edge `r.find()` to `Repo.find` | `Repo r` declared type resolved by explicit import; the static `make` import is a member import |
| base_nested_class_nominal_factory | no edge | `Outer.Repo` is nested: not reachable as bare `Repo` from top level; `import ext.*` makes the name `MaybeExternal` |
| base_member_shadows_toplevel_nominal_factory | no edge | step 3 finds `C.make` before the top-level |
| e1 | no `Repo.find` (abstain or `Other`) | step 3 inherited member via `supertypes_by_decl` (Base declared once) |
| e5 | no `Repo.find` | companion member is owned by `Host`, found in step 3 |
| e6 | no `Repo.find` (Swift) | owner `Host` (same module) has `make` declared in the extension |
| z3_default_import_name_repo_declared_and_explicitly_imported_binds | edge `r.get() -> a/Result.kt` | explicit `a.Result` beats the default import; generic stripped; callee-side spelling |
| z3_same_file_local_class_list_recall | edge `total` | same-package `List` (rank 5) beats default `kotlin.collections.List` |
| z3_a_local_from_a_function_written_to_return_a_generic_list_is_a_list, z3_nested_generics_type_the_outer_constructor | edge to the extension `List.total` / `Map.total` | both `External(kotlin.collections.*)`; extension owner equal |
| ov_swift_extensions_across_files_sibling_call_not_self | no self edge | owner `T` shared across A/B in one module => overload set |
| **ov_csharp_partial_classes_across_files_sibling_call_not_self** | stays `#[ignore]` | **needs V2** (namespace); t3 must keep passing meanwhile |
| py_from_import_value_function_only, ..._and_global_unique_name | `d` binds nothing | scope walk finds nothing; name imported in `a` => global rung vetoed |
| py_init_reexport_function_level_import | `d` binds nothing | same |
| py_value_reference_function_import_other_function, py_function_only_import_read_at_module_scope_is_unbound | `d` / module scope unbound | flat leak removed |
| py_class_body_import_not_visible_in_method, py_module_level_import_inside_nested_if_in_class_at_module, py_async_def_and_method_importing, py_method_reads_function_alias_from_other_method, py_other_function_param_named_like_alias | methods do not see class-body or sibling-function bindings | scope rules 1-5 |
| py_global_decl_before_import | `other()` unbound | a `global` write makes the module binding ambiguous (`python_name_is_ambiguous`) |
| py_module_level_unresolved_import_then_function_import | `d` unbound, `a` binds | module-level external import is rank 4 in `d`; function import is `a`'s |
| p1, p2, p3, p4, p5 | p1: `b -> user.py::x` (module-level x); p2: `b -> user.py::x`; p3: `a` binds, `b` does not; p4: builtin `open` in `b`, `a` binds `other.open`; p5: `C.a` binds, `C.b` does not | rank 4 vs sibling scopes; builtin rank 5 |

Round-3 guards (pass today, must stay green):

- j1, j8: Scala stem allowance kept until V1 (3.5). Unchanged result.
- p9: module `from helper import *` binds `b -> helper.x`; `a`'s local import is a
  separate scope.
- t1, t2, t3, e3: distinct owners (module-qualified for Swift, namespace for C# after
  V2), top-level functions have no owner, and `Sub` in `q` is a different `Fqn` from
  `Sub` in `p` so e3's member does not block `p`'s factory.
- a nested class never answers an unrelated import
  (`a_nested_class_never_answers_an_unrelated_import`): nested types are reachable only
  by step 1/3/4 and never by step 5-7.
- ChronosFlow `Task.toDayQuickItem`, `Habit.statusLabel`: the extension-owned-by-external
  case, same as x1/x2.
- e2, e4, r1, r2, r5, r8, s2, s4, s7, j3, j4, j7, j10: unchanged expectations.
  `j10` (typealias in package `a`, class in `a.impl`, `import a.Foo`) binds today only by
  the stem; under the model `a.Foo` is the alias declaration, so the edge is kept by
  following `typealias` to its target when the alias is a single-name reference
  (Q2: confirm the extractor emits the alias target as a Type reference).

Tests the model changes on purpose: `z3_a_name_the_corpus_declares_twice_abstains` expects
an abstain for `List<String>` when the corpus has `q.List` and `r.List`. Under the model
`p/Z.kt`'s `List` is `External(kotlin.collections.List)` (not in p, not imported), the
extension is owned by the same external type, so the edge is **true**. That test encodes
a conservative expectation from before identity existed. I propose changing it to assert
the edge and adding the variant where `p` declares or star-imports `q`, which must abstain
(Q3). No other existing assertion is weakened.

## 6. Verification plan

1. **Probes**: all 45 un-ignored except `ov_csharp_...` (V2); every guard above green;
   `kg.Z3` row removed from `rust/testdata/accuracy/known_failing.tsv` with
   `ratchet.json` counts moved in the same commit (`accuracy_fixtures.rs` fails otherwise);
   the Scala-stem probe `j9s` added ignored with its reason.
2. **Phase 0 baseline**: run the 45 ignored probes plus the guards on main and record
   which actually fail today (section 5's "now" is unverified).
3. **Corpus diff vs main** on Curio, Meridian, ChronosFlow, Graft, DevType, MarkDev
   (checkouts under `/Users/bharath/Code/apps` and `/Users/bharath/Code/devtools`).
   Measured language mix (file counts, `rg --files`, 2026-10-21): Kotlin
   ChronosFlow 1020, Graft 171, Curio 170, Meridian 126; Swift DevType 717, MarkDev 234,
   Curio 202, Graft 200, ChronosFlow 205, Meridian 115; Python <= 13 files each; **Java 0,
   Scala 0**. Method: copy each checkout at its recorded HEAD sha (ChronosFlow and DevType
   are pinned in `rust/testdata/accuracy/corpora.json` and refuse a dirty tree; the other
   four are not pinned, so record their shas in the report), `devmap build` with the main
   binary and with the branch binary, dump `edges` rows
   (source, target, kind, resolution, confidence) from both stores, diff. Every removed
   edge is classified true/wrong/unknowable by reading both ends; the gate is **zero
   removed true edges**; every added edge is classified too. Real-corpus runner:
   `devmap-analyze/tests/real_corpus_accuracy.rs` with `DEVMAP_CORPUS_<NAME>`.
4. **Gap, stated**: none of the six corpora has Java or Scala and Python is negligible, so
   the corpus diff cannot validate F6-F7 or the Python half. Coverage there is the
   fixtures only; I propose adding a small Java and a small Scala accuracy fixture (labelled
   before running, per the fixtures README) and measuring Python on `Lappi-decision` and
   `scholarlm` (in `corpora.json`).
5. **Bench budget** (no committed resolver bench exists today; `rust/devmap-query/examples/
   query_bench.rs` is a query bench): commit `rust/devmap-resolve/examples/resolve_bench.rs`
   that times `index_extractions` + `resolve_all` on pre-extracted corpora; interleaved A/B
   vs main, min of 7, same machine, idle load (corpus runs drop files above load, per the
   5 s parse guard). Budget on ChronosFlow (1020 `.kt`): resolve wall <= +5%, index wall
   <= +3%, peak RSS <= +5%. Structural bounds: `resolve_name` memoised on `(file, scope,
   name, namespace)`, steps 3 and 6 bounded by chain depth, `HERITAGE_WALK_MAX_DEPTH` and
   the number of star imports; no per-call corpus scan.
6. **DevMap gaps recorded**: the worktree had no store, built with `devmap build
   --manifest` (1866 files, 23714 symbols). `devmap impact` for `type_method_target` and
   `lookup_in_package` returned transitive test rows through `resolve_all` and no
   direct-caller rows (hidden 967); callers were taken by `rg` and are labelled as such.
   Fixture indexing through a nested scratch directory indexed 1 file, so probes were not
   reproducible with the installed binary; they would be run through the test harness.
   `gitpulse_insights` was not called (design-only task, no files shared with other
   worktrees).

## 7. Open questions for the reviewer

- **Q1 Repo-wide fallback.** The model removes the "unique class of that spelling in the
  repo" fallback for typed receivers and factories on JVM. If the corpus diff shows lost
  true edges (generated sources, unmodelled `typealias`, Android `R`), do we add a
  guarded last tier (top-level only, no unindexed stars, no explicit import of the name,
  `Ambiguous` never) or accept the recall loss?
- **Q2 Typealias.** Does the Kotlin extractor emit the alias target as a Type reference
  (needed for `j10` and `typealias Foo = a.impl.Foo`)? If not, V5 (alias target fact) or
  abstain.
- **Q3 `z3_a_name_the_corpus_declares_twice_abstains`.** Invert it (the model's edge is
  correct), or keep the abstain by refusing `External` default-import owners on a name the
  repo also declares? I recommend inverting.
- **Q4 Python global rung.** Keep `UniqueGlobal` for Python bare names that no scope binds
  (this card), or retire it for Python (ft-0e6cc90f)? The design is compatible with both.
- **Q5 Module rule** (r7): derive module from `gradle_module_dirs`, else
  `.../src/<set>/<lang>/` path prefix. A path heuristic; acceptable, or require a Gradle
  parse of `project(":x")` dependencies (a new fact)?
- **Q6 Schema timing.** V1 and V2 force extract-v98 and the cross-app sweep
  (`check:schema-parity`, GitPulse vendor, Manvi/Jarvis pins). Land the resolver model
  first against v97 (Scala on the stem allowance, C# partial probe stays ignored), then
  v98 as its own approved step? I recommend yes.
- **Q7 Default-import table ownership.** The table (Kotlin/Java/Scala default imports,
  including types) is new data; it belongs in `builtins.rs` beside `KOTLIN_BUILTINS`. Its
  completeness decides how often `Unbound` is returned for a stdlib type; incomplete
  entries abstain rather than bind, so errors are recall-only.
- **Q8 Language policy.** Rust, in the existing crate; no new dependency.

## Review ruling (Fable, 2026-10-21): approved with changes

1. Phase 0 is a gate, not a step. Every probe whose "now" column proves wrong gets its F-row re-justified before commit 1.
2. Commit 3 is split into three green commits: typed receivers, factories, bare-call ladder. A seam dies with its last caller, never earlier.
3. The resolver bench (`resolve_bench.rs`) lands in commit 1, so the A/B baseline predates the behaviour changes.
4. The Java and Scala accuracy fixtures land before commit 3 merges. The corpus has neither language, so the fixtures are the only F6/F7 evidence.
5. Q2: if Phase 0 shows no typealias target fact, j10 becomes an ignored probe citing V5, with no alias stem rule.

Open questions:

- Q1: no fallback up front. If the corpus diff loses a non-generated true edge, bring the case, not a tier.
- Q3: invert the test, with a star-import variant that must abstain.
- Q4: keep the rung with the stated gating; retiring it is ft-0e6cc90f's call.
- Q5: path heuristic, labelled; no derivable module means Ambiguous.
- Q6: v97 first; v98 is its own approved step after the consumer sweep.

## Phase 0 results (2026-10-21, main 99d41a1f, implementer run)

Run with `cargo test -p devmap-resolve --test it -- --ignored` and the named guards; the CLI
guard through `-p devmap-cli --test it`. Section 5's "now" was [I]; this section is [V].

- All 45 `ft-2c800986` probes **fail** on main (none passes). 39 fail for the mechanism the
  F-row names. Three groups were mis-described:
  - **e6 (Swift) is a false probe, not a failure shape.** Main's edges are
    `Host.use -> Host.make` and `Host.use -> A.swift::Other.find`: correct. The assertion
    `!to_file(.., "A.swift::Repo.find")` is satisfied by *file path + member name* (`to_file`,
    `attack_resolver_bundle5.rs:40`), so `Other.find` in the same file matches. Under the model
    the edge stays; the probe needs a symbol-exact assertion. e6 leaves F5.
  - **r3, j6, e7 (Java receivers) fail before the resolver.** The stored extraction of
    `int run(Task t) { return t.find(); }` has a `Type` reference `Task` with no `assigned_to`, so
    `local_bindings` for `t` has no `declared_type` (scratch run with the installed `devmap 0.2.5`,
    build aacffb852c4f; main's `r3` also returns `[]`). A Java local initialised by `new Repo()`
    types; one initialised by `make()` (e7) or a parameter (r3, j6) does not. The cause is an
    extractor fact (Java declared types), not F6's import or family handling. Un-ignoring them needs
    a new fact, named **V6: Java declared type for formal parameters and call-initialised locals**.
    j6 additionally needs the family bridge F6 describes. Root-cause code location is [I].
  - **x1, x2 fail today (`[]`), contrary to the guard table.** Two identities named `Task.toItem`
    (the repo member and the file-private extension) make `receiver_type_method` abstain [I]. The
    ChronosFlow guards pass on the real corpus only because no unrelated class there has a member
    of that name; x1/x2 are probes of the model, not regression guards.
- Guards that pass today: j1, j8, p9, t1, t2, t3, e3, j3, j4, j7,
  `z3_explicit_ext_import_with_star_import_of_repo_package`,
  `z3_a_name_the_corpus_declares_twice_abstains` (Q3 inverts it) and the CLI test
  `a_nested_class_never_answers_an_unrelated_import`. j10 also passes today (by the stem).
- **Q2: no typealias target fact.** A Kotlin `typealias Foo = a.impl.Foo` extracts no symbol and
  no `Type` reference: its identifiers appear only as `Name` references, and no edge leaves
  `Aliases.kt` (scratch build, installed binary; the Kotlin extractor has no `type_alias` arm).
  Per the ruling, j10 becomes an ignored probe citing V5, with no alias stem rule.
- Surprises confirmed by reading: `receiver_type_method` (`dispatch.rs:589-628`) has no import,
  package or visibility test for non-Swift, non-Go families; a JVM star import is bound under the
  local name `"*"` (`import_local_name` takes `rsplit('.')`, `scope.rs:4-27`, bound at
  `index.rs:1134-1136`); Java is `LangFamily::CStyle` (`model.rs:70`).
- The `to_file` matcher is file-plus-member-name based. Every probe that asserts *absence* with it
  should be re-read for the e6 trap before it is un-ignored.

## Phase 0 ruling (Fable, 2026-10-21)

1. **r3, j6, e7 stay `#[ignore]`, citing V6** (Java declared-type facts, extract-v98). The Java
   fixture is the v98 proof, not a v97 gate: no Java F-row is claimed fixed by the v97 resolver
   model, and F6 is scoped to Kotlin (j2, j5) and Java static members (s3).
2. **e6** is a guard that passes today, with an exact-id assertion. Every `attack_resolver_bundle*`
   and `kotlin_written_types` assertion that matched by prefix, `contains`, `ends_with` or
   file-plus-member-name now compares whole symbol ids; the only remaining non-exact helper is
   `reaches_file`, used solely to assert a file is *not* reached.
3. **x1, x2 are model targets, not guards.** The ChronosFlow shape is covered by two fixtures that
   pass today and must keep passing: `x4` (extension on an imported external `Task` beside an
   unrelated repo class `Widget` with a same-named member) and `x5` (beside a nested repo
   `UndoableAction.Task`).
4. **j10** is `#[ignore]`d citing V5; there is no alias stem rule.
5. **Commit 1** carries the bench, the tables and the `resolve_name` seam, and changes no edge:
   the corpus edge set must be byte-identical to main.

## Commit 1 review: the adversarial pass and what the tables now do

**Ruling (Fable, option D): a label is a ledger entry, never a verdict.** A row whose placement
rests on a fact that can be wrong carries a `Doubt` label so the unresolved-ledger can say why.
The label records; it does not decide. A member of a labelled owner is returned from both tables
exactly like any other and binds as `Decl` at today's confidence. **`resolve_name` must never turn
a label into `Ambiguous`**: `Ambiguous` is for two declarations that really tie at one rank.
This is encoded in the types (`JvmDecl::ledger_label` is the only reader of the label, the
`placement` field is not public, the `NameVerdict::Ambiguous` doc says so, and a test pins that a
labelled row is returned like any other), so commit 2 cannot get it wrong by omission.

A label is attached only where a *fact* names the doubt:

- `PartialParse`: a symbol at or after the first parse-error range. A fact (the parse outcome).
- `UnparsedFile`: any symbol of a file that was not parsed. A fact.
- A label is inherited down the chain: the chain above a member is only as good as its owner's.
- There is no label for class-level initializer content. A supertype constructor call or a
  `by lazy` is no evidence of an object expression, and a typed declaration is none either; those
  triggers are gone. No reference or extraction fact names an `object` expression, so
  `OwnerBodyHasGaps` does not exist until V7 supplies a fact.

What the tables do (`jvm_tables.rs`, devmap-resolve only, no extractor change):

1. **Identity.** A `DeclId` is `(file, container chain + name)` rebuilt from spans, not the
   extractor's qualified name, which keeps only the direct parent (`X.A.Inner.f` and
   `Y.A.Inner.f` are both `Inner.f`). `JvmDecl::symbol` keeps the extractor's id for edges.
   Members are keyed by the owner's row, so those two have distinct owners and members.
2. **Local bodies.** A symbol whose innermost enclosing symbol is a callable is local and in
   neither table. (Commit 1 also excluded a symbol inside a class-level call or constructor
   expression, such as a Java anonymous class in a field initializer. That exclusion had no fact
   behind it before V7/V8 and cost a third of the table build, so it was removed in the follow-up
   to commit 2: the anonymous class's method is now taken for a member of its host, pinned by
   the ignored probe `v8_a_java_anonymous_class_method_is_not_a_member_of_its_host`.)
3. **Known mis-ownership, characterized, no label (V7, V8).**
   - **V7 property and field symbols.** `val o = object { fun f() }` in a plain class leaves `f`
     looking like a member of the class. Fixture row `kotlin-gaps/kg.V7` (`ZqOwners.kt`), a
     `false_edge` in `known_failing.tsv` owned by ft-2c800986 and blocked on extract-v98: the bare
     call `zqOwnerName()` in the class body binds to the object expression's member, not the
     top-level function.
   - **V8 Java `record` and `@interface` symbols.** A record emits no symbol, so its method looks
     like a member of the enclosing class. The harness scores no Java language gate (a `java` row is
     refused there), so this is the ignored probe `v8_a_java_record_method_is_not_a_member_of_its_host`
     in `attack_resolver_bundle5.rs`, citing V8 and extract-v98.
4. **Extension receivers.** The extractor keeps only the last dotted segment of a receiver, with
   generics stripped, and nothing for a nullable or function-type receiver. `Receiver::Last(name)`
   is a visible receiver (a member extension keeps both: its owner is the dispatch receiver),
   `Receiver::Unseen` means no marker is visible, which is not a promise there is none, and
   `NotExtension` is only for non-callables and Java.
5. **Twins.** Identical `(file, path)` declarations are one row with `copies > 1`: a conditional
   twin for a type, an overload set of one id for a callable.
6. **Headerless files.** `Package` is a validated type (non-empty segments), so an FQN cannot start
   with a dot.
7. **Scale.** Nesting beyond 64 is not indexed and the package is recorded. The container-chain
   rebuild is one sort (skipped when the extractor's order already is outermost-first, which it
   is) plus one stack walk, with no per-symbol rescan.

**Migration notes.** `jvm_package_symbols` still has callers and stays until commit 3.
`resolve_name` (commit 2) reads `Decl` rows without regard to a label and may attach the label
to a ledger row. `jvm_package_is_unsure` is a ledger fact too: it explains an unresolved row and
never changes a verdict.

## Commit 2: `resolve_name` for Kotlin and Java (no caller wired)

`names.rs::resolve_name` applies ranks 1-8 of section 3.3, memoised on
`(file, innermost enclosing type, name, namespace)`; locals depend on the offset and are answered
before the memo. Nothing in `resolve_all` calls it, so no edge changes. Per-rank unit tests and the
adjacent-pair precedence tests are in `jvm_names_tests.rs`; every probe the design maps to
`resolve_name` is exercised there as a verdict, not an edge (r3, r4, r6, r7, j2, j4, j5, j6, s1, s3,
s5, s6, e1, e3, e4, e5, e7, x1/x2/x3, z3, base_nested, and the label rule).

What the ranks read, built in `index_jvm_decls`: `jvm_imports` (explicit and star lists, from the
extractor's specifier and alias, and from the source text for a Java `import static`, whose member
the specifier drops), the type rows per file, the heritage references per type, the functions and
classes local to a callable, and the known packages. The default-import tables are in
`builtins.rs` (`default_import`), and the Scala stem allow-list is `importpath::stem_rung_allowed`
(not consulted yet: wiring it is what removes the rung for Kotlin and Java).

Decisions taken that the design left open, and one deviation:

- **Rank 8 and an unindexed star.** Section 3.3 says a name no rank names is `Unbound`; section 5
  (`base_nested_class_nominal_factory`) says `import ext.*` makes the name `MaybeExternal`. The
  second wins: a name nothing names, in a file that star-imports a source the index declares
  nothing under, is `MaybeExternal`, because that star could supply it. Without such a star it is
  `Unbound`.
- **Inherited members** resolve each supertype at the subtype's declaration site (the scope outside
  its own body), bounded by `HERITAGE_WALK_MAX_DEPTH`, with a visited-by-depth cycle guard. An
  unresolved, external or too-deep supertype that no repository supertype beats makes the name
  `MaybeExternal`.
- **Kotlin `scope_receivers`** (`apply { }`, `with(x) { }`) are not modelled: the receiver is a
  written *value* expression whose type this module cannot name. The Kotlin receiver rungs that
  already type it run before the ladder and are not moved by this commit. A closure's captured
  outer locals are seen only where the extractor's parent chain reaches that callable.
- **Scala** and a file with no package header answer `Unmodelled` (no package fact until V1).
- **Labels never decide**: a labelled row binds like any other (`a_ledger_label_never_changes_a_verdict`).
- **Default imports** follow the Kotlin docs' list (`kotlin.*`, `kotlin.annotation`,
  `kotlin.collections`, `kotlin.comparisons`, `kotlin.io`, `kotlin.ranges`, `kotlin.sequences`,
  `kotlin.text`, `kotlin.math`, and on the JVM `java.lang.*` and `kotlin.jvm.*`;
  <https://kotlinlang.org/docs/packages.html>). The page does not state how a default import ranks
  against explicit, same-package and star imports; the order used (explicit, same package, star,
  default) is the design's [I] reading of the specification and is unverified here. The tables list
  commonly written names only (Q7): a name missing from them is `Unbound`, never assumed external.
- **Same-package tie** between declarations of one name in several files: callables are an
  `Overloads` set, types go to the Gradle-module tie-break (`jvm_module_of`: an included module
  directory, else the prefix before `src/<set>/{kotlin,java}`), and an underivable module is
  `Ambiguous` (Q5).
- **typealias (V5)**: `import a.Foo` where `a.Foo` is a typealias is `External("a.Foo")` today, since
  no symbol exists for it; j10 stays ignored citing V5.

## Commit 2 review: the adversarial round on `resolve_name`

The governing rule: **`resolve_name` never returns a confident `Decl` or `External` that could be
wrong.** Where facts are missing the verdict is `MaybeExternal` or `Ambiguous`. 50 probes
(`jvm_names_attack.rs`) were attacked at commit 2; all pass or are ignored below with a named
V-fact and a sibling test pinning the non-confident verdict that stands in until the fact exists.

- **Qualified spellings** are resolved by their head with the unindexed-star fallback off, so a
  library star (`import java.util.*`) cannot hide that `a.b.Repo` is a package of this repository;
  a path nothing in the repository names is `External`, and one with a repository package or
  declaration as a prefix is `MaybeExternal` (a member or nested type the extractor did not emit).
- **Inherited members**: an external or unresolved supertype may declare any member name, so a
  value-position hit beside one is `MaybeExternal`; a type-position name is not blanked by it.
  Every supertype is visited once (a diamond costs its size), to depth 64.
- **Explicit imports** whose path the index does not declare reach the member through the longest
  prefix that is a type (companion members, Java fields, members inherited through a subclass), else
  are `External` only when no prefix is a repository package or declaration (V7).
- **Header scope.** A name in a type's header (its supertype list and a supertype constructor call's
  arguments) resolves outside the type's body. `Heritage::header_end` is where the supertype
  references and the matching constructor reference end.
- **Overloads**: callables across files and star imports, and a class with factory functions of its
  name in the value namespace, are `Overloads`; a type-position use gets the class. A type and a
  callable of one name in one file have distinct ids (the callable's ends in `()`).
- **Private** (Kotlin, not exported by the extractor) top-level declarations are visible only from
  their own file. A renamed import hides the original from a star import. A Java non-static
  `import a.Outer.*` brings in nested types only. Java `import static` is parsed token by token
  (whitespace, newlines and comments between the words); backticks are unquoted in imports and
  package names.
- **Calls.** `NameSite::call` says the name is a callee: a Java variable never shadows a method name
  at a call. Elsewhere a variable binding is `Local` only when nothing else could be meant.
- **Ambiguity** with fewer than two ids is not a tie: it is `MaybeExternal`. `maxOf`/`minOf` are
  `kotlin.comparisons`.

New facts the model needs, none of which the extractor emits at v97:

- **V3 type parameters.** (Superseded by round 2: the one-letter heuristic that stood here was
  deleted; V3 shapes are ignored probes awaiting the type-parameter fact.)
- **V9 `inner` / `static` modifiers.** A member of an outer type found from inside a nested one is
  `MaybeExternal`: it is visible to an inner class and a companion's or static member to both.
  Ignored probes `a01`, `a01b`; pinned by `a01_v9_...`.
- **V12 extents of local bindings.** Ignored probe `a17e`; pinned by `a17e_v12_...`.
- **V13 the Kotlin grammar drops declarations after a parse error** (and some one-line class
  bodies parse Partial: the `a07`, `a10`, `a20` probes were reformatted multi-line, since the
  classes they test were never extracted). Ignored probe `a21b`; the label records the doubt and
  the verdict stays `Unbound`.

## Round 2 of the adversarial pass, and the pause (Fable, 2026-10-22)

The second attack on `resolve_name` (`jvm_names_attack2.rs`, 8ee22c07) found that most of what it
could still break needs a fact the extractor does not emit, not a better rule. Fable's ruling (option
B): the lane pauses at commit 2 until extract-v98 supplies those facts, and does not merge.

What this round changed:

- **The one-letter type-parameter heuristic is deleted.** It downgraded a real class named `C` or
  `I1` to `MaybeExternal` on no evidence. A type name inside a generic declaration that may be a
  type parameter is now a V3 ignored probe, not a guess from the spelling.
- **Two real bugs fixed.** An explicit import of a private class of another file is `MaybeExternal`,
  not the lower-ranked name of the same spelling (`v02b`). A member of a *local class* is no name of
  the enclosing callable (`m01`, `s01`), and a value name written inside a local class body is
  `MaybeExternal`, since its members and supertypes are in no table.
- **One probe was the attacker's mistake** (`r06` imported `a.B` but declared `Bee`) and is fixed.
- **Every other failing probe is `#[ignore = "extract-v98 JVM declaration facts: <V-id>: <shape>"]`.**
  Nothing was deleted. They are the v98 acceptance suite: run them with
  `cargo test -p devmap-resolve --lib jvm_names_attack -- --ignored`; all 45 fail today, and each
  must pass when its fact lands. Where a stand-in verdict exists, a sibling test pins the
  non-confident answer (`a01_v9_...`, `a17e_v12_...`); the verdict never becomes a confident wrong
  `Decl` or `External`.

## v98 JVM declaration facts

What the extractor must emit for the tables and `resolve_name` to answer without abstaining, and the
probes that wait on each. "Probes" are `#[ignore]`d tests that fail today.

| Fact | What it is | Probes waiting |
|---|---|---|
| V1 | Scala package scopes (`package a.b`, nested blocks) | `r4`, `j8` guards in `attack_resolver_bundle5` stay on the stem allow-list until it lands |
| V2 | C# namespace per type | `ov_csharp_partial_classes_across_files_sibling_call_not_self` |
| V3 | Type-parameter names on classes, methods and typealiases | 15: `a02`, `a02b`, `a02c`, `d02`, `t01`, `t01j`, `t05`, `t06`, `t07`, `t08`, `t08b`, `t09`, `t10`, `t11`, `t12` |
| V4 | Import kind flags (`static`, `*`, member) so the resolver stops parsing text | none (hardening) |
| V5 | Kotlin `typealias` target fact | `j10` |
| V6 | Java declared types for parameters and call-initialised locals | `r3`, `j6`, `e7` |
| V7 | Property and field symbols (Kotlin properties, Java fields, enum entries, companion constants) | 9: `c02b`, `c04`, `c05`, `c09`, `c14`, `c15`, `m03`, `o05`, `i02`; fixture row `kotlin-gaps/kg.V7` |
| V8 | Java `record`, `@interface` and anonymous-class symbols | `v8_a_java_record_method_is_not_a_member_of_its_host`, `v8_a_java_anonymous_class_method_is_not_a_member_of_its_host` |
| V9 | `inner` (Kotlin) and `static` (Java) modifiers on nested types | 2: `a01`, `a01b` |
| V10 | Companion object symbols, named or not, with their own span | 3: `c06`, `c06b`, `i03b` |
| V11 | Class header and body spans (supertype list, primary constructor, annotation arguments, `where` clauses, type-parameter bounds, delegation) | 9: `d01`, `h01`, `h03`, `h03b`, `h03c`, `h05`, `h07`, `h08`, `h08b` |
| V12 | Block and lambda extents of local bindings | 1: `a17e` |
| V13 | A parse-drop flag: which declarations the extractor lost to a grammar error (Kotlin drops the declarations after one, and some one-line class bodies parse Partial) | 3: `a21b`, `p01`, `p02` |
| V14 | Visibility modifiers (`internal`, package-private, `protected`) | 3: `v03`, `v04`, `v05` |

Ignored-probe counts per V-id across `jvm_names_attack{,2}.rs`: V3 15, V7 9, V9 2, V10 3, V11 9,
V12 1, V13 3, V14 3: 45 in all. V1, V2, V5, V6 and V8 are older ignored probes in
`attack_resolver_bundle3.rs` and `attack_resolver_bundle5.rs`.
