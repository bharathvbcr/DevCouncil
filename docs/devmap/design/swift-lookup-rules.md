# Swift lookup rules for the DevMap resolver (design note, normative)

Status: design only. No code, no build. Base: main `782b9f16`, store generation 4634.
Accepted by Fable ruling 75 (2026-10-28) with three amendments: (i) [U] rows are not binding rules, so a brief that relies on one carries its own swiftc probe; (ii) the §4 single-verdict shape is gated on F1, so a lane may implement the `Bound` arm alone only with the waiver named (ruling 63 is the first); (iii) this file is the committed copy. Rulings 66–74 answer §9: Q1, Q3 and Q9 are owner decisions; Q2 waits for F1; Q4 is a capped Dispatch fan-out after F5; Q5 is import-only, gated on F3; Q6 keeps today's order; Q7 is a role flag in F1; Q8 is lane M's red step.
Ruling 55 makes this note a prerequisite. Every Swift resolver brief cites it by section number, and a lane that departs from a rule names the rule and the ruling that waives it.

**Labels.** [V] means verified this session by reading code, a branch ref, a card or evidence. [I] means inferred from code read this session, not run. [U] means unverified. [R] means swiftc behaviour recorded by an earlier lane; it was not re-run here, so it counts as inferred-from-record.

**How the evidence was gathered.**
- **DevMap.** The DevMap CLI located symbols (`devmap search`, truncated=false). It cannot answer for struct fields or constants (`type_methods`, `symbol_parents`, `supertypes`, `HERITAGE_WALK_MAX_DEPTH`): it returned 0 items for each. Gap `swift-lookup-note-fields` is recorded, and those fields were read from `rust/devmap-resolve/src/resolver.rs` directly.
- **Rung order.** Read from source, `resolve_all.rs`. DevMap has no rung-order query.
- **Swift semantics.** The access-level definitions are [V] against TSPL source (`swiftlang/swift-book`, `TSPL.docc/LanguageGuide/AccessControl.md`, fetched 2026-10-28). TSPL has **no normative text on unqualified-lookup order**. Every lookup-order claim below is therefore either [R], from the lane J attacker's swiftc runs (header at `6b280ead:rust/devmap-resolve/tests/swift_resolver_residue.rs:805-816`; brief J §3), or [U].

**Path note.** `swift_modules.rs` is a **test** file, `rust/devmap-resolve/tests/swift_modules.rs`. There is no `resolver/swift_modules.rs` [V]. Cards citing "`swift_modules.rs:282`" mean the test `a_type_shared_method_call_binds_the_type_as_receiver`.

---

## 0. Global invariants (apply to every rule)

- **I1.** A wrong edge is worse than an abstention. Where a rule cannot decide, it abstains with a reason (`Resolution::Unresolved`). It never falls through to a later rung that binds by spelling.
- **I2.** **Abstention propagates downward.**
  - Once a rung establishes that a visible member of the base name exists in the caller's member chain (§4 step 1), no later rung may bind a free function, an import, or a global of that name. That holds even when the member rung cannot name one target.
  - The converse is equally binding. A member that is invisible (§2) or owned by another type (§1) establishes **nothing**. It must not block the free function.
  - Lane J was parked for breaking this converse (fs1–fs9).
- **I3.** Every lookup on `type_methods` or `supertypes` must pass the owner-identity test (§1) and the visibility test (§2). Today neither test is applied uniformly; the gaps are listed in each section.

## 1. Owner identity

**Rule.** A Swift member's owner is `(module, nested type path)`. Examples: `(M, "A.C")` and `(M, "C")` are different owners, and so are `(M1, "C")` and `(M2, "C")`. The bare type name is never an identity. It is a lookup key only, and every hit under it is filtered by identity.

- **Binds** a member only when its owner identity equals one of these:
  - the caller's enclosing type;
  - an outer type of it (§4 step 1b);
  - a supertype reached through a heritage entry that resolves to one identity.
- **Abstains** when the caller's own identity, or any heritage entry on the walk, cannot be resolved to one identity, because two declarations share the spelling at the same visibility.
- **Never binds** a member, or a member-shadow, from a namesake owner: a different nesting path or a different module.

**Extensions.** An `extension X` member belongs to the type that `X` resolves to, from the extension's file and module. `extension A.C` names `(module, "A.C")`. An extension of a type from another module, `extension Foundation.Date`, owns members of that type and never of a same-named local type.

