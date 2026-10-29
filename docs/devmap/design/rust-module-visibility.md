# Rust module and visibility model for the DevMap resolver (design note, normative)

Status: design only. No code, no build. Base: local main `782b9f16`, store generation 4634.
Accepted by Fable rulings 106–107 (Wave 10, 2026-10-29) on ruling 75's terms: [U] rows and [U] sub-clauses are not binding rules, so a brief that relies on one carries its own rustc probe. The amendments, all applied in this copy:
- **106.** The draft's claim that `use x::*` binds at the bare-name rung was refuted by main. A glob of an indexed module binds at rung 2a with no visibility test. That is a pre-existing wrong-edge class outside K (ruling 18, part 2), named as gap G-GLOB. Changed: I2, R3.7, §4 row 2, §7 row R3.7, §10, §12 item 2.
- **107.**
  - Ruling 87 item 5 (crate roots from the declarer graph) is replaced by R1.7: target path first, and the declarer-graph fallback only outside every known source root.
  - Ruling 87's "parents come from `rust_mod_declarers`" becomes: parents come from a declarer map the resume builds on main (the lane's map is not on main), and a hop out of an inline module goes to the enclosing module (U3).
  - Two disagreeing parents give Visible (§6).
- **112.** Every probe verdict is the probe runner's `results.txt` line (50 probes, kept with the probes and not committed). It is reproduced in Appendix B.

Required by Fable ruling 88 before the K resume (ruling 87, card ft-1b21e9b6). It also serves card ft-fff6fb52 (`crate::` from roots outside `src`), which shares this model.
Every Rust resolver brief cites this note by rule number (R1.1 and so on). A lane that departs from a rule names the rule and the ruling that waives it.

**Labels.**
- **[V]** means verified against rustc this session. The probe is named, and its source and verdict are in Appendix A.
- **[U]** means unverified, so the rule is not binding. A brief that relies on a [U] rule carries its own probe.
- Claims about DevMap code carry a second label:
  - [C]: read in source at the cited ref this session.
  - [R]: recorded by an earlier lane or attack and not re-run here.
  - [I]: inferred from code read this session, not run.

**Probe environment.**
- rustc 1.99.0 (b940084d7 2026-09-28) and cargo 1.99.0 (5f94df478 2026-08-27), edition 2021.
- Packages are checked with `cargo check --offline -j 1 --all-targets --keep-going`. Single files are checked with `rustc --edition 2021 --emit=metadata`.
- "compiles" means exit 0. A rejection is reported with its error code.
- Every verdict cited below is that probe's line in the runner's `results.txt`, one sequential run of all 50 probes, reproduced in Appendix B. Individual compiler logs are not cited.
- Probes A01–A20 are the K attacker's scratch crates (cited by name at `f34f66fb:rust/devmap-resolve/tests/attack_rust_visibility_k.rs`, e.g. "scratch crate v06"). They were copied unchanged and re-run here. The probes named P… are new.

---

## 0. Invariants

- **I1.** A wrong edge is worse than an abstention. An abstention is `Resolution::Unresolved { reason }` with a reason. It never falls through to a rung that binds by spelling.
- **I2. The premise is compiling code.** In code that compiles, every path and every `use` the author wrote names an item that is visible from where it is written. rustc checked it. Visibility therefore matters to the resolver in exactly two places:
  - **(a) A rung that picks a target by spelling.** This is the bare-name `UniqueGlobal` rung. Visibility filters which namesakes could be the target.
  - **(b) A rung that maps a written path to a file.** This is `rust_qualified_call`. There, a Hidden verdict proves that the resolver mapped the path to the wrong module or crate, so the answer is `Unresolved{reason}`.
  - Rungs that follow a **named** `use` or a `mod` binding (2a for a named import, 2b) need no visibility check, because the author named the item and rustc checked it.
  - **A glob is not a named import.** `use x::*` names no item. rustc imports through it only the items the importer can see (R3.7). Main's rung 2a instead binds *every* `file_symbols` entry of the glob's target (`index.rs:1071-1093`) [C]. That is wrong-edge class G-GLOB: pre-existing, outside K (ruling 106; ruling 18, part 2). Section 4 lists the rungs.
- **I3. The predicate has three values** (§6): Hidden only on proof, Visible, or Unknown. Unknown keeps main's answer byte for byte (ruling 87).
- **I4. A file is not a module.** A module is an *instance*: a file as loaded by one `mod` declaration (or as a crate root) inside one crate. One file can be several instances (R1.6, R2.1). Visibility is decided per instance pair. A file-level verdict is:
  - Visible if any caller instance can see any callee instance in a crate that compiles both;
  - Hidden only if every pair is proven hidden;
  - Unknown otherwise.

## 1. Crate-root identity

- **R1.1 [V: A04, A10, PXC, PBINS, P18, P13, A20, P16, P21, P14, P22].** A crate root is a file the build compiles as an entry point. In a Cargo package the roots are exactly the targets:
  - the library: `[lib] path`, else `src/lib.rs` (PXC);
  - binaries: `src/main.rs` (A04, A10), `src/bin/<n>.rs` and `src/bin/<n>/main.rs` (PBINS), and `[[bin]] path` (P18);
  - integration tests: `tests/<n>.rs` and `tests/<n>/main.rs` (PBINS, P13), and `[[test]] path` (A20);
  - examples: `examples/<n>/main.rs` (P16);
  - benches: `benches/<n>.rs` (P21);
  - the build script: `build.rs` (P14).

  Outside Cargo, a root is any file handed to rustc (P22).

  Not probed, and so [U]: `examples/<n>.rs`, `benches/<n>/main.rs`, `[[example]]`/`[[bench]] path`, and `package.build`. These are the same Cargo auto-discovery rules.
- **R1.2 [V: A10, PBINS].** A root need not declare any module. A10's `src/main.rs` is `fn main() {}`. PBINS's `src/bin/one.rs` and `tests/a.rs` declare nothing. Each is still a root.
- **R1.3 [V: PLIBMOD].** A root can also be declared. When `tests/t.rs` writes `#[path = "../src/lib.rs"] mod l;`, `src/lib.rs` is the library root *and* module `l` of the test crate. Membership by target wins, and the declaration adds a second instance.
- **R1.4 [V: PSTRAY].** A file that no `mod` declares and no target names is compiled into nothing. `src/stray.rs`, holding text that is not Rust, sits beside `src/lib.rs` and the package still checks clean.
- **R1.5 [V: PBINSN, PXCN].** Every target is its own crate:
  - `src/bin/two.rs` cannot write `crate::only_one()` for an item of `src/bin/one.rs` (E0425 "cannot find function `only_one` in the crate root");
  - `tests/b.rs` cannot reach `tests/a.rs`'s item (E0425);
  - library, binary, test and example are distinct crates (PXCN, §5).
- **R1.6 [V: P04T, PLIBMOD].** `crate` names the root of the crate that compiles *this instance*.
  - In P04T, a file reached from two roots is two modules with distinct items: E0308 "expected `p04t::util::S`, found `util::S`".
  - In PLIBMOD, `crate::inner()` written in `src/lib.rs` compiles as the library and fails when the test crate compiles the same file (E0425: there `crate` is `tests/t.rs`).
- **R1.7 Resolver rule.** Clause (a) and the facts behind (b) are [V] by R1.1–R1.4. Clause (b) as a heuristic is [U] (resolver policy).
  - **(a)** f is a root if it sits at a target path the index knows: a conventional path under a package directory, or a manifest `lib_path`/`bin_paths`. This holds even when some `mod` declares f (R1.3).
  - **(b) Fallback,** for target lists the index cannot see (a custom `[[test]]`/`[[example]]`/`[[bench]]` path, a script): f is a root if it lies **outside every source root the index knows**, no indexed file declares it, and it declares at least one module, by `mod` or `#[path]`.
    - The source-root guard matters. Without it, a dead `src/foo/mod.rs` that nothing declares but that declares children would become a root, which contradicts R1.4 and U7. It would also hide the library's `pub(crate)` items from that file where main binds them. The lane's `rust_target_key` has the same guard (lane `:338-340`).
    - `it/main.rs` and `scripts/gen/main.rs` qualify.
  - An undeclared file that meets neither clause is **unowned**.

  The card and ruling 87 define a root as clause (b) alone. R1.2 shows (b) is not necessary, and R1.3 shows "no `mod` declares it" is not necessary either (§12).
- **R1.8 [V: P04T, PLIBMOD, P13]. Crate set.** A file's crate set is the set of roots whose declarer walk (§2) reaches it, plus the file's own key if it is a root. A root that another root declares (R1.3) has both keys. `crate::` written in a file is evaluated once per member of its crate set (R1.6).

