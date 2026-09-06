//! Import extraction for the languages whose specifier names a file (W0.3-2).
//!
//! Every test here fails against the pre-change tree, where `extraction.imports`
//! is **empty** for all seventeen language keys — not partially wrong, not
//! low-confidence: absent. Before this module the whole extractor had five
//! `imports.push` sites (Python, JS/TS, Rust `use`, Go `import_spec`, the
//! embedded-script merge) and no handler for `#include` anywhere, so the answer
//! to "does anything import this file" was structurally `no` for 23 of 35
//! declared languages, C and C++ included.
//!
//! The tests go through `devmap_extract::extract_file`, which is the path a real
//! build takes, so they cover the dispatcher wiring in `extract_node` as well as
//! the modules behind it. A module that works but is never reached fails here.

use devmap_extract::extract_file;

/// The specifiers extracted from one source, sorted, so a test asserts the
/// whole set rather than "at least one" — which is the assertion that lets a
/// duplicate or a stray extra import through unnoticed.
fn specifiers(path: &str, source: &str) -> Vec<String> {
    let extraction = extract_file(path, source);
    assert!(
        !extraction.is_parse_failure(),
        "{path} must parse for this test to mean anything: {:?}",
        extraction.parse_outcome
    );
    let mut found: Vec<String> = extraction
        .imports
        .iter()
        .map(|import| import.module_specifier.clone())
        .collect();
    found.sort();
    found
}

fn assert_specifiers(path: &str, source: &str, expected: &[&str]) {
    let found = specifiers(path, source);
    let mut want: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    want.sort();
    assert_eq!(found, want, "specifiers extracted from {path}");
}

// ---------------------------------------------------------------------------
// The C family: the gap that made the honest gate necessary.
// ---------------------------------------------------------------------------

#[test]
fn c_include_names_the_header_it_includes() {
    assert_specifiers(
        "src/main.c",
        "#include \"util.h\"\n#include <stdio.h>\n\nint main(void) { return 0; }\n",
        &["util.h", "stdio.h"],
    );
}

#[test]
fn cpp_include_carries_a_directory_prefix() {
    assert_specifiers(
        "src/engine.cpp",
        "#include \"core/widget.hpp\"\n#include <vector>\n\nvoid run() {}\n",
        &["core/widget.hpp", "vector"],
    );
}

#[test]
fn objc_import_is_the_same_node_as_include() {
    assert_specifiers(
        "src/View.m",
        "#import \"View.h\"\n#import <Foundation/Foundation.h>\n",
        &["View.h", "Foundation/Foundation.h"],
    );
}

#[test]
fn cuda_include_is_extracted() {
    assert_specifiers(
        "src/kernel.cu",
        "#include \"kernel.cuh\"\n\n__global__ void k() {}\n",
        &["kernel.cuh"],
    );
}

/// A macro include names no file until the preprocessor has run.
///
/// Emitting `HEADER_NAME` as a specifier would add an import that can only ever
/// fail to resolve, inflating the import count with an entry carrying no
/// information — the repository's own rule that a check which could not run
/// must not report the same result as one that ran.
#[test]
fn a_macro_include_is_refused_rather_than_guessed() {
    assert_specifiers(
        "src/macro.c",
        "#define HEADER_NAME \"real.h\"\n#include HEADER_NAME\n\nint f(void) { return 0; }\n",
        &[],
    );
}

// ---------------------------------------------------------------------------
// The JVM family: a dotted name the language maps to a path.
// ---------------------------------------------------------------------------

#[test]
fn java_import_keeps_the_dotted_type_name() {
    assert_specifiers(
        "src/App.java",
        "package app;\n\nimport com.foo.Bar;\nimport java.util.List;\n\nclass App {}\n",
        &["com.foo.Bar", "java.util.List"],
    );
}

/// A static import's last segment is a member of the type before it, so the
/// member is dropped: `com.foo.Bar.baz` names the file `com/foo/Bar.java`.
#[test]
fn a_static_java_import_names_the_type_not_the_member() {
    assert_specifiers(
        "src/App.java",
        "package app;\n\nimport static com.foo.Bar.baz;\n\nclass App {}\n",
        &["com.foo.Bar"],
    );
}

#[test]
fn a_java_wildcard_import_is_marked_as_a_package() {
    assert_specifiers(
        "src/App.java",
        "package app;\n\nimport com.foo.*;\nimport static com.foo.Bar.*;\n\nclass App {}\n",
        &["com.foo.*", "com.foo.Bar.*"],
    );
}