**Nested types.** For `struct O { struct C {} }`, the identity of `C` is `O.C`. Members of `O`, including its extensions and supertypes, are in scope for unqualified lookup inside `O.C` (§4 step 1b).

**Generic owners.** For `struct Box<T>`, `extension Box`, `extension Box where T == Int` and `extension Box<Int>`, the owner is `Box`. A constrained extension is not proven to apply at the call site, so its members are **shadow-relevant but never bind targets**. They count for I2 (block the free function) but are not bound. [R]: brief J §3 records that swiftc treats a constrained-extension member as a shadow.

**Typealias-named owners.** A type written through an alias (`class C: B` with `typealias B = Base`, or `extension Alias`) resolves through the alias to its target's identity. When the alias target is not known, because there are no facts, the owner is unidentifiable and the rule abstains (§4 I2 still applies if any candidate declares the name).

### What the code does today

| Fact | Where | Keyed by | Label |
|---|---|---|---|
| `type_methods` | `resolver.rs:395`, built at `index.rs:184-201` | `(family, bare owner tail, member)`. The tail is the last segment of `parent_symbol` after `::` | [V] |
| `supertypes` / `subtypes` | `resolver.rs:411,455`, built at `index.rs:1254-1329` | bare subtype → bare supertype. A qualified base `Outer.Base` is reduced to `Base` at `index.rs:1281-1288` | [V] |
| `symbol_parents` | `resolver.rs:579` | `(file, qualified name)` → parent identity | [V] |
| `type_name_is_identifiable` | `scope.rs:388-407` | refuses a bare name that two files declare as a type. **Module-blind**: a namesake in another SwiftPM target also refuses | [V] |
| Owner of a Method | `langdecl/swift.rs:86-87`, `enclosing_owner` (`langdecl/mod.rs:249-265`) | the **nearest** owner's bare name: for `struct A { struct C { func mk() } }` the qualified name is `C.mk`. For an extension it is the written name text (`extension A.C` gives `A.C`, `extension Box<Int>` gives `Box<Int>`; `langdecl/swift.rs:380-383`) | [V] code; [I] the exact `Box<Int>` key |
| Owner of an enum case or nested enum | `langdecl/swift.rs:84-85`, `enclosing_owner_path` (`mod.rs:225-241`) | the full dotted path (`A.E.case`) | [V] |
| Owner of a property | `langdecl/swift.rs:82-83` | bare, like a Method | [V] |
| Module of a file | `dispatch.rs:11-51` `swift_modules_of`, stored in `swift_module_by_file` (`index.rs:422-431`) | `Package.swift` targets (deepest claim wins), else every `project.yml` target listing the folder, else the path heuristic (`languages.rs:1150-1177`). `project.pbxproj` is not read (`swifttargets.rs:17-19`) | [V] |
| Module in any member key | none | `type_methods`, `supertypes` and `symbol_parents` carry no module | [V] |
| In-file nested type paths | `swiftscope.rs:79-105` (`FileIndex.declared`: short name → qualified names) | extractor-internal and **not emitted** | [V] |

**Consequences.**
- [I] A member in the body of `A.C` keys `(Swift, "C", mk)`, while the same member in `extension A.C` keys `(Swift, "A.C", mk)`. Neither key carries the module. This asymmetry, together with bare-name `supertypes`, is the root of fs1, fs2, fs3, fs5 and fs9 once a walk stops filtering by identity.
- Lane J's `swift_member_declared_in_scope` (`22cbc421:receiver_types.rs` +891..979) probes both the dotted and the bare key with `contains_key`. [I] from the diff: that is how a namesake owner became a shadow.

### Missing extractor facts

These are extractor-output changes. Per `extract-v98.md` ground rule 1, they are not new fields on `ExtractedSymbol`. They form one side table on `Extraction`, keyed `(qualified_name, span.start_byte)`, the shape `jvm_decls` uses.