| Root kind | Example path | `crate::` in a file of this crate means | Probe |
|---|---|---|---|
| library | `src/lib.rs`, `[lib] path = "core/lib.rs"` | that file | A10, PXC |
| default binary | `src/main.rs` | `src/main.rs`, a separate crate from the library | A04, A10 |
| `src/bin` file / dir | `src/bin/one.rs`, `src/bin/dir/main.rs` | that file | PBINS, A08 |
| custom binary | `[[bin]] path = "src/cli/main.rs"` | that file, even under the library's `src/` | P18 |
| integration test | `tests/a.rs`, `tests/d/main.rs` | that file; `tests/common/mod.rs` is a module of every test that declares it | PBINS, P13 |
| custom test | `[[test]] path = "it/main.rs"` | that file (not in `RustCrateManifest`, so R1.7(b)) | A20, P20Q |
| example / bench | `examples/x/main.rs`, `benches/b.rs` | that file | P16, P21 |
| build script | `build.rs` | `build.rs`; `mod helper;` there is `<pkg>/helper.rs` | P14 |
| script (no manifest) | `scripts/gen/main.rs` | that file | P22 |

## 2. Module parent

- **R2.1 [V: A04, P05, P04P, P04PN].** A file module's parent is the module whose `mod` item loaded it. Each declaration is its own instance with its own parent:
  - `util.rs` declared by both `lib.rs` and `main.rs` (A04, P05);
  - or declared within one crate by `lib.rs` and, through `#[path]`, by `a/mod.rs` (P04P).

  `pub(super)` is decided per instance. In P04P the root's own copy is visible to the root. In P04PN the copy loaded under `a` is hidden from the root: `a::s::helper()` gives E0603.
