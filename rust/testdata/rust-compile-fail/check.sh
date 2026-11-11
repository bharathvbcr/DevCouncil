#!/bin/sh
# The rustc facts the Rust macro-silence rules rely on, as a record that can be
# re-run: `sh check.sh`. Each case is one small lib crate compiled with plain
# rustc (no cargo, no dependency fetched). A case either compiles ("ok") or
# fails with the first `error` line quoted below; the script says PASS when
# rustc still agrees with the record.
#
# Recorded with rustc 1.99.1 (58cf8f9c3 2026-10-13); last run under the
# toolchain rust-toolchain.toml pins (see the PASS count the run prints).
# Edition 2021. `cargo test -p devmap-resolve --test compile_fail_record -- --ignored`
# runs it (an #[ignore]d test: it needs rustc and is not part of the default suite).
#
# Which rule relies on which case (`rust_macro_is_silent` in
# rust/devmap-resolve/src/resolver/rust_site_scope.rs):
#   std_prelude_*      a glob cannot rebind a std prelude macro      -> skip the glob check
#   nostd_*, noprel_*  ... unless the implicit prelude is absent    -> do not skip it
#   explicit_*         an explicit import shadows a glob             -> skip the glob check
#   pathed_root_*      a glob cannot rebind the root of `a::b!`      -> pathed calls ignore globs
#   derive_*           a glob cannot rebind a std derive             -> only an explicit `use` can
set -u
here=$(cd "$(dirname "$0")" && pwd)
work=${TMPDIR:-/tmp}/devmap-compile-fail.$$
mkdir -p "$work" && cd "$work" || exit 2
rustc --version
fail=0

# kio exports a macro under the names of the std ones; each emits a harmless item.
names="println print eprint eprintln dbg vec format format_args assert assert_eq assert_ne debug_assert debug_assert_eq debug_assert_ne panic matches write writeln todo unimplemented unreachable thread_local"
: > kio.rs
for n in $names; do
  printf '#[macro_export] macro_rules! %s { ($($t:tt)*) => { pub fn from_kio() {} }; }\n' "$n" >> kio.rs
done
rustc --edition 2021 --crate-type lib --crate-name kio kio.rs || exit 2

# kdep is an ordinary dependency whose `info!` a glob-imported module of the same name could shadow.
printf '#[macro_export] macro_rules! info { ($($t:tt)*) => { pub fn from_kdep() {} }; }\n' > kdep.rs
rustc --edition 2021 --crate-type lib --crate-name kdep kdep.rs || exit 2

# kder is a proc-macro crate with a derive named like a std one.
cat > kder.rs <<'EOF'
extern crate proc_macro;
use proc_macro::TokenStream;
#[proc_macro_derive(Clone)]
pub fn c(_: TokenStream) -> TokenStream { "pub fn from_derive() {}".parse().unwrap() }
EOF
rustc --edition 2021 --crate-type proc-macro --crate-name kder kder.rs || exit 2
kder=$(ls libkder.* | head -1)

# case NAME EXPECTED SOURCE: EXPECTED is "ok" or a prefix of the first error line.
case_() {
  name=$1; expected=$2
  printf '%s\n' "$3" > "$name.rs"
  out=$(rustc --edition 2021 --crate-type lib -L . --extern kio=libkio.rlib --extern kdep=libkdep.rlib --extern kder="$kder" "$name.rs" 2>&1)
  first=$(printf '%s\n' "$out" | grep -m1 '^error')
  if [ "$expected" = ok ]; then
    [ -z "$first" ] && echo "PASS $name (compiles)" || { echo "FAIL $name: expected ok, got $first"; fail=1; }
  else
    case "$first" in
      "$expected"*) echo "PASS $name ($first)";;
      *) echo "FAIL $name: expected $expected, got ${first:-ok}"; fail=1;;
    esac
  fi
}

# --- a glob against the std prelude -------------------------------------------------
case_ std_prelude_println 'error[E0659]: `println` is ambiguous' 'use kio::*;
println!{}'
case_ std_prelude_vec 'error[E0659]: `vec` is ambiguous' 'use kio::*;
vec!{}'
case_ std_prelude_assert 'error[E0659]: `assert` is ambiguous' 'use kio::*;
assert!{}'
for n in print eprint eprintln dbg format format_args assert_eq assert_ne debug_assert debug_assert_eq debug_assert_ne panic matches write writeln todo unimplemented unreachable thread_local; do
  case_ "std_prelude_$n" "error[E0659]: \`$n\` is ambiguous" "use kio::*;
$n!{}"
done