| # | Fact | Unblocks |
|---|---|---|
| F1 | `swift_decls`: for each Method, property, enum case and type: `owner_path` (full nesting as written, extension target included), `access` (own modifier: `private`/`fileprivate`/`internal`/`package`/`public`/`open`/none), `is_static`, `role` (`method`/`enum_case`/`property`/`type`), `in_extension`, `extension_constrained` (has a `where` clause or generic arguments), `conditional` (under `#if`) | §1, §2, §3; lane J resume (a)–(c); O1c outer-static; Y1 guards |
| F2 | `swift_alias_decls {name, holder_path, target, access}`. This already exists on parked `lane/swift-init-shape` (`9fa3d7fd`, per card ft-adc4db2e) | ms15 (typealias superclass), ft-e3c6ff8f, ft-adc4db2e |
| F3 | Import attributes on `ExtractedImport`: `testable`. `ExtractedImport` (`model.rs:877-903`) has no such field [V], and no `ImportKind` exists in `rust/devmap-extract/src` (`rg -uu`) [V], although `extract-v98.md` V4 plans one | §2 module boundary |
| F4 | A construction-receiver initializer shape for `let x = T(..).f()` (§6) | ft-4ca45b19 resume 1 |
| F5 | The existential Type ref for `any P` / `some P` (§6) | ft-9b28adab item 3, only if fan-out is chosen (Q4) |

Whether F1–F5 ride the open 98 or wait for 99 is the owner's push decision (Fable 45; ft-e3c6ff8f). See Q1. **Heritage paths need an extractor change (corrected by Fable ruling 132, Wave 10).** This note first said the reference name carries the written spelling, so keeping `Outer.Base` whole would be a resolver-only change. That is wrong. The extractor cuts a qualified supertype such as `N.Base` to `Base` at `heritage.rs:213-215`, because `user_type` is terminal at `:268`. The resolver cuts it again at `index.rs:1281-1288`. The qualifier never reaches the resolver, so carrying it is an extract-payload change inside the open 98 bundle, gated on Q1. Lane M-resume parked on exactly this. Its attack tests msr_03/04 cover `class C: N.Base` beside a same-file local `Base`.

## 2. Access control

**Swift definitions** [V, TSPL AccessControl.md]:
- "A module is a single unit of code distribution."
- open/public: usable "within any source file from their defining module" and from importing modules. Only `open` allows subclassing or overriding outside the module.
- internal: usable "within any source file from their defining module".
- fileprivate: "restricts the use of an entity to its own defining source file."
- private: "restricts the use of an entity to the enclosing declaration". Extensions in the same file are included: "Declare a private member in the original declaration, and access that member from extensions in the same file."
- @testable: "a unit test target can access any internal entity, if you mark the import declaration for a product module with the `@testable` attribute and compile that product module with testing enabled."

### Visibility predicate V(member, caller)

| Member access | Visible from the caller iff | Label |
|---|---|---|
| `private` | the caller is in the same file **and** inside the declaring type, its extension, or its nested types in that file | [V] TSPL; [U] the nested-type clause |
| `fileprivate` | same file | |
| `internal` / none | same module, or an importing module whose import is `@testable` | |
| `package` | same SwiftPM package | [U]: DevMap has no package identity; treat as `internal` |
| `public` / `open` | same module, or any importing module | |

The effective access is the minimum of the member's own level and its container's (TSPL; noted at `langdecl/swift.rs:420-424`). Reading only the member's own modifier over-reports visibility. For shadowing that is the *unsafe* direction under I2's converse, so where F1 is absent the rule must also check the container's access.

**Rules.**
- **Binds** a member, or counts it as a shadow (I2), only if V holds.
- **Abstains** when V is undecidable, for example an `internal` member across modules when the import's `@testable` status is unknown. Abstaining here means no bind and no shadow, so the free function keeps its edge only when it is itself visible and unique.
- **Never** binds or shadows with a `private`/`fileprivate` member from another file, or with a superclass's `private` member from a subclass in another file. fs6 and fs7 [R] show the free function wins there.

### What the code knows today

- **Two levels only.** `is_exported` is false only for `private`/`fileprivate` (`langdecl/swift.rs:425-430`) [V]. The doc at `:401-418` records why `internal` is deliberately not modelled for dead code. `unexported_symbols` holds those private names (`index.rs:146-152`) [V].
- **`internal` and `public` are indistinguishable**, and `@testable` has no effect: no flag is extracted (F3). Consequence [I]: `import N` binds N's **internal** free functions too. The import binding at `index.rs:1097-1131` reads `swift_module_symbols`, which admits every non-private file-level name (`index.rs:452`).
- **Where the predicate exists and is called** [V]:
  - `candidate_is_visible_from` (`candidates.rs:122-139`);
  - `swift_member_hit_is_visible_from` (`:152-182`);
  - receiver-typed hits (`dispatch.rs:598-613`);
  - chained receivers (`dispatch.rs:376-386`);
  - module symbols (`index.rs:452`);
  - import bindings (`index.rs:1111`);
  - the same-module rung (`dispatch.rs:79-83`).