- **R2.2 [V: A07, A07G].** With `#[path = "impls/x_impl.rs"] mod x;` in `lib.rs`, `lib.rs` is the parent of `x_impl.rs`. The directory `impls/` plays no part.
- **R2.3 [V: A01, A09, P12].** `a/mod.rs` and the leaf `a.rs` are the same module shape. Both keep their children in `a/`.
- **R2.4 [V: PPATH, A01].** Where a declaration points:
  - plain `mod x;` in a root or a `mod.rs` points to `<dir>/x.rs` or `<dir>/x/mod.rs`;
  - plain `mod x;` in the leaf `a.rs` points into `<dir>/a/` (A01's `a2.rs` declares `a2/child.rs`);
  - `#[path = "p"]` on a non-inline `mod` is relative to the **declaring file's directory** for leaf files too: in `src/a.rs`, `#[path = "x.rs"]` loaded `src/x.rs`, not `src/a/x.rs`;
  - inside an inline `mod inl { … }`, both plain `mod z;` and `#[path = "y.rs"]` are relative to the directory **plus the inline segments**: `src/a/inl/z.rs` and `src/a/inl/y.rs` from leaf `src/a.rs`, and `src/b/inl/y.rs` from `src/b/mod.rs`.
- **R2.5 [V: PINL, PINLN].** Inline modules are modules. In `f.rs`, an item of `mod inner { … }` has the parent `f::inner`. Its `pub(super)` reaches `f` and `f`'s subtree, not `f`'s parent: the crate root calling `crate::f::inner::x()` gives E0603.
- **R2.6 [V: PIP-rootsuper].** A crate root has no parent. `pub(super)` written in a root is E0433 ("too many leading `super` keywords"). So compiling code never asks for the parent of a root instance. A file that is both a root and a module (R1.3) has parents only in its module instances.
- **R2.7 [V: A07G, P04P, P04PN, PINL; derived from R2.1–R2.6]. Resolver rule.** `parents(f)` is the set of indexed files whose `mod` or `#[path]` declaration resolves *exactly* to f. Per ruling 107, the resume builds this declarer map on main; the lane's `rust_mod_declarers` is not on main. For crate C, f's instance in C has as parents those declarers whose own instance is in C. A directory convention is never a parent source; that is the k07g failure. A hop out of an inline module goes to the enclosing module, not to a declarer.

## 3. Visibility kinds

Notation: T(m) is module instance m together with all its descendants, through file and inline modules alike.

- **R3.1 [V: P01, A01, P01N, PIP-inself]. Private, `pub(self)` and `pub(in self)` are visible in T(m).**
  - That includes a child (`super::helper()`), a grandchild (`crate::a::helper()`, `super::super::helper()`) and a child's `use super::*;` glob (P01, A01 for both `a/mod.rs` and the leaf `a2.rs`). `pub(in self)` compiles (PIP-inself, A01 `a3`).
  - A sibling module gets E0603 for both the `pub(self)` item and the private item (P01N).
  - Private and `pub(self)` behave identically in every probe.
- **R3.2 [V: A04, A07, A07G, A08, A09, P09N, P04P, P04PN, A06, P13, PINL, PINLN]. `pub(super)` is visible in T(parent(m))**, computed per instance (R2.1).
  - A09: `a.rs` and `a/c.rs` reach `a/b.rs`'s item.
  - P09N: `far.rs` gets E0603.
  - A06 and P13: a `tests/common/mod.rs` item reaches the test that declares it.
- **R3.3 [V: A10, PXC, PXCN]. `pub(crate)` and `pub(in crate)` are visible in every module of the crate that compiles the instance.** A10 (`pub(in crate)`, with `src/main.rs` present) compiles.
- **R3.4 [V: A11, P11N, P12, P12N, PIP-nonancestor, PIP-child, PIP-nokeyword]. `pub(in path)`.** The path must start with `crate`, `self` or `super`: edition 2021 rejects `pub(in a)` with "relative paths are not supported in visibilities in 2018 edition or later". It must also name an ancestor: a sibling (`crate::b`) and a child (`self::x`) both give E0742. The item is visible in T(A), where A is that ancestor.
  - `pub(in super::super)` is two hops. A11's `a/x.rs` compiles, and P11N's `far.rs` gets E0603.
  - `pub(in crate::a)` excludes the prefix sibling `ab` (P12 compiles, P12N gets E0603).
  - **Consequence.** A is found without any file lookup: walk k hops up the instance's own ancestor chain (`self` is 0 hops, each `super` is 1), or, for `crate::p`, take the ancestor whose path from the root is `p`. A namesake can never make an `in` path ambiguous, and any path that is not an ancestor never occurs in compiling code.
- **R3.5 [V: PXCN]. `pub` places no limit of its own, but the path must cross only reachable modules.** From another crate, `pxcn::hidden::behind()` (a `pub fn` in the private `mod hidden`) gives E0603 "module `hidden` is private". This is why a cross-crate `pub` edge cannot be proven visible from the item's header alone.
- **R3.6 [V: PXCN, PBINSN, PXCB]. Nothing restricted crosses a crate boundary.** Private, `pub(self)`, `pub(super)`, `pub(in …)` and `pub(crate)` items are never visible from another crate, even inside one package. PXCN gets E0603 at `src/main.rs`, `tests/t.rs` and `examples/e.rs` for the library's `pub(crate)` fn. So the crate bound holds for every restricted item, whether or not a header was stored.
- **R3.7 [V: PGLOBN]. A glob imports only what the importer can see.** `use crate::a::b::*;` in `far.rs` binds `open()`, while `helper()` (`pub(super)` in `b`) gives E0425 "not found in this scope". A hidden namesake is absent: it neither binds nor shadows.
  - **Resolver consequence (ruling 106).** Main's rung 2a binds every `file_symbols` entry of a glob's indexed target (`index.rs:1071-1093`) [C], hidden items included. So rung 2a breaks R3.7 wherever a glob's target declares a namesake the importer cannot see. This is **gap G-GLOB**, a pre-existing wrong-edge class. It is out of K's scope (ruling 18, part 2) and is not fixed by the K predicate, which never sees rung 2a.
- **R3.8 [V: PIP-space].** `pub (crate)`, with a space, is accepted. The extractor's restricted marker tests `starts_with("pub(")` (`lang_helpers.rs:1300`) [C], so such an item reads as plain `pub`, and the verdict falls back to main's answer. That is the safe direction.
- **R3.9 [V: PTRAIT]. A missing `pub` does not make an item private.** A trait-impl method (`impl T for S { fn m() }`) is callable outside its module. Any later slice that applies R3.1 to `unexported_symbols` must first exclude trait-impl items. [I]: whether the extractor marks them unexported was not checked. K keeps ruling 58: private items keep the `rust_crate_src_root` rule.

## 4. Path forms and the rung that handles each on main

Rung names and lines are from `resolve_all.rs` at `782b9f16` [C].

| Form | Rung on main | Visibility consulted today | Model |
|---|---|---|---|
| bare `helper()`, no import | bare-name `UniqueGlobal` (`:1552-1606`; single namesake at `:1585`) | `candidate_is_visible_from` (`candidates.rs:122-139`): only non-`pub` items, by `rust_crate_src_root`. Restricted items always pass | (a) of I2: filter the one namesake with §6, after `declarations == 1` (ruling 56) |
| bare `helper()` through `use x::*` whose module resolves to an indexed file | **2a import binding.** The extractor emits a glob as alias `"."` (`rust_use.rs:437-441`). Rung 2a's index step binds every `file_symbols` entry of the target file, skipping only the file's own module (`index.rs:1071-1093`). The test `rust_module_paths.rs:232-252` asserts `ImportScoped` for `use crate::prelude::*` [C] | **none**: hidden items bind too | G-GLOB (R3.7): a pre-existing wrong-edge class, out of K (ruling 106) |
| bare `helper()` through `use x::*` where `x` is a `#[path]` module, from a custom root, or otherwise not resolved to an indexed file (k07g, k14g, k20g, k21g, k31) | **the bare-name rung, by record.** In the attack, the lane's own Hidden reason string fired on k07g and k20g [R], which only the bare-name rung or `rust_qualified_call` can produce. Why these globs miss 2a is [U]: the inferred cause, that the specifier does not resolve through a `#[path]` declaration, is not traced. K's step 1 traces it at G0 | `candidate_is_visible_from` only | (a) of I2; the predicate applies |
| `use a::b::helper;` or `use … as h;`, then `helper()` / `h()` | 2a import binding (`:838-868`), then `lookup_in_package` (`dispatch.rs:194-223`) | none | none needed (I2) |
| `mod x;` then `x::f()` | 2b module member (`:944-1000`) | none | none needed (I2) |
| `crate::a::f()`, `self::m::f()`, `super::f()`, `super::super::f()`, `krate::m::f()` | 2d' (`:1348-1387`), `rust_qualified_call` (`rust_modules.rs:426-512`), `rust_exact_module` (`:356-374`), `rust_module_files` (`:282-354`) | none on main | (b) of I2: Hidden gives `Unresolved{reason}` |
| `crate::helper()` through a `pub(crate) use` facade | 2d', then `declaring_site` (`rust_modules.rs:534-545`). That follows `reexport_chains`, and a `pub(crate) use` is not recorded as a re-export (`rust_use_is_public`, `rust_use.rs:589`, accepts bare `pub` only) [C]. So k27 misses [I] | none | resolver-only: follow the facade file's own import binding for the name; it is a `use`, so I2 needs no check |
| `Type::assoc()`, `x.method()` | receiver rungs 1 and 2d | none | out of K's predicate: the lane does not index methods (lane `rust_visibility.rs:116-121`) |

**How main maps 2d' paths today** [C], used in §7:
- `crate::tail` resolves `tail` by file name under `rust_crate_src_root(file)` (`rust_modules.rs:297-305`). That root is the innermost `src` ancestor, else a manifest root, else the literal `"src"` (`:80-104`, fallback at `:102`). It never follows `mod`/`#[path]` edges.
- `crate` alone takes every root candidate in that source directory (`consider_crate_root_files`, `:202-215`). With both `lib.rs` and `main.rs` present that is two files, which is Ambiguous.
- `super` is the directory parent of the call base (`:292-296`) and never a declarer.
- `super::super` with no tail is read as one hop plus a module named `super` (`:310-327`). The loop strips `super::` only when more text follows. [I]: main therefore misses `super::super::f()`. No test pins this.

## 5. Cross-crate edges

- **R5.1 [V: PXC, PXCB, PDEPS, PDEPSN].** A crate names another crate only as a dependency.
  - Library and binaries see `[dependencies]`.
  - Tests and examples also see `[dev-dependencies]` and the package's own library (PXC, PDEPS). Benches follow the same rule, [U] (not probed).
  - The library does not see a dev-dependency (PDEPSN: E0433 "cannot find module or crate `d`"), except when cargo compiles the library as its own unit-test crate: the "lib test" unit in PDEPSN accepted `d`.
  - `build.rs` sees `[build-dependencies]` (PDEPS) and not its own package's library (PXCB: E0433). The library does not see a build-dependency (PDEPSN: E0433 for `bd`).
- **R5.2 [V: R3.6].** Restricted items never cross. Disjoint crate sets therefore prove Hidden for every restricted item, including one with no stored header (ruling 57). Inside overlapping crate sets, an item with no header is Unknown.
  - Ruling 57 count [R]: 3,301 restricted file-level items, 1,035 of them crate-wide for lack of a usable header (BUILD_REPORT, Wave 9 lane K result).
- **R5.3 [C].** The lane's empty-dependency evidence holds:
  - ojas-core's `Cargo.toml` has an empty `[dependencies]` table (read 2026-10-29), and ojas-cpu depends on `ojas-core = { path = "../ojas-core" }`. So no edge from ojas-core into ojas-cpu can be true, whatever the item's visibility.
  - The lane removed 42 ojas edges [R]: 36 from ojas-core into ojas-cpu's `pub(crate)` fns, and 6 `.map` calls bound to ojas-cpu's `pub(crate) fn map`. R5.2 covers all 42, with no dependency fact.
  - A dependency-direction rule would extend the fix to `pub` items, but it needs fact F-R2 (§9), which K does not have.
- **R5.4 [U, policy, not rustc]. Unowned files** (the card's rule):
  - an unowned **callee** stays visible to every caller, which keeps main's answer and covers `include!`'d support files;
  - an unowned **caller** is Hidden from every restricted item of an owned crate.

  rustc says only R1.4: no target the index knows compiles such a file. The way the Hidden half could hide a true edge is a declarer the index cannot see, such as `include!`, generated code or an unindexed file. The first two are out of scope (§10). The third is input U6 in §6. The card's test `an_unowned_caller_does_not_see_another_crates_pub_crate_name` requires the Hidden half.

## 6. The three-valued predicate

`V(caller_file, callee_file, callee_identity)` returns one of `Hidden(reason)`, `Visible` or `Unknown`. It is computed in this order:

1. The callee is not a restricted file-level item, meaning it has no `ModulePrivateScope` mark or it is a method. Return **Unknown**: main's answer, with private items keeping ruling 58's source-root rule.
2. `caller_file == callee_file`: **Visible**.
3. The callee is unowned (R1.7) or falls under U7: **Unknown** (R5.4: an unowned callee keeps main's answer). An unowned callee's empty crate set must never reach step 4.
4. Both crate sets are complete (no input from U4–U6 below) and disjoint: **Hidden** ("restricted items never leave their crate", R3.6). This step holds for every kind, including no header. An unowned **caller** has an empty set, so it is Hidden here unless U6 applies (R5.4, policy).
5. For each crate C in both sets, and each pair of instances in C, decide the reach from the header kind:
   - `Crate`, `InPath(crate)`: Visible.
   - `SelfOnly` (private, `pub(self)`, `pub(in self)`): Visible iff the caller's instance is in T(callee's module). That is, the caller's declarer chain in C reaches the callee file, or the caller is the callee file.
   - `Super`: Visible iff the caller's instance is in T(p) for the parent p of this callee instance (R2.1, R2.7).
   - `InPath(self | super^k | crate::p)`: Visible iff the caller's instance is in T(A), with A from R3.4.
6. **Visible** if any pair is Visible. **Hidden** if every pair in every common crate is decided Hidden. **Unknown** otherwise.

**Inputs that make a verdict Unknown (exhaustive for this design):**
- **U1. No header.** The callee is restricted but no header is stored. Only fns keep one, and only up to `MAX_SIGNATURE_BYTES = 1024` (`rust_use.rs:570-580`, `rustsig.rs:25`) [C]. Structs, enums, traits, consts, statics and type aliases always land here. Unknown applies inside crate overlap only; step 3 still proves Hidden outside it.
- **U2.** One name is marked restricted twice in a file, for example by `cfg` variants (lane `:126`).
- **U3.** The callee file declares inline modules and the kind is `SelfOnly`, `Super` or `InPath`. The extractor files inline-module items under the file namespace, so `symbol_parents` and `declared_at_file_level` (`scope.rs:97-102`) say "file level" for them as well [C] (main relies on this at `rust_modules.rs:436-453`). The item's module, the file or `f::inner`, is unknown (R2.5). `Crate` is unaffected.
- **U4.** The declarer walk hits its bound. The lane's `RUST_DECLARER_WALK_BOUND = 64` (lane `:55`) silently stops (lane `:256`). It must yield Unknown, not "no root found".
- **U5.** A declaration whose target cannot be placed exactly:
  - a `#[path]` inside an inline module, whose specifier omits the inline segments (`langimports/rust.rs:57-60`; R2.4) [C].
  - Not an Unknown input: the lane accepts both bases for a leaf-file `#[path]` (lane `:211-231`). That is a defect, not an Unknown, because by R2.4 the declaring file's directory is the only base.
- **U6.** The caller or the callee is reached by no root, and its package holds a Rust file that is refused by discovery, over the size limit, failed to parse, or parsed with ERROR regions. Any of those could hold the missing `mod` declaration, so the crate set is a lower bound. Membership a walk *found* is still proof; non-membership is not.
- **U7.** (Consulted at step 3.) The callee is undeclared, not a root, and lies under a source root (R1.4: compiled into nothing the index knows). The lane guesses the library's key (lane `:368-386`). The model says Unknown, which is what R5.4's "unowned callee visible" gives anyway.

**Explicitly not Unknown** (ruling 91: main binds these correctly, so abstaining loses true edges):
- `pub(self)` or private, with a caller in T(m): **Visible** (k01, k03).
- A file declared by two roots: each instance is decided on its own, and existence decides (k04, k10, k30).
- **Two parents disagreeing** for one caller: **Visible**. In compiling code the caller's call is legal in every crate that compiles the caller, so a parent under which it would be hidden is a parent in a crate the caller is not in, or a mis-mapped path. Unknown would keep main's bind here too, but the verdict is Visible.

**Outputs.**
- At the bare-name rung, Hidden becomes `Unresolved{reason}` after the single-namesake count. It never promotes another candidate (ruling 56), and `global_candidates` and its cache stay byte-identical. The lane did this at `b1a22348:resolve_all.rs:1605-1614` [C].
- In `rust_qualified_call`, Hidden returns `Some(Resolution::Unresolved { reason })`, never `None`. The lane returned `None` (`b1a22348:rust_modules.rs:474-478`), which lost the reason (ruling 87).
- Visible and Unknown keep main's resolution unchanged.
- The reason names the kind, the item file, both crate-key sets and the caller (the lane's format at `:499-503` suffices).

## 7. Main today and the parked lane, rule by rule

"Main" is `782b9f16` and "lane" is `b1a22348`, where `rust_visibility.rs` is the new file. All entries are [C] unless marked.

| Rule | Main today | Lane | Lane defect against this note |
|---|---|---|---|
| R1.1, R1.7 roots | No root model. `rust_crate_src_root` (`rust_modules.rs:80-104`) keys a file to the innermost `src`, else a manifest root, else literal `"src"`. All of `src/bin/*` share the library's `src` | `rust_root_key` (`:273-307`) and `rust_target_key`/`target_below` (`:320-364`) cover the conventional targets and manifest `lib`/`bin` | No R1.7(b) fallback: `it/main.rs` (custom `[[test]]`) is unowned (`:368-386`), which caused **k20g** |
| R1.3, R1.8 crate set | none | `collect_rust_root_keys` (`:249-266`) stops at the first root, so a root another root declares gets one key | R1.3 instance missed (no attack test); U4 truncation reads as "none" |
| R1.6 `crate` per instance | `consider_crate_root_files` (`:202-215`): every root in the source directory | `InPath("crate")` goes through `rust_exact_module`, hence `consider_crate_root_files`: two roots are Ambiguous, so Hidden | **k10**: `pub(in crate)` must be `Crate` (`parse_rust_item_vis` `:136-162` makes it `InPath("crate")`) |
| R2.1, R2.7 parents | none; `super` is a directory parent (`:292-296`) | `rust_mod_declarer_edges` (`:169-187`) builds the declarer map, but `Super` uses `rust_parent_module_file` (`:421-434`): directory convention, with zero or two candidates giving None | **k04, k30** (two declarers giving two candidates); **k07g** (a `#[path]` parent is not in the directory) |
| R2.4 `#[path]` base | `resolve_import_path` | `rust_mod_decl_target` (`:211-231`) accepts `<leaf>/x.rs` and `<dir>/x.rs` | wrong base for a leaf-file `#[path]` when both exist (no attack test) |
| R2.5 inline | `rust_inline_modules` names only | `Super` widens to the file when the file declares inline modules (`:476-481`) | acceptable as Unknown-equivalent (U3); `SelfOnly`/`InPath` are not widened |
| R3.1 private / `pub(self)` | private: same `rust_crate_src_root` (`candidates.rs:137-138`), so a sibling sees it (over-binds; ruling 58 keeps it). `pub(self)`: treated as exported, visible everywhere | `SelfOnly => false` (`:486`) | **k01, k03, k30**: descendants are lost |
| R3.2 `pub(super)` | visible everywhere | see R2.1 | as R2.1 |
| R3.3 `pub(crate)` | visible everywhere, which caused the 47 wrong corpus edges [R] | `rust_crates_overlap` (`:405-416`) | correct, apart from R1.7(b) |
| R3.4 `pub(in …)` | visible everywhere | `rust_exact_module(item_file, path)` (`:482-485`), a file lookup by spelling | **k11**: `super::super` is misread by `rust_module_files` (`rust_modules.rs:310-327`); R3.4 needs no file lookup |
| R3.7 glob | a resolved glob binds every target symbol at 2a with no visibility test (G-GLOB; `index.rs:1071-1093`). An unresolved glob reaches the bare-name rung [R] | 2a untouched; the predicate applies at the bare-name rung | G-GLOB stays (out of K, ruling 106) |
| R5.2 cross-crate | none | step 3 equivalent | correct; the 47 wrong edges stay removed |
| R5.4 unowned | none | `RUST_UNOWNED_KEY` (`:44-52`), as the card specifies | matches policy |
| §6 qualified output | no check | `None` (`rust_modules.rs:474-478`) | must be `Unresolved{reason}` |
| §6 three values | n/a | two values; `Unknown` is a *header* kind (`:41`) that means "crate-wide" | U1–U7 must keep main's answer, except where step 3 proves Hidden |

## 8. Rule-to-test map

The test file is `rust/devmap-resolve/tests/attack_rust_visibility_k.rs` at `f34f66fb`. The "Fails today" column is [R], from that commit's message; it was not re-run here.
- **lane** means red on the lane and green on main: a lane-introduced loss.
- **both†** means red on both, with the lane also dropping a true edge main binds.
- **both** means red on both: pre-existing. The 13 pre-existing tests are the ft-fff6fb52 set plus k29.
- **none** means green on the lane; **main** marks the ones red on main.

| Test | Rule | Probe | rustc | Fails today |
|---|---|---|---|---|
| k01 | R3.1, R2.7 | P01, A01 | yes | lane |
| k01g | R3.1, R3.7 | P01 (`gl.rs`) | yes | none |
| k02 | R3.1 | A01 (`a3`), PIP-inself | yes | none |
| k03 | R3.1, R2.3 | A01 (`a2`) | yes | lane |
| k04 | R2.1, R3.2, R1.6 | A04 | yes | lane |
| k05 | R2.1, R3.2 | P05 | yes | none |
| k06 | R1.1 test root, R3.2, ft-fff6fb52 | A06 (`common::`), P13 (`crate::common::sup()`) | yes | both |
| k06g | R1.1, R3.2 | P13 (`b.rs`) | yes | none |
| k07 | R2.2, ft-fff6fb52 (`crate::` through `#[path]`) | A07 | yes | both |
| k07g | R2.2, R2.7 | A07G | yes | lane |
| k08 | R1.1 `src/bin/x/main.rs`, ft-fff6fb52 | A08 | yes | both |
| k08g | R1.1 | A08 (qualified form only) | yes | none |
| k09 | R3.2, R2.3 | A09 (positive), P09N (`far.rs` E0603, with `pub mod b`) | yes / no | none (main) |
| k10 | R3.3, R1.6 | A10 | yes | lane |
| k11 | R3.4 | A11 (positive), P11N (`far.rs` E0603) | yes / no | both† |
| k12 | R3.4 | P12, P12N (`ab.rs` E0603) | yes / no | none (main) |
| k13 | R1.1, R1.8 (one module, many roots), ft-fff6fb52 | P13 (two tests; 70 is a scale case) | yes | both |
| k14 | R1.1 build.rs, R2.2, ft-fff6fb52 | P14 | yes | both |
| k14g | R1.1 build.rs | A14G | yes | none |
| k15 | R1.1 build.rs, R2.4, ft-fff6fb52 | P14 (`mod helper;` → `<pkg>/helper.rs`) | yes | both |
| k15g | R1.1 build.rs | P14 (qualified form; glob not compiled) | yes | none |
| k16 | R1.1 example, ft-fff6fb52 | P16 (positive); negative `y.rs` not compiled | yes / [U] | both |
| k16g | R1.1 example | P16 | yes | none (main) |
| k17 | R1.1 nested package, R5.1, ft-fff6fb52 | P17 (positive); negative by R5.1 | yes | both |
| k17g | R1.1 | P17 (positive); negative not compiled | yes / [U] | none (main) |
| k18 | R1.1 `[[bin]] path` | P18 (positive); negative not compiled | yes / [U] | none (main) |
| k19 | R1.1 `[lib] path`, R3.6 | PXC, PXCN (`tests/t.rs` E0603) | yes / no | none (main) |
| k20 | R1.7(b), ft-fff6fb52 | P20Q | yes | both |
| **k20g** | R1.7(b), R1.8 | **A20** (`results.txt`: `av20` and `av20-test-it`, exit 0, `it/main.rs` compiled as `--crate-name it`) | **yes** (new verdict) | lane |
| k21 | R1.1 bench, R2.2, ft-fff6fb52 | P21 | yes | both |
| k21g | R1.1 bench | P21 | yes | none |
| k22 | R1.1 script, R5.4, ft-fff6fb52 | P22 | yes | both |
| k22g | R1.1 script | P22 | yes | none |
| k23 | R5.1 | PXCB (no dependency, no name) | yes / no (R5.1) | none (main) |
| k24 | R1.6, R3.3 | A04, P04T | yes | none |
| k25 | R3.1–R3.3 | P01 (`grand.rs`), A10 | yes | none |
| k26 | R3.6 | PXCN | no | none (main) |
| k27 | §4 facade, ft-fff6fb52 | P27 | yes | both |
| k28 | out of K (methods) | not compiled | [U] | none |
| k29 | R1.8, I4 parity | P13 (after state); before state not compiled | yes | both |
| k30 | R2.1, R2.2, R3.1, R3.2 (composite) | A04, A07, P01, P13 | yes | both† |
| k31 | R1.8, R2.2 | P13 and A07 (not this exact fixture) | yes | none |

**Acceptance set for the K resume (ruling 87):** k01, k03, k04, k07g, k10, k11, k20g and k30 each map to a rule above with a [V] probe. **ft-fff6fb52 set:** k06–k08, k13–k17, k20–k22 and k27. All of them need `crate::` resolved from the caller's crate set (R1.8) by walking declared children (R2.7, `#[path]` included), not file names under `rust_crate_src_root`. k27 also needs §4's facade row.

## 9. Facts

**Stored today and sufficient for K** [C]:
- **`mod` declarations.** Each is an `ExtractedImport` carrying the `raw_import` text and the specifier `self::a::b`, with inline ancestors included, or `self::<path>` for `#[path]` (`langimports/rust.rs:34-85`). These give `parents(f)` (R2.7).
- **The restricted marker.** `WiringKind::ModulePrivateScope` is set on every symbol whose node carries a `visibility_modifier` starting `pub(` (`lang_helpers.rs:1288-1318`), methods and non-fn items included.
- **The kind.** It is parsed from the fn header in `rust_signatures` (`index.rs:138-145`; header from `rust_fn_signature`, `rust_use.rs:570-580`).
- **Non-`pub` items.** These are in `unexported_symbols` (`index.rs:146-152`).
- **Inline module names.** `Extraction::rust_inline_modules` (`model.rs:1675`) holds names only, and is enough to detect U3.
- **Manifest roots.** `RustCrateManifest { dir, names, lib_path, bin_paths }` (`gomod.rs:207-222`). This is the manifest pass's output, re-read on every build (`devmap-cli/src/commands/build.rs:354`; `devmap-store/src/db/generation.rs:434, 1053`), not part of the extraction payload.

So K is **resolver-only**. R1.7(b) stands in for the missing `[[test]]`/`[[example]]`/`[[bench]]` paths whenever the custom root declares a module. A custom root that is a single file declares no modules, so:
- as a callee, its restricted items reach only itself, which step 2 of §6 handles;
- as a caller, it is unowned, and R5.4 applies.

**Not stored.** None of these is needed for K.

| # | Fact | Where it would live | Shrinks | Gate |
|---|---|---|---|---|
| F-R1 | `[[test]]`/`[[example]]`/`[[bench]]` paths, `package.build` | `RustCrateManifest` (manifest pass) | the reliance on R1.7(b) | [I]: not extract schema 98, since manifests are not in the payload. But store freshness compares only manifest *refusals* (`generation.rs:1066`), so a `Cargo.toml` edit would not mark the store stale. Owner decision |
| F-R2 | `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` names | `RustCrateManifest` | cross-crate `pub` edges with the dependency direction wrong (R5.3) | same as F-R1 |
| F-R3 | the byte range of each inline module, or each item's inline module path | extraction payload | U3 | **extractor change, gated (schema 98 is owner-controlled)** |
| F-R4 | the inline segments in a `#[path]` specifier inside an inline module | extraction payload (`langimports/rust.rs:57-60`) | U5 | **extractor change, gated** |
| F-R5 | the visibility text of non-fn restricted items, and of fn headers over 1,024 bytes | extraction payload | U1 (1,035 crate-wide items, ruling 57) | **extractor change, gated** |

## 10. Out of scope (named, not modelled)

- **Card ft-1b21e9b6 part 2.** The glob and re-export gains (`bind_published_rust_reexports`, 596 and 658 true edges) and their five listed defects. This is the six-BLOCK family that ruling 18 cut. Only R3.7, that a glob never imports a hidden item, is stated here.
- **Gap G-GLOB (ruling 106).** Main's rung 2a binds every symbol of a resolved glob's target, with no visibility test (`index.rs:1071-1093`). It is a pre-existing wrong-edge class that belongs to part 2, and K neither fixes nor widens it.
- **Macro-generated items and module inclusion by macro:** `macro_rules!` and proc-macro output, `include!`, `#[cfg_attr(…, path = …)]`, and `cfg`-selected `mod` declarations. U2 covers only a name marked twice in one file.
- **Methods and associated items** (k28), which keep main's answer.
- **Edition 2015** `pub(in path)` without a leading keyword: rejected in 2018 and later (R3.4); 2015 was not probed [U].

## 11. Provenance

- **DevMap.** The CLI (`devmap explore`, `devmap search`, build `782b9f168833-dirty`, extract schema 98) located `rust_crate_src_root`, `rust_qualified_call`, `lookup_in_package`, `candidate_is_visible_from`, `bare_name_has_other_evidence`, `names_a_non_local_import`, `declared_at_file_level`, `index_project_manifests` and `RustCrateManifest`. The MCP server is `binary_replaced` and was not used.
- **DevMap gaps.**
  - `rust_crate_roots` (a field), `UniqueGlobal` and `ModulePrivateScope` (enum variants) answered "outside index". They were located with `rg` and read from source. This is recorded as gap `rust-vis-note-fields-variants`.
  - `explore` reports "callers 0 of 0" for `bare_name_has_other_evidence` and `names_a_non_local_import`, though `resolve_all.rs:1563` and `candidates.rs:245` call them. This is recorded as gap `rust-vis-note-zero-callers`.
  - DevMap has no rung-order query, so the rung order in §4 was read from `resolve_all.rs`.
  - `rust_mod_declarers` is not on main at all: it exists only at `b1a22348:rust/devmap-resolve/src/resolver.rs:372`. DevMap's "outside index" answer for it was therefore correct.
- **GitPulse.** `gitpulse_insights`: 8 worktrees, 0 collisions, 1 live session. Its codeintel facet reports main's store `is_fresh=false` ("stored extraction payload is obsolete"). That is the 97/98 reader mismatch the owner must resolve. No store was rebuilt.
- **Read with `git show` only, without checking anything out:** `b1a22348` (lane), `f34f66fb` (attack), `docs/swift-lookup-rules` (format model). Cards: ft-1b21e9b6 revision 2 and ft-fff6fb52 revision 1.

## 12. Where rustc or the code contradicts the rulings or the card text

1. **Root definition** (ruling 87 item 5 and card ft-fff6fb52: "an indexed file that no `mod` declares and that itself declares modules"). rustc rejects it as a definition in both directions:
   - a root need not declare modules (R1.2: A10's `main.rs`, PBINS);
   - a root can be declared (R1.3: PLIBMOD);
   - an undeclared file is not thereby a root (R1.4: PSTRAY).

   It survives only as the fallback R1.7(b), behind the target-path rule. **Ruling 107 adopts R1.7 in place of ruling 87 item 5.**
2. **Globs (corrected by ruling 106).** The draft said a glob of an indexed module reaches the bare-name rung. Main refutes that:
   - a resolved glob binds at rung 2a (`rust_use.rs:437-441`, `index.rs:1071-1093`, `rust_module_paths.rs:232-252`), so card ft-fff6fb52's "`use x::*` binds through the import rung" is right for a resolved glob;
   - what that binding lacks is a visibility test (G-GLOB, out of K).

   Only a glob that does not resolve to an indexed file reaches the bare-name rung [R] (§4 row 3). The attack header (`f34f66fb:…/attack_rust_visibility_k.rs:14-16`) and its k05/k20g doc comments each describe one of the two cases.
3. **Ruling 87, "parents come from `rust_mod_declarers`" (replaced by ruling 107).** That map exists only on the lane. The parents now come from a declarer map the resume builds on main, and a hop out of an inline module goes to the enclosing module, which no declarer map holds (R2.5; U3).
4. **Ruling 87, "`super::super` is two declarer hops."** This is right for file modules (A11). A hop out of an inline module goes to the enclosing module (R2.5). In `pub(in …)` the hops never pass a root (R2.6) and need no file lookup (R3.4).
5. **Two disagreeing parents give Visible (ruling 107).** rustc decides each instance separately (R2.1). In compiling code, disagreement means the hidden instance lives in a crate the caller is not in. §6 makes it Visible.
6. **Card ft-1b21e9b6, "an unowned callee stays visible to all."** This is resolver policy and has no rustc basis (R1.4). It is kept as [U] policy (R5.4).
7. **Rulings 86 and 87 otherwise match rustc:** `pub(self)` is the module tree (R3.1), parents are per declarer (R2.1), and `pub(in crate)` is `Crate` (R3.3).

**Count.** The note states 28 numbered rules (R1.1–R1.8, R2.1–R2.7, R3.1–R3.9, R5.1–R5.4).
- **[V]: 26.** R1.1–R1.6, R1.7(a), R1.8, R2.1–R2.7, R3.1–R3.9, R5.1 and R5.2.
- **[U]: 2 whole rules,** R1.7(b) and R5.4, both resolver policy.
- **3 [U] sub-clauses inside [V] rules:** the unprobed target shapes in R1.1, the bench dependency scope in R5.1, and edition 2015 (§10).
- R5.3 is a code and manifest fact [C], not a rustc rule.
- §6 adds no rule of its own. Each step and each Unknown input cites the rule it rests on.

[U] rows are not binding.

---

## Appendix A. Probes (rustc 1.99.0, edition 2021)

Each block lists file paths as comments. Packages use `[package] name = "<id>" version = "0.1.0" edition = "2021"` unless the block shows extra manifest lines.

**A01**: compiles. Covers R3.1 and R2.3.
```rust
// src/lib.rs
pub mod a; pub mod a2; pub mod a3;
// src/a/mod.rs
mod child;
pub(self) fn helper() -> i32 { 1 }
// src/a/child.rs
pub fn go() -> i32 { super::helper() }
// src/a2.rs          (leaf parent)
mod child;
pub(self) fn helper() -> i32 { 1 }
// src/a2/child.rs
pub fn go() -> i32 { super::helper() }
// src/a3/mod.rs
mod child;
pub(in self) fn helper() -> i32 { 1 }
// src/a3/child.rs
use super::*;
pub fn go() -> i32 { helper() }
```
**P01**: compiles. Covers R3.1: a grandchild, a glob, and private equal to `pub(self)`.
```rust
// src/lib.rs
pub mod a;
// src/a/mod.rs
mod child;
mod gl;
pub(self) fn helper() -> i32 { 1 }
fn private_helper() -> i32 { 2 }
// src/a/child.rs
mod grand;
pub fn go() -> i32 { super::helper() + super::private_helper() }
// src/a/child/grand.rs
pub fn go() -> i32 { crate::a::helper() + super::super::helper() + crate::a::private_helper() }
// src/a/gl.rs
use super::*;
pub fn go() -> i32 { helper() + private_helper() }
```
**P01N**: E0603 twice ("function `helper` is private", "function `private_helper` is private").
```rust
// src/lib.rs
pub mod a;
pub mod b;
// src/a.rs
pub(self) fn helper() -> i32 { 1 }
fn private_helper() -> i32 { 2 }
// src/b.rs
pub fn go() -> i32 { crate::a::helper() + crate::a::private_helper() }
```
**A04**: compiles. **P05**: compiles (the glob form, with `mod util; use util::*;` and a bare `helper()` in both roots).
```rust
// src/lib.rs
mod util;
pub fn from_lib() -> i32 { crate::util::helper() }
// src/main.rs
mod util;
fn main() { println!("{}", crate::util::helper()); }
// src/util.rs
pub(super) fn helper() -> i32 { 1 }
```
**P04T**: E0308 "expected `p04t::util::S`, found `util::S`". Covers R1.6.
```rust
// src/lib.rs
pub mod util;
// src/main.rs
mod util;
fn main() { let _x: p04t::util::S = crate::util::S; }
// src/util.rs
pub struct S;
```
**P04P**: compiles. **P04PN**: P04P plus one line in `lib.rs`, which gives E0603 at `a::s::helper`. Covers R2.1 and R3.2.
```rust
// src/lib.rs
mod shared;
pub mod a;
pub fn f() -> i32 { shared::helper() }
pub fn h() -> i32 { a::s::helper() }      // P04PN only
// src/shared.rs
pub(super) fn helper() -> i32 { 1 }
// src/a/mod.rs
#[path = "../shared.rs"]
pub mod s;
pub fn g() -> i32 { s::helper() }
```
**A06**: compiles.
```rust
// src/lib.rs
pub fn lib_fn() {}
// tests/common/mod.rs
pub(super) fn helper() -> i32 { 1 }
// tests/a.rs
mod common;
#[test]
fn t() { assert_eq!(common::helper(), 1); }
```
**A07**: compiles. **A07G**: compiles with `use x::*;` and `pub fn go() -> i32 { helper() }` in place of the qualified call.
```rust
// src/lib.rs
#[path = "impls/x_impl.rs"]
mod x;
pub fn go() -> i32 { crate::x::helper() }
// src/impls/x_impl.rs
pub(super) fn helper() -> i32 { 1 }
```
**A08**: compiles.
```rust
// src/lib.rs
pub fn lib_fn() {}
// src/bin/x/main.rs
mod util;
fn main() { println!("{}", crate::util::tool()); }
// src/bin/x/util.rs
pub(super) fn tool() -> i32 { 1 }
```
**A09**: compiles. **P09N**: E0603 at `far.rs`.
```rust
// A09 src/lib.rs: pub mod a;          A09 src/a.rs: mod b; mod c;
//   pub fn go() -> i32 { crate::a::b::helper() }
// A09 src/a/b.rs:  pub(super) fn helper() -> i32 { 1 }
// A09 src/a/c.rs:  pub fn go2() -> i32 { super::b::helper() }
// P09N src/lib.rs: pub mod a; pub mod far;   P09N src/a.rs: pub mod b;
// P09N src/a/b.rs: pub(super) fn helper() -> i32 { 1 }
// P09N src/far.rs: pub fn go3() -> i32 { crate::a::b::helper() }
```
**A10**: compiles. Covers R3.3 and R1.2.
```rust
// src/lib.rs
pub mod util; pub mod user;
// src/util.rs
pub(in crate) fn helper() -> i32 { 1 }
// src/user.rs
pub fn go() -> i32 { crate::util::helper() }
// src/main.rs
fn main() {}
```
**A11**: compiles. **P11N**: the same tree plus `pub mod far;` in `lib.rs` and `src/far.rs` with `pub fn go2() -> i32 { crate::a::b::c::helper() }`, which gives E0603.
```rust
// src/lib.rs
pub mod a;
// src/a/mod.rs
pub mod b; pub mod x;
// src/a/b/mod.rs
pub mod c;
// src/a/b/c.rs
pub(in super::super) fn helper() -> i32 { 1 }
// src/a/x.rs
pub fn go() -> i32 { crate::a::b::c::helper() }
```
**P12**: compiles. **P12N**: E0603 at `ab.rs`.
```rust
// P12  src/lib.rs: pub mod a;    src/a.rs: pub mod b; pub mod c;
// P12  src/a/b.rs: pub(in crate::a) fn helper() -> i32 { 1 }
// P12  src/a/c.rs: pub fn go() -> i32 { crate::a::b::helper() }
// P12N src/lib.rs: pub mod a; pub mod ab;    src/a.rs: pub mod b;
// P12N src/a/b.rs: pub(in crate::a) fn helper() -> i32 { 1 }
// P12N src/ab.rs:  pub fn go2() -> i32 { crate::a::b::helper() }
```
**P13**: compiles. Covers R1.1 test roots, R3.2 and R1.8.
```rust
// src/lib.rs
pub fn lib_fn() {}
// tests/common/mod.rs
pub(crate) fn shared_helper() -> i32 { 1 }
pub(super) fn sup() -> i32 { 2 }
// tests/a.rs
mod common;
#[test]
fn t() { assert_eq!(crate::common::shared_helper() + crate::common::sup(), 3); }
// tests/b.rs
mod common;
use common::*;
#[test]
fn t() { assert_eq!(shared_helper() + sup(), 3); }
```
**P14**: compiles. Covers build.rs as a root, `#[path]`, and `mod helper;`.
```rust
// src/lib.rs
mod config;
pub fn lib_go() -> i32 { crate::config::load() }
// src/config.rs
pub(crate) fn load() -> i32 { 1 }
// build.rs
#[path = "src/config.rs"]
mod config;
mod helper;
fn main() { let _ = crate::config::load() + crate::helper::generate(); }
// helper.rs
pub(crate) fn generate() -> i32 { 1 }
```
**A14G**: compiles. In `build.rs`: `#[path = "src/config.rs"] mod config; use config::*; fn main() { load(); } mod helper;`. The rest is as in P14.

**P16**: compiles.
```rust
// src/lib.rs
pub fn lib_fn() {}
// examples/x/main.rs
mod util;
use util::*;
fn main() { let _ = crate::util::demo() + demo(); }
// examples/x/util.rs
pub(crate) fn demo() -> i32 { 1 }
```
**P17**: compiles (checked from `outer/inner/Cargo.toml`; `outer` is a plain package with no `[workspace]`).
```rust
// outer/src/lib.rs
pub fn outer_fn() {}
// outer/inner/src/lib.rs
pub mod util;
// outer/inner/src/util.rs
pub(crate) fn inner_helper() -> i32 { 1 }
// outer/inner/tests/t.rs
mod common;
#[test]
fn t() { assert_eq!(crate::common::test_helper(), 1); }
// outer/inner/tests/common/mod.rs
pub(crate) fn test_helper() -> i32 { 1 }
```
**P18**: compiles. Manifest: `[[bin]] name = "cli" path = "src/cli/main.rs"`.
```rust
// src/lib.rs
pub mod api;
// src/api.rs
pub fn go() {}
// src/cli/main.rs
mod args;
fn main() { println!("{}", crate::args::parse_args()); }
// src/cli/args.rs
pub(crate) fn parse_args() -> i32 { 1 }
```
**A20 (k20g)**: compiles. The runner also checks the custom target alone, `cargo check --test it -v` (`results.txt` line `av20-test-it`), and counts one `rustc --crate-name it --edition=2021 it/main.rs … --test` invocation, finishing with exit 0. **P20Q (k20)**: compiles, with the same manifest and `#[test] fn t() { assert_eq!(crate::util::helper(), 1); }` in `it/main.rs`, and no glob.
```rust
// Cargo.toml extra:  [[test]] name="it" path="it/main.rs"
// src/lib.rs
mod util;
pub fn lib_go() -> i32 { crate::util::helper() }
// src/util.rs
pub(crate) fn helper() -> i32 { 1 }
// it/main.rs
#[path = "../src/util.rs"]
mod util;
use util::*;
#[test]
fn t() { assert_eq!(helper(), 1); }
```
**P21**: compiles. Manifest: `[[bench]] name = "b" harness = false`. `--benches -v` shows `--crate-name b … benches/b.rs`.
```rust
// src/lib.rs
mod util;
pub fn lib_go() -> i32 { crate::util::helper() }
// src/util.rs
pub(crate) fn helper() -> i32 { 1 }
// benches/b.rs
#[path = "../src/util.rs"]
mod util;
use util::*;
fn main() { let _ = helper() + crate::util::helper(); }
```
**P22**: compiles (`rustc --edition 2021 --emit=metadata scripts/gen/main.rs`).
```rust
// scripts/gen/main.rs
mod helpers;
use helpers::*;
fn main() { let _ = crate::helpers::emit() + emit(); }
// scripts/gen/helpers.rs
pub(crate) fn emit() -> i32 { 1 }
```
**P27**: compiles.
```rust
// src/lib.rs
mod a;
pub(crate) use a::b::helper;
pub mod user;
// src/a/mod.rs
pub mod b;
// src/a/b.rs
pub(crate) fn helper() -> i32 { 1 }
// src/user.rs
pub fn go() -> i32 { crate::helper() }
```
**PINL**: compiles. **PINLN**: E0603 "function `x` is private" at `lib.rs`.
```rust
// PINL src/lib.rs
pub mod f;
// PINL src/f.rs
pub mod inner {
    pub(super) fn x() -> i32 { 1 }
    pub(self) fn y() -> i32 { 2 }
    pub fn z() -> i32 { y() }
}
pub fn go() -> i32 { inner::x() + inner::z() }
// PINLN src/lib.rs
pub mod f;
pub fn up() -> i32 { crate::f::inner::x() }
// PINLN src/f.rs
pub mod inner { pub(super) fn x() -> i32 { 1 } }
pub fn go() -> i32 { inner::x() }
```
**PXC**: compiles. **PXCN**: four E0603 errors: `helper` is private at `src/main.rs`, `tests/t.rs` and `examples/e.rs`, plus "module `hidden` is private" at `src/main.rs`. Manifest for both: `[lib] path = "core/lib.rs"`.
```rust
// core/lib.rs
pub mod a;
pub mod b;
mod hidden;
// core/a.rs
pub(crate) fn helper() -> i32 { 1 }
pub fn open() -> i32 { 1 }
// core/b.rs
pub fn go() -> i32 { crate::a::helper() }
// core/hidden.rs
pub fn behind() -> i32 { 1 }
// PXC  src/main.rs, examples/e.rs: fn main() { println!("{}", pxc::a::open()); }
// PXC  tests/t.rs: #[test] fn t() { assert_eq!(pxc::a::open(), 1); }
// PXCN src/main.rs: fn main() { println!("{}", pxcn::a::helper() + pxcn::hidden::behind()); }
// PXCN tests/t.rs:  #[test] fn t() { assert_eq!(pxcn::a::helper(), 1); }
// PXCN examples/e.rs: fn main() { println!("{}", pxcn::a::helper()); }
```
**PXCB**: E0433 "cannot find module or crate `pxcb`" at `build.rs`.
```rust
// src/lib.rs
pub fn open() -> i32 { 1 }
// build.rs
fn main() { pxcb::open(); }
```
**PBINS**: compiles. **PBINSN**: E0425 at `src/bin/two.rs` (`crate::only_one`) and at `tests/b.rs` (`crate::only_a`).
```rust
// PBINS src/bin/one.rs
pub(crate) fn only_one() -> i32 { 1 }
fn main() { println!("{}", crate::only_one()); }
// PBINS src/bin/dir/main.rs:  mod part; fn main() { println!("{}", crate::part::p()); }
// PBINS src/bin/dir/part.rs:  pub(crate) fn p() -> i32 { 1 }
// PBINS tests/a.rs:  pub(crate) fn only_a() -> i32 { 1 }  #[test] fn t() { assert_eq!(crate::only_a(), 1); }
// PBINS tests/d/main.rs:  mod part; #[test] fn t() { assert_eq!(crate::part::p(), 1); }
// PBINS tests/d/part.rs:  pub(crate) fn p() -> i32 { 1 }
// PBINSN src/bin/two.rs:  fn main() { println!("{}", crate::only_one()); }
// PBINSN tests/b.rs:      #[test] fn t() { assert_eq!(crate::only_a(), 1); }
// (PBINSN also holds src/lib.rs, src/bin/one.rs and tests/a.rs as in PBINS.)
```
**PLIBMOD**: the library compiles; the test crate gives E0425 "cannot find function `inner` in the crate root" at `tests/../src/lib.rs:2`. Covers R1.3 and R1.6.
```rust
// src/lib.rs
pub(crate) fn inner() -> i32 { 1 }
pub fn f() -> i32 { crate::inner() }
// tests/t.rs
#[path = "../src/lib.rs"]
mod l;
#[test]
fn t() { assert_eq!(l::inner() + l::f() + plibmod::f(), 3); }
```
**PSTRAY**: compiles. Covers R1.4.
```text
// src/lib.rs:    pub fn f() {}
// src/stray.rs:  this is not rust at all {{{
```
**PGLOBN**: E0425 "cannot find function `helper` in this scope" at `far.rs`. `open()` resolves. Covers R3.7.
```rust
// src/lib.rs:  pub mod a; pub mod far;        src/a.rs: pub mod b;
// src/a/b.rs:  pub(super) fn helper() -> i32 { 1 }  pub fn open() -> i32 { 2 }
// src/far.rs:  use crate::a::b::*;  pub fn go() -> i32 { open() + helper() }
```
**PPATH**: compiles. Each `which()` exists only in the file that rustc must load, and the decoy files define only `not_this`. Covers R2.4.
```rust
// src/lib.rs
pub mod a;
pub mod b;
// src/a.rs                        (leaf file)
#[path = "x.rs"]
pub mod m;                          // loads src/x.rs
pub mod inl {
    #[path = "y.rs"]
    pub mod m;                      // loads src/a/inl/y.rs
    pub mod z;                      // loads src/a/inl/z.rs
}
pub fn go() -> i32 { m::which() + inl::m::which() + inl::z::which() }
// src/x.rs:        pub fn which() -> i32 { 1 }
// src/a/x.rs:      pub fn not_this() {}
// src/a/inl/y.rs:  pub fn which() -> i32 { 3 }
// src/inl/y.rs:    pub fn not_this() {}
// src/a/inl/z.rs:  pub fn which() -> i32 { 4 }
// src/b/mod.rs:    pub mod inl { #[path = "y.rs"] pub mod m; }  pub fn go() -> i32 { inl::m::which() }
// src/b/inl/y.rs:  pub fn which() -> i32 { 5 }
```
**PIP** (single-file libs):
```rust
// nonancestor.rs: E0742 "visibilities can only be restricted to ancestor modules"
pub mod a { pub(in crate::b) fn f() {} }
pub mod b {}
// child.rs: E0742
pub mod a { pub mod x {} pub(in self::x) fn f() {} }
// nokeyword.rs: error "relative paths are not supported in visibilities in 2018 edition or later"
pub mod a { pub(in a) fn f() {} }
// rootsuper.rs: E0433 "too many leading `super` keywords"
pub(super) fn f() {}
// inself.rs: compiles
pub mod a { pub(in self) fn f() {} }
// space.rs: compiles
pub mod a { pub (crate) fn f() -> i32 { 1 } }
pub fn g() -> i32 { a::f() }
```
**PTRAIT**: compiles. Covers R3.9.
```rust
pub mod a {
    pub struct S;
    pub trait T { fn m(&self) -> i32; }
    impl T for S { fn m(&self) -> i32 { 1 } }
}
pub fn g() -> i32 { use a::T; a::S.m() }
```
**PDEPS**: compiles. **PDEPSN**: the plain library gives E0433 for both `d` and `bd`; the "lib test" unit gives E0433 for `bd` only. Packages `d` (`pub fn dfn() -> i32 { 1 }`) and `bd` (`pub fn bdfn() -> i32 { 1 }`) are path crates.
```rust
// top/Cargo.toml extra:  [dev-dependencies] d = { path = "../d" }   [build-dependencies] bd = { path = "../bd" }
// top/build.rs:        fn main() { let _ = bd::bdfn(); }
// top/src/lib.rs:      pub fn top_fn() -> i32 { 1 }
// top/tests/t.rs:      #[test] fn t() { assert_eq!(d::dfn() + top::top_fn(), 2); }
// top/examples/e.rs:   fn main() { let _ = d::dfn(); }
// topn/src/lib.rs (same manifest tables):  pub fn top_fn() -> i32 { d::dfn() + bd::bdfn() }
```

## Appendix B. `results.txt` (probe runner output, verbatim)

Probe ids are lowercase here: `avNN` is ANN, and `pxxx` is PXXX. PDEPS and PDEPSN appear as `pdeps-top` and `pdeps-topn`. `av20-test-it` is a second, targeted run of A20, so the 51 result lines cover 50 probes.

```text
rustc 1.99.0 (b940084d7 2026-09-28)
cargo 1.99.0 (5f94df478 2026-08-27)
av01 exit=0 errors=0 codes=[] first=[]
av04 exit=0 errors=0 codes=[] first=[]
av06 exit=0 errors=0 codes=[] first=[]
av07 exit=0 errors=0 codes=[] first=[]
av07g exit=0 errors=0 codes=[] first=[]
av08 exit=0 errors=0 codes=[] first=[]
av09 exit=0 errors=0 codes=[] first=[]
av10 exit=0 errors=0 codes=[] first=[]
av11 exit=0 errors=0 codes=[] first=[]
av14g exit=0 errors=0 codes=[] first=[]
av20 exit=0 errors=0 codes=[] first=[]
p01 exit=0 errors=0 codes=[] first=[]
p01n exit=101 errors=4 codes=[ 2 error[E0603];] first=[error[E0603]: function `helper` is private]
p04p exit=0 errors=0 codes=[] first=[]
p04pn exit=101 errors=3 codes=[ 1 error[E0603];] first=[error[E0603]: function `helper` is private]
p04t exit=101 errors=3 codes=[ 1 error[E0308];] first=[error[E0308]: mismatched types]
p05 exit=0 errors=0 codes=[] first=[]
p09n exit=101 errors=3 codes=[ 1 error[E0603];] first=[error[E0603]: function `helper` is private]
p11n exit=101 errors=3 codes=[ 1 error[E0603];] first=[error[E0603]: function `helper` is private]
p12 exit=0 errors=0 codes=[] first=[]
p12n exit=101 errors=3 codes=[ 1 error[E0603];] first=[error[E0603]: function `helper` is private]
p13 exit=0 errors=0 codes=[] first=[]
p14 exit=0 errors=0 codes=[] first=[]
p16 exit=0 errors=0 codes=[] first=[]
p18 exit=0 errors=0 codes=[] first=[]
p20q exit=0 errors=0 codes=[] first=[]
p21 exit=0 errors=0 codes=[] first=[]
p27 exit=0 errors=0 codes=[] first=[]
pinl exit=0 errors=0 codes=[] first=[]
pinln exit=101 errors=3 codes=[ 1 error[E0603];] first=[error[E0603]: function `x` is private]
pxc exit=0 errors=0 codes=[] first=[]
pxcn exit=101 errors=8 codes=[ 4 error[E0603];] first=[error[E0603]: function `helper` is private]
pxcb exit=101 errors=2 codes=[ 1 error[E0433];] first=[error[E0433]: cannot find module or crate `pxcb` in this scope]
ppath exit=0 errors=0 codes=[] first=[]
pstray exit=0 errors=0 codes=[] first=[]
pglobn exit=101 errors=3 codes=[ 1 error[E0425];] first=[error[E0425]: cannot find function `helper` in this scope]
plibmod exit=101 errors=2 codes=[ 1 error[E0425];] first=[error[E0425]: cannot find function `inner` in the crate root]
pbins exit=0 errors=0 codes=[] first=[]
pbinsn exit=101 errors=5 codes=[ 2 error[E0425];] first=[error[E0425]: cannot find function `only_a` in the crate root]
av20-test-it exit=0 errors=0 codes=[] first=[]
av20-test-it rustc-invocations-of-it/main.rs=1
pdeps-top exit=0 errors=0 codes=[] first=[]
pdeps-topn exit=101 errors=5 codes=[ 3 error[E0433];] first=[error[E0433]: cannot find module or crate `bd` in this scope]
ptrait exit=0 errors=0 codes=[] first=[]
p17 exit=0 errors=0 codes=[] first=[]
p22 exit=0 errors=0 codes=[] first=[]
pip-nonancestor exit=1 errors=2 codes=[ 1 error[E0742];] first=[error[E0742]: visibilities can only be restricted to ancestor modules]
pip-nokeyword exit=1 errors=2 codes=[] first=[error: relative paths are not supported in visibilities in 2018 edition or later]
pip-rootsuper exit=1 errors=2 codes=[ 1 error[E0433];] first=[error[E0433]: too many leading `super` keywords within `crate`]
pip-child exit=1 errors=2 codes=[ 1 error[E0742];] first=[error[E0742]: visibilities can only be restricted to ancestor modules]
pip-space exit=0 errors=0 codes=[] first=[]
pip-inself exit=0 errors=0 codes=[] first=[]
```