#[test]
fn kotlin_import_keeps_the_dotted_name() {
    assert_specifiers(
        "src/App.kt",
        "package app\n\nimport com.foo.Bar\nimport com.foo.util.*\n\nfun main() {}\n",
        &["com.foo.Bar", "com.foo.util.*"],
    );
}

#[test]
fn scala_import_reduces_a_selector_list_to_its_package() {
    assert_specifiers(
        "src/App.scala",
        "package app\n\nimport foo.bar.Baz\nimport foo.qux.{A, B}\nimport foo.wild._\n\nobject App\n",
        &["foo.bar.Baz", "foo.qux.*", "foo.wild.*"],
    );
}

// ---------------------------------------------------------------------------
// Quoted paths.
// ---------------------------------------------------------------------------

#[test]
fn dart_import_keeps_the_uri() {
    assert_specifiers(
        "lib/app.dart",
        "import 'util.dart';\nimport 'package:app/helper.dart';\n\nvoid main() {}\n",
        &["util.dart", "package:app/helper.dart"],
    );
}

#[test]
fn solidity_import_keeps_the_path() {
    assert_specifiers(
        "contracts/Token.sol",
        "pragma solidity ^0.8.0;\n\nimport \"./Ownable.sol\";\nimport {SafeMath} from \"./math/SafeMath.sol\";\n\ncontract Token {}\n",
        &["./Ownable.sol", "./math/SafeMath.sol"],
    );
}

#[test]
fn erlang_include_names_the_header() {
    assert_specifiers(
        "src/app.erl",
        "-module(app).\n-include(\"records.hrl\").\n-include_lib(\"kernel/include/file.hrl\").\n\nrun() -> ok.\n",
        &["records.hrl", "kernel/include/file.hrl"],
    );
}

/// `-import(lists, [map/2]).` names a module for unqualified calls, and that
/// relation is already carried by the call graph. Emitting it here would
/// double-count one dependency under two edge kinds.
#[test]
fn an_erlang_function_import_is_not_a_file_import() {
    assert_specifiers(
        "src/app.erl",
        "-module(app).\n-import(lists, [map/2]).\n\nrun() -> ok.\n",
        &[],
    );
}

#[test]
fn ruby_require_and_require_relative_both_count() {
    assert_specifiers(
        "app/main.rb",
        "require 'json'\nrequire_relative 'helper'\n\ndef run\n  1\nend\n",
        &["json", "helper"],
    );
}

/// A computed path names no file statically, and a `require` sent to a receiver
/// is somebody else's API.
#[test]
fn a_computed_or_received_ruby_require_is_refused() {
    assert_specifiers(
        "app/main.rb",
        "require File.join(dir, 'x')\nloader.require 'y'\nrequire CONST\n",
        &[],
    );
}

#[test]
fn lua_require_is_extracted_with_and_without_parentheses() {
    assert_specifiers(
        "src/main.lua",
        "local a = require(\"app.util\")\nlocal b = require \"app.other\"\n",
        &["app.util", "app.other"],
    );
}

#[test]
fn luau_shares_lua_require() {
    assert_specifiers(
        "src/main.luau",
        "local a = require(\"app.util\")\n",
        &["app.util"],
    );
}

#[test]
fn r_source_is_a_file_import_and_library_is_not() {
    assert_specifiers(
        "R/main.R",
        "library(dplyr)\nsource(\"helpers.R\")\n\nf <- function(x) x + 1\n",
        &["helpers.R"],
    );
}

#[test]
fn nix_import_names_a_path() {
    assert_specifiers(
        "default.nix",
        "let lib = import ./lib; in { inherit lib; }\n",
        &["./lib"],
    );
}

#[test]
fn pascal_uses_names_each_unit_separately() {
    assert_specifiers(
        "src/App.pas",
        "unit App;\n\ninterface\n\nuses SysUtils, App.Helpers;\n\nimplementation\n\nend.\n",
        &["SysUtils", "App.Helpers"],
    );
}

#[test]
fn php_use_and_require_are_both_extracted() {
    assert_specifiers(
        "src/App.php",
        "<?php\nnamespace App;\n\nuse App\\Foo\\Bar;\nrequire_once 'lib/util.php';\n\nclass App {}\n",
        &["App\\Foo\\Bar", "lib/util.php"],
    );
}

// ---------------------------------------------------------------------------
// The two declines, pinned so they stay decisions rather than oversights.
// ---------------------------------------------------------------------------