- **Where it is not called** [V by reading]:
  - `implicit_receiver_target` (`implicit_receiver.rs:22-95`);
  - `supertype_method` (`:106-149`);
  - `inherited_method` (`dispatch.rs:456-509`);
  - lane J's shadow walk.
- **Possible pre-existing wrong edge** [I], untested: a `private` extension member in file E, plus a same-module free function in a third file, gives a bare call in file B that binds E's private member through `supertype_method`. A test is owed before any lane relies on the member rung.

## 3. Coexistence

| Coexisting declarations of base name `n` in one owner | Swift (call `n(...)`) | Resolver rule |
|---|---|---|
| Overloads by arity or labels | Overload resolution picks one. [R] brief J b5: a labelled overload still shadows a free `n()` by base name | Counts as a shadow (I2). **Binds** only when exactly one overload exists, or all share one identity (overloads mint one id today, `index.rs:120-167` `shared_identities`). The overload-self abstention at `resolve_all.rs:1612-1625` stays |
| Enum case `case n(..)` + method or static `func n(..)` | The labels select. MarkDev `Deco.tableRow` and rc1/rc3/rc5 show the method is the target | **Binds the method** when the call cannot be a case construction: it is bare inside the type and the case is constructed only through leading-dot `.n` / `T.n`. **Abstains** when the case and the method are indistinguishable by the recorded facts. **Never** drops the edge because a case of that name exists |
| Property `var n` + method `func n()` | Both are members; a call `n()` reaches the method (`property_member` doc, `candidates.rs:329-336`) | Calls consult the plain key and reads consult `\u{1}n` first (`candidates.rs:341-345`) [V]. A stored closure property `var n: () -> D` shadows a free `n` (ms20) |
| Static + instance member of one name | Both are found by member lookup; [R] brief J a4: a static called from an instance method shadows (swiftc error) | Counts as a shadow. **Binds** only when `is_static` matches the call context (needs F1). Otherwise **abstains** |

**Today** [V]:
- An enum case and a method share **one identity**. The case is owned by `enclosing_owner_path` and the method by `enclosing_owner`. Both give `Deco.tableRow` for a top-level enum (`langdecl/swift.rs:82-88`).
- The case keeps the plain `type_methods` key (`index.rs:181-183`). The pair becomes two hits under one key, and `implicit_receiver_target` returns `None` on `local.len() > 1` (`implicit_receiver.rs:47-48`).
- Rung 2c drops the identity because the case is a `Field`. `file_declares_as` (`references.rs:845-855`) is true when *any* declaration of that identity in the file matches `never_a_call_target` (`candidates.rs:263-279`); the filter is at `resolve_all.rs:1094-1098` [V by reading; not run].
- **How lane J lost rc1/rc3/rc5 is unverified.** Ruling 55(c) requires the trace before any fix. The working hypothesis, that main binds the method only at the global rung and the lane's shadow flag suppressed that rung, is [U].
- The durable fix is a distinct identity for the case (F1 `role`, or v98 row 17 "distinct overload ids", which lists Swift in Q3 of `extract-v98.md`).

## 4. Lookup order for a bare call `n(...)` inside a type body

### Swift's order (normative target)

0. **Local scopes, innermost outward,** up to the type body: locals, parameters, closure parameters, capture lists, `if/guard/while let`, `case let`, `catch`, and local functions. A hit ends lookup. [R]: fs8, a local function, wins over both member and free.
1. **Members.**
   - **1a.** The innermost enclosing type: its body, its extensions in every file of the module (visible per §2), its superclass chain, the extensions of the protocols it conforms to, and, inside a protocol extension, the protocol's own requirements (ms21).
   - **1b.** Then each outer enclosing type in turn, with the same chain (ms12, ms13, ms14 [R]).
   - The first level with any visible member of base name `n` decides (I2). Several candidates at that level means abstain. [U]: whether Swift continues to an outer type after an inner one matched only by an invisible member. This note treats invisible members as absent (§2).
2. **Module scope: this file.** File-level functions, including `private`/`fileprivate` ones.
3. **Module scope: other files of the caller's module(s).** Non-private names only.
   - [U] Swift has one module scope, so steps 2 and 3 are not ordered by the language. A same-file free function and another file's internal free function of one signature may be ambiguous. Keep today's order (2 before 3) and owe an attack test.
4. **Imported modules.** Public/open names, plus internal names under `@testable` (§2).
   - The caller's own module shadows an import. This is [V] as resolver behaviour (`own_module_declares`, `dispatch.rs:56-63`, `index.rs:1125`) and [U] as a TSPL citation.
   - Two imports declaring `n` means abstain.