# --- ... unless the implicit prelude is absent --------------------------------------
case_ nostd_println ok '#![no_std]
use kio::*;
println!{}'
case_ nostd_dbg ok '#![no_std]
use kio::*;
dbg!{}'
case_ nostd_thread_local ok '#![no_std]
use kio::*;
thread_local!{}'
case_ nostd_vec ok '#![no_std]
use kio::*;
vec!{}'
for n in print eprint eprintln format; do
  case_ "nostd_$n" ok "#![no_std]
use kio::*;
$n!{}"
done
case_ nostd_format_with_macro_use_alloc 'error[E0659]: `format` is ambiguous' '#![no_std]
#[macro_use] extern crate alloc;
use kio::*;
format!{}'
case_ nostd_println_with_macro_use_alloc ok '#![no_std]
#[macro_use] extern crate alloc;
use kio::*;
println!{}'
case_ nostd_vec_with_macro_use_alloc 'error[E0659]: `vec` is ambiguous' '#![no_std]
#[macro_use] extern crate alloc;
use kio::*;
vec!{}'
case_ nostd_println_with_macro_use_std 'error[E0659]: `println` is ambiguous' '#![no_std]
#[macro_use] extern crate std;
use kio::*;
println!{}'
for core_macro in assert assert_eq assert_ne debug_assert debug_assert_eq debug_assert_ne panic matches write writeln todo unimplemented unreachable format_args; do
  case_ "nostd_core_$core_macro" "error[E0659]: \`$core_macro\` is ambiguous" "#![no_std]
use kio::*;
$core_macro!{}"
done
case_ noprel_println ok '#![no_implicit_prelude]
extern crate kio;
use kio::*;
println!{}'
case_ noprel_vec ok '#![no_implicit_prelude]
extern crate kio;
use kio::*;
vec!{}'
case_ noprel_matches ok '#![no_implicit_prelude]
extern crate kio;
use kio::*;
matches!{}'
case_ noprel_builtin_assert 'error[E0659]: `assert` is ambiguous' '#![no_implicit_prelude]
extern crate kio;
use kio::*;
assert!{}'
case_ noprel_builtin_panic 'error[E0659]: `panic` is ambiguous' '#![no_implicit_prelude]
extern crate kio;
use kio::*;
panic!{}'
case_ noprel_builtin_unreachable 'error[E0659]: `unreachable` is ambiguous' '#![no_implicit_prelude]
extern crate kio;
use kio::*;
unreachable!{}'
for n in print eprint eprintln format dbg thread_local write writeln assert_eq assert_ne debug_assert debug_assert_eq debug_assert_ne todo unimplemented; do
  case_ "noprel_$n" ok "#![no_implicit_prelude]
extern crate kio;
use kio::*;
$n!{}"
done
case_ noprel_builtin_format_args 'error[E0659]: `format_args` is ambiguous' '#![no_implicit_prelude]
extern crate kio;
use kio::*;
format_args!{}'
case_ noprel_has_no_extern_prelude 'error[E0432]: unresolved import `kio`' '#![no_implicit_prelude]
use kio::*;'

# --- an explicit import against a glob ----------------------------------------------
case_ explicit_beats_glob_same_scope ok 'use kio::println;
use kio::*;
println!{}'
case_ explicit_outer_glob_inner 'error[E0659]: `println` is ambiguous' 'use kio::println;
mod m { pub fn f() { use kio::*; println!{} } }'

# --- the root of a pathed call against a glob ---------------------------------------
case_ pathed_root_module_glob 'error[E0659]: `kdep` is ambiguous' 'mod h {
    pub mod kdep {
        macro_rules! info { ($($t:tt)*) => { pub fn from_glob() {} }; }
        pub(crate) use info;
    }
}
use h::*;
kdep::info!{}'
case_ pathed_root_module_glob_in_block 'error[E0659]: `kdep` is ambiguous' 'mod h {
    pub mod kdep {
        macro_rules! info { ($($t:tt)*) => { pub fn from_glob() {} }; }
        pub(crate) use info;
    }
}
fn f() { use h::*; kdep::info!{} }'

# --- a derive against a glob --------------------------------------------------------
case_ derive_glob 'error[E0659]: `Clone` is ambiguous' 'use kder::*;
#[derive(Clone)]
pub struct S;'
case_ derive_explicit_use ok 'use kder::Clone;
#[derive(Clone)]
pub struct S;'

rm -f -- "$work"/*.rs "$work"/*.rlib "$work"/*.dylib "$work"/*.so
rmdir "$work" 2>/dev/null
exit $fail