/// C# `using System;` names a **namespace**, not a file.
///
/// A C# namespace spans any number of files and a single file may declare
/// several, so there is no rule — of the language or of its tooling — that maps
/// a `using` to a file. Extracting one would mean either inventing a
/// path-from-namespace convention the language does not have, or emitting a
/// specifier that never resolves. Both are worse than the honest exclusion the
/// coverage gate already reports.
#[test]
fn csharp_using_is_deliberately_not_an_import() {
    assert_specifiers(
        "src/App.cs",
        "using System;\nusing App.Models;\n\nnamespace App { class Program {} }\n",
        &[],
    );
}

/// Swift `import Foundation` names a **module**.
///
/// Worse than unresolvable: it is the one thing that cannot explain intra-repo
/// wiring, because files in the *same* module — which is what a Swift target
/// is, and what almost every file in a Swift repository belongs to — need no
/// import of each other at all. A Swift file's inbound dependencies are
/// invisible to import syntax by design, so `unwired_candidates` must keep
/// excluding Swift and saying so.
#[test]
fn swift_import_is_deliberately_not_a_file_import() {
    assert_specifiers(
        "Sources/App/main.swift",
        "import Foundation\nimport UIKit\n\nfunc run() {}\n",
        &[],
    );
}

// ---------------------------------------------------------------------------
// The nested-invocation rule, at every language that shares the walk.
//
// `require File.join(dir, 'x')` extracted `x` — a specifier the author never
// wrote, which resolves either to nothing or, worse, to a real file of that
// name. It surfaced in Ruby and was fixed in the shared walk, so these pin the
// same rule for the others rather than leaving twelve languages carrying a
// defect that was only ever demonstrated in one.
// ---------------------------------------------------------------------------

#[test]
fn a_computed_lua_require_is_refused() {
    assert_specifiers(
        "src/main.lua",
        "local a = require(resolve(\"app.util\"))\n",
        &[],
    );
}

#[test]
fn a_computed_r_source_is_refused() {
    assert_specifiers("R/main.R", "source(file.path(d, \"helpers.R\"))\n", &[]);
}

/// PHP's concatenation stays transparent on purpose: `__DIR__ . '/util.php'`
/// is the idiomatic form and the string in it really is the path. A call in the
/// same position is not.
#[test]
fn php_sees_through_concatenation_but_not_through_a_call() {
    assert_specifiers(
        "src/a.php",
        "<?php\nrequire __DIR__ . '/util.php';\n",
        &["/util.php"],
    );
    assert_specifiers("src/b.php", "<?php\nrequire resolve('util.php');\n", &[]);
}

// ---------------------------------------------------------------------------
// Rust `mod`, found by measuring the real corpus rather than by auditing the
// capability table — Rust already declared `IMPORTS` for `use`, so every check
// that asked "does this language extract imports" answered yes while the
// statement that names a file had no handler at all.
// ---------------------------------------------------------------------------

#[test]
fn a_rust_mod_declaration_names_the_file_it_declares() {
    assert_specifiers(
        "src/lib.rs",
        "pub mod parser;\nmod internal;\n\npub fn run() {}\n",
        &["self::parser", "self::internal"],
    );
}

/// An inline module's contents are in this same file. Emitting an import for it
/// would be an edge from a file to itself.
#[test]
fn an_inline_rust_module_is_not_a_file_import() {
    assert_specifiers(
        "src/lib.rs",
        "pub fn run() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
        &[],
    );
}

/// `mod a { mod b; }` declares `a/b.rs`, one directory deeper.
#[test]
fn a_nested_rust_mod_carries_its_enclosing_module() {
    assert_specifiers(
        "src/lib.rs",
        "pub mod outer {\n    pub mod inner;\n}\n",
        &["self::outer::inner"],
    );
}

/// `#[path = "…"]` exists precisely because the conventional path is wrong for
/// that module, so ignoring it would resolve to nothing and leave the real file
/// reported as unwired.
#[test]
fn a_rust_path_attribute_overrides_the_conventional_file() {
    assert_specifiers(
        "src/lib.rs",
        "#[path = \"generated/tables.rs\"]\nmod tables;\n",
        &["self::generated/tables.rs"],
    );
}

/// `use` is still extracted, by the arm that always did it. The two statements
/// are on opposite sides of this module's rule and both must survive.
#[test]
fn rust_use_declarations_are_still_extracted_beside_mod() {
    assert_specifiers(
        "src/lib.rs",
        "use crate::parser::Token;\nmod parser;\n",
        // X41. The `use` arm now splits module from name, so the specifier is
        // the module `crate::parser` and `Token` is what it imports — the shape
        // every other language's extractor produces, and the one the resolver's
        // per-name binding walk reads. Only `mod` is normalised to a `self::`
        // module path here, because only `mod` names a file outright.
        &["crate::parser", "self::parser"],
    );
}