**`import N` free function versus a member.** [R]: the attack's `ms17` and `ms17b` (`atk-j.rs:1239,1385`) record that swiftc picks the member D over N's free function. The member step (1) precedes the import step (4).

### The resolver's order today

| Step | Code | Notes |
|---|---|---|
| 1b X42 | `resolve_all.rs:749-815` | Only when `receiver_expr` is `self` [V] |
| **2a import binding** | `resolve_all.rs:838-868` | Runs for **every bare call**, before any member step. Reads `import_bindings`, which `import N` fills with N's file-level names (`index.rs:1097-1131`) [V] |
| 2c lexical | `resolve_all.rs:1085-1103` (`lexical_target`, `unresolved.rs:16-55`) | Includes Methods for Swift (`family_needs_explicit_receiver`, `scope.rs:88-93`; filter at `unresolved.rs:32-34`). It finds a same-file member first: a body member, and likely a same-file extension member too, because `owner_identity` gives that the same bare owner (`langdecl/swift.rs:376-386`) [I]. It then falls through to the same-file free function. It never sees an extension in another file or a supertype [V] |
| 2c' protocol fan-out | `resolve_all.rs:1143-1192` | |
| 2d type-named receiver | `resolve_all.rs:1194-1290` | Not reached by a bare call |
| **Member rung** (cross-file extensions, supertypes) | `resolve_all.rs:1437-1474` → `implicit_receiver_target` | **After 2a and 2c.** Answers only `Unique`; on `None` the call falls through (no I2) [V] |
| Same module | `resolve_all.rs:1475-1495` (`same_swift_module_target`, `dispatch.rs:65-99`) | |
| S2 global | `resolve_all.rs:1552-1606` | Unique namesake becomes `UniqueGlobal`; several namesakes become a ledger-only `AmbiguousGlobal` |
| `factory_return_type` bare branch | `value_types.rs:301-318` | Types a `let x = n()` local from a same-file or imported free function's return. It never asks about members [V] |

| Swift step | Resolver today | Defect (card) |
|---|---|---|
| 1 member before 4 import | 2a import **before** the member rung | ms17, ms17b (ft-42bde27a) |
| 1 member (any file, supertypes) before 2 file | 2c same-file free function **before** the member rung | lane J b1 (ft-ba70369a) |
| 1 with a typealias base | `supertypes` holds the alias name, which has no members | ms15 (ft-42bde27a) |
| 1 with an unindexed base | the walk ends silently | NSObject.copy (ft-2cb3620f, §5) |
| 1 decides (I2) | member-rung `None` falls through to 3, 4 and global | lane J added I2 without the converse, giving fs1–fs9 |

**Required shape (normative).**
1. Compute the member verdict once per bare call, before rung 2a: `Bound(target) | Shadowed(reason) | NoMember | LeftIndex(family)`.
2. Feed that one verdict into 2a, 2c, the same-module rung, S2, and `factory_return_type`'s bare branch.
3. The verdict reads the §1 owner identity and the §2 visibility. It never reads a bare `contains_key`.

## 5. The unindexed-superclass rule (SDK)

**Problem.** A walk that leaves the index (an SDK base: `NSObject`, `XCTestCase`, `UIViewController`) cannot say whether the base declares `n`. Card ft-2cb3620f's case, [R] per the card: inside `class C: NSObject`, a bare `copy()` is `NSObject.copy()`, but main binds the same-file free `copy` at 1.0.

**Numbers** [V, `lane-j-evidence/evidence.tsv`]:
- At lane J's first build (`b9ec1c69`), family abstention lost 159 edges in 17 class rows.
- 157 of them are in `XCTestCase` subclasses: ChronosFlow 13 classes, Graft 1 class with 31 edges, M5Blade 1 class with 8 edges. All are true edges by fact (ruling 54). Two `first=View` rows were the Curio `DigestController.dismiss` edges (`Curio.lost.edges`). The indexed-only cut at `22cbc421` also removes them [V, BUILD_REPORT lane J row]. That they come from an indexed member shadow is [I]: the shadowing member was not read.
- Zero corpus wrong edges come from an unindexed base. The NSObject case exists only as a synthetic test (`atk-g.rs:1245`; `atk-j.rs:604-657`, five ignored tests).

**Options for the owner** (one must be chosen before ft-2cb3620f opens):

| Option | Effect on the corpus | Risk |
|---|---|---|
| A. Status quo: bind through an unindexed base | 0 lost, 0 fixed | Keeps the synthetic NSObject wrong edge at 1.0 |
| B. Abstain by family, whenever any walked class's first heritage entry is unindexed (lane J's `b9ec1c69` design) | −157 true edges, 0 wrong fixed | Already rejected by the owner ("Indexed shadow only", 2026-10-28) |
| C. Curated SDK member list (recommended): a committed data file of base names for the common SDK classes (`NSObject`, `NSResponder`, `NSView`/`UIView`, `NSViewController`/`UIViewController`, `XCTestCase`, …). Generate it offline from SDK `.swiftinterface` files by a script, with the SDK version recorded. Abstain only when an unindexed base in the walk is on the list **and** its list declares `n` | Keeps the 157 (`XCTestCase` declares no `decodeStream`). Fixes `NSObject.copy` | The list ages. A base not on the list falls back to A, which must be counted in the run report |
| D. Read SDK interfaces at index time | Same as C | Machine-dependent output (Xcode presence and version) breaks determinism; heavy. Rejected |

**Rule under C.**
- **Binds** as today when no listed unindexed base declares `n`.
- **Abstains** (`LeftIndex`) when a listed base declares `n`. That counts for I2.
- **Never** treats an unlisted unindexed base as declaring `n` (no blanket abstention).
- The heritage entry is read as the **first** entry of the class's own declaration. A conformance (`View`, `Codable`) is not a superclass. Brief J §2 gives the narrowing; struct and enum never leave the index.

## 6. Receiver-typing rules

| Shape | Typed when | Abstains when | Today | Card |
|---|---|---|---|---|
| `let x = a ?? b` | `a` has a declared type `T?` (or `T!`): `x` is `T`, from the **left** operand | `a` is untyped. Never type from `b`'s constructed type, which may be a subtype; finality is not recorded | The climb types from `b`: the catch-all arm is [V] at `langcalls/swift.rs:630`, the behaviour is [R] from the lane I brief probe. Lane I abstains: 48 corpus drops, ordered by ruling 50 | ft-9b28adab item 1 |
| `let x = c ? A() : B()` | Both arms construct the same nominal `T` and nothing is written | The arms differ, or either arm is not a construction | The climb types it [V] | ft-9b28adab |
| `x?.f()` / `x!.f()` | `x`'s declared type with one trailing `?`/`!` stripped, stripped **in `admissible_nominal_type` only** (ruling 52) | The type is generic or a collection | `admissible_nominal_type` (`receiver_types.rs:766-816`) rejects `ErasePlan?` [V]. The cited `:468` strip is Rust's `?` operator in `rust_call_receiver_shape`, not a Swift optional [V] | ft-4ca45b19 |
| `T.shared.f()` (the `.shared` convention) | `shared`'s written type is `T`, or its initializer is exactly `T(...)` (`scoped_receiver_types["F:F::T:shared"]`, [R] brief G §1 probe) | `shared` is written or initialized as anything else (`= T().inner`, `: any P`, a subclass) | Rung 2d reads the **leading capitalised segment** of `Renderer.shared` as the type, whatever `shared` is (`resolve_all.rs:1216-1231`) [V]. Pinned by `tests/swift_modules.rs:282`; needs a decoy | ft-9b28adab item 2; T4 |
| Existential `any P` / `some P` | Q4 decides: fan out through `protocol_dispatch` (`dispatch.rs:522-572`) as `Dispatch`, never `ReceiverType` | Always under the abstain option. Never the initializer's concrete type (ruling 46) | No Type ref is emitted, so it abstains by accident (lane I brief §2; [U] this session). `swift_chained_receiver` strips `some`/`any` from return types only (`dispatch.rs:396-400`) [V] | ft-9b28adab item 3 |
| `T(..).f()` as a receiver | `T` is one visible indexed class, struct or enum (`swift_constructed_receiver`, `dispatch.rs:277-313`) [V] | `T` is a function, a chain, or ambiguous | Typed | — |
| `let x = T(..).f()` (construction receiver in a binding) | F4 shape, distinct from static `T.f`. `f` is the one visible member of `T` (§1, §2); its written return is admissible after the `?` strip; `-> Self` gives `T` | Overloads, generics, collections, a namesake `T` | `call_shape` (`swiftscope.rs:1566-1588`) returns `None` for this shape, so there is no initializer [V] | ft-4ca45b19 resume 1 |
| `let`-slot climb | The value is the declaration's whole `value`, through the allow-list only: `try`/`try?`/`try!`, `await`, a sole trailing `!`, one unlabeled parenthesis. Stacking is allowed (rulings 48/50) | Member read, call result, literal, `as`, `??`/ternary (handled by their own rows), interpolation | Catch-all `_ => current = parent` (`langcalls/swift.rs:596-634`) [V] | ft-4ca45b19; ft-c0b8696f 1g/1h |
| Written type versus initializer | A written type always wins over the constructed one | — | Lane I implements this [V by card] | ft-4ca45b19 |

## 7. Conformance table

The test files are the acceptance tests. A row resumes only when every listed test passes, the corpus diff (§8) is clean, and the rules cited hold.

| Card / lane | Rules | Extractor facts | Acceptance tests |
|---|---|---|---|
| **ft-ba70369a** (lane J; `22cbc421`, attack `6b280ead`) | §1, §2, §3 (enum case), §4 (single verdict and I2 with its converse) | F1 for full (a). Resolver-only is possible for 2c/factory, but fs1/fs2/fs9 need owner paths | `6b280ead:…/swift_resolver_residue.rs`: the 42 `attack_j_*` (`:838-1496`) and lane tests `:348-800`. ms15/ms17/ms17b stay ignored, owned by ft-42bde27a. Ruling 55(e): DevType +≤1% instructions |
| **ft-42bde27a** (ms15, ms17, ms17b) | §4 (import after member), §1 (typealias base) | ms17/17b: none (resolver order). ms15: F2 | `6b280ead` `:1207`, `:1239`, `:1385`, un-ignored |
| **ft-2cb3620f** (SDK base) | §5 | None (data file, under option C) | `6b280ead` `:604-657` (5 ignored); `2d8b2c38` `:1245`. Corpus keeps the 157 |
| **ft-4ca45b19** (lane I; `9bb3ec01`) | §6 climb, `?`, construction receiver | F4 (98/99) | `9bb3ec01:…/swift_resolver_residue.rs` `ctl_main_*` `:318-353` and the allow-list tests `:383-660`; `langcalls_swift_scala.rs` additions. Corpus: exactly 80 lost (ruling 52) |
| **ft-dc82251f** (lane G T4 row; `140bd425`, attack `2d8b2c38`) | §6 `.shared`, §1 | T4's `assigned_to` on the Name ref (extractor, 98/99). Waits on ft-4ca45b19 | `2d8b2c38` T4 `:777-1052`, `:1567-1580`, guards `:1587-1645` |
| **ft-843b1e92** (O1c) | §4 step 0 (rebinds), §4 1b (outer-type static), §1 | O1c `[]name` initializer (extractor, 98/99) | `2d8b2c38` O1c `:1053-1325`, `:1475-1566` |
| **ft-adc4db2e** (Y1) | §1 (alias holder identity), §2, §4 4 (import), `#if` | F2, F1 `conditional`/`extension_constrained` | `2d8b2c38` Y1 `:1326-1474`, guard `:1637` (`guard_y1_target_alias_alone_…`) |
| **ft-e3c6ff8f** (extension typealias Type edge) | §1 typealias, the same Y1 guards | F2 (owner push decision) | the five Y1 wrong-edge `atk!` in `2d8b2c38` `:1331-1474` |
| **ft-9b28adab** (`??`, `.shared`, existential) | §6 | `??`: none if the left operand's declared type is already extracted [U]. Existential fan-out: F5 | `9bb3ec01` `ctl_main_member_typed_any_protocol` (`:353`); a new decoy beside `tests/swift_modules.rs:282` |
| **ft-c0b8696f** (Swift walkers 2a/2c/1g/1h) | §6 climb (1g/1h), receiver-text hygiene | Extractor output (98/99) | `attack_recovery_residue.rs` @ `e25889df`; `attack_swift_empty_name.rs` (ignored rows citing the card) |
| **ft-8ca270ad** (L-Swift umbrella) | All. Open ACs route to ft-80c5d100 and ft-915b0a03 (card log, 2026-10-17) | Per child card | `rust/testdata/accuracy/swift/edges.tsv`, `known_failing.tsv` |

## 8. Corpus-diff discipline for Swift lanes

1. **Run before review, not after** (ruling 55 owner note).
   - Script: the lead's corpus-diff script. It is session-local and not committed, so the lane brief supplies it. The `REPOS` default is **15 repos**: ChronosFlow, Meridian, Graft, Vani, Curio, M5Blade, GitPulse, Strait, Blitztree, gusset, ojas, scholarlm, DevCouncil, AppLock, DevType.
   - Each repo is a `git archive HEAD` snapshot built cold with `build --full` and a fresh `--db`.
   - Set `C=` to a lane-private directory whenever another run may be live, because `$C/out` is shared.
   - The baseline is `bench/tgt-main782`: `BASE=main782 LAND=<lane> SUMMARY=<lane> C=… bash run5.sh`.
   - **Add MarkDev to `REPOS`** as `devtools/MarkDev` (it exists at `/Users/bharath/Code/devtools/MarkDev` [V]; its build under the script is [U]). rc1 came from MarkDev, outside the corpus, and the corpus missed it (card ft-ba70369a log).
2. **Validity gates before reading the diff.**
   - Every arm `rc=0`.
   - `gaps lost = added = 0`. A budget refusal or parse gap invalidates the row (memory: corpus record load guard).
   - Note the load average the script prints.
   - Extractor lanes run both arms cold (shared `extract-v98` cache key).
3. **Classify every lost and every added edge into exactly one bucket.** Cite the binding line and the declaration for each.
   - **wrong-removed**: show the rule that makes it wrong.
   - **true-lost**: parks the lane.
   - **ordered-abstention**: name the ruling, for example ruling 50's `??`.
   - **Unknown-abstained**: §5; evidence per row (ruling 54).
   - **true-added**: show the declaration.
   - **wrong-added**: parks the lane.
   - No edge goes unclassified, and a sample is never presented as the whole set.
4. **Pre-state the expected counts** in the brief, for example "exactly 80 lost". A different count is a stop, not a re-classification.
5. Stop conditions are written into the card before the attack round, under rulings 48 and 55. One attack round per lane.

## 9. Open questions for Fable and the owner

1. **Do F1 (`swift_decls`), F2, F3 and F4 ride the open 98 or wait for 99?**
   - *Recommendation:* hold the push for F1 and F4 only.
   - F1 and F4 gate the two parked resolver lanes (J and I), and 98 is still unpushed.
   - F2, F3 and F5 go to 99 with its sweep, because their cards are P2/P3 with no corpus value (ft-adc4db2e: no real-code shape).
2. **Should lane J resume resolver-only, with a partial owner identity built from file, module and qualified names, or wait for F1?**
   - *Recommendation:* wait for F1.
   - Lane J's `qualified_names` reconstruction is exactly what fs1/fs2/fs9 broke. A second resolver-only attempt repeats the brief failure ruling 55 named.
3. **Unindexed SDK base: option A, B, C or D (§5)?**
   - *Recommendation:* C, the curated list generated from `.swiftinterface` and committed as data.
   - Owner decision, because B was already refused.
   - A new data file needs no dependency, but the generator script's language is subject to the language policy: Rust or Go.
4. **Existential `any P`: abstain, or fan out through `protocol_dispatch` as `Dispatch` (ruling 50 left this open)?**
   - *Recommendation:* fan out, only for an explicitly written `any P` / `some P`, labelled `Dispatch` and capped by `DISPATCH_FANOUT_CAP`.
   - Never `ReceiverType` to the initializer's type.
5. **Should `internal` across modules be modelled (it needs F3 and the `public` level)?**
   - *Recommendation:* yes, for import binding only (`index.rs:1097-1131`): bind only public/open names unless the import is `@testable`.
   - Keep dead-code's two-level model unchanged (`langdecl/swift.rs:401-418`).
6. **Same-file versus same-module free function of one name (§4, steps 2 and 3, [U]).** Probe with swiftc in a lane brief.
   - *Recommendation:* keep today's order until a probe shows the pair is ambiguous; then abstain.
7. **Enum case and method identity: distinct ids now (v98 row 17 lists Swift), or a role flag in F1?**
   - *Recommendation:* the F1 `role` flag plus a resolver rule (§3). Distinct ids churn every edge key.
   - Trace the rc1 loss first (ruling 55(c)).
8. **Fix the possible pre-existing wrong edge first?** A private extension member in another file is bound through `supertype_method` with no visibility check (§2, [I]).
   - *Recommendation:* add the failing test in the first Swift lane's red step. If it is red on main, fold the visibility filter into the single member verdict (§4) rather than file a separate card.
9. **Add MarkDev to the corpus, as `REPOS` repo 16?**
   - *Recommendation:* yes. rc1 was real code the 15 repos missed. This also changes the baseline counts, so rebuild `tgt-main782`'s diff first.
