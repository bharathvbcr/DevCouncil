//! An extracted specifier that resolves to nothing is worth nothing.
//!
//! W0.3 move 2 gave thirteen more languages import extraction, but
//! `unwired_candidates` asks whether a file has an inbound `Imports` **edge**,
//! and an edge needs a target. Extraction without resolution would have moved
//! the failure one stage later and left the answer the same: every C header,
//! every Java class, every Ruby helper still reported as imported by nothing.
//!
//! Every test here fails against the pre-change tree in the way that matters —
//! no edge at all — and each goes through `Resolver::resolve_all`, which is the
//! path a build takes.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::Resolver;

/// The `Imports` edges a corpus produces, as `(source, target)` pairs.
fn import_edges(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolved = resolver.resolve_all(&extractions);
    let mut edges: Vec<(String, String)> = resolved
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Imports)
        .map(|edge| (edge.source_file.clone(), edge.target_file.clone()))
        .collect();
    edges.sort();
    edges.dedup();
    edges
}

fn assert_imports(files: &[(&str, &str)], expected: &[(&str, &str)]) {
    let found = import_edges(files);
    let mut want: Vec<(String, String)> = expected
        .iter()
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect();
    want.sort();
    assert_eq!(found, want, "import edges");
}

#[test]
fn a_c_include_resolves_to_the_header_beside_it() {
    assert_imports(
        &[
            (
                "src/main.c",
                "#include \"util.h\"\nint main(void){return 0;}\n",
            ),
            ("src/util.h", "int helper(void);\n"),
        ],
        &[("src/main.c", "src/util.h")],
    );
}

/// The header is not beside the caller and the include path is not written in
/// the source — which is the ordinary shape of a CMake project.
#[test]
fn a_c_include_resolves_through_an_include_root() {
    assert_imports(
        &[
            (
                "src/main.c",
                "#include \"core/widget.h\"\nint main(void){return 0;}\n",
            ),
            ("include/core/widget.h", "void widget(void);\n"),
        ],
        &[("src/main.c", "include/core/widget.h")],
    );
}

/// Two headers of the same name and no path that distinguishes them: the
/// basename rung abstains rather than picking one.
#[test]
fn an_ambiguous_header_basename_produces_no_edge() {
    assert_imports(
        &[
            (
                "src/main.c",
                "#include \"util.h\"\nint main(void){return 0;}\n",
            ),
            ("a/util.h", "int a(void);\n"),
            ("b/util.h", "int b(void);\n"),
        ],
        &[],
    );
}

#[test]
fn a_system_header_that_matches_nothing_produces_no_edge() {
    assert_imports(
        &[(
            "src/main.c",
            "#include <stdio.h>\nint main(void){return 0;}\n",
        )],
        &[],
    );
}

#[test]
fn a_java_import_resolves_through_the_maven_source_root() {
    assert_imports(
        &[
            (
                "src/main/java/app/App.java",
                "package app;\n\nimport com.foo.Bar;\n\nclass App {}\n",
            ),
            (
                "src/main/java/com/foo/Bar.java",
                "package com.foo;\n\npublic class Bar {}\n",
            ),
        ],
        &[(
            "src/main/java/app/App.java",
            "src/main/java/com/foo/Bar.java",
        )],
    );
}

/// A wildcard import depends on every file in the package, so every one gets an
/// edge. Truncating the list would leave the files past the cut reported as
/// imported by nothing — a bound turning into a false finding.
#[test]
fn a_java_wildcard_import_reaches_every_file_in_the_package() {
    assert_imports(
        &[
            (
                "src/main/java/app/App.java",
                "package app;\n\nimport com.foo.*;\n\nclass App {}\n",
            ),
            (
                "src/main/java/com/foo/Bar.java",
                "package com.foo;\n\npublic class Bar {}\n",
            ),
            (
                "src/main/java/com/foo/Baz.java",
                "package com.foo;\n\npublic class Baz {}\n",
            ),
        ],
        &[
            (
                "src/main/java/app/App.java",
                "src/main/java/com/foo/Bar.java",
            ),
            (
                "src/main/java/app/App.java",
                "src/main/java/com/foo/Baz.java",
            ),
        ],
    );
}

#[test]
fn a_kotlin_import_resolves_through_its_source_root() {
    assert_imports(
        &[
            (
                "src/main/kotlin/app/App.kt",
                "package app\n\nimport com.foo.Bar\n\nfun main() {}\n",
            ),
            (
                "src/main/kotlin/com/foo/Bar.kt",
                "package com.foo\n\nclass Bar\n",
            ),
        ],
        &[(
            "src/main/kotlin/app/App.kt",
            "src/main/kotlin/com/foo/Bar.kt",
        )],
    );
}

#[test]
fn a_ruby_require_relative_resolves_beside_the_file() {
    assert_imports(
        &[
            (
                "app/main.rb",
                "require_relative 'helper'\n\ndef run; 1; end\n",
            ),
            ("app/helper.rb", "def help; 2; end\n"),
        ],
        &[("app/main.rb", "app/helper.rb")],
    );
}

#[test]
fn a_ruby_require_resolves_through_lib() {
    assert_imports(
        &[
            ("bin/run.rb", "require 'app/helper'\n"),
            ("lib/app/helper.rb", "def help; 2; end\n"),
        ],
        &[("bin/run.rb", "lib/app/helper.rb")],
    );
}

#[test]
fn a_php_use_resolves_under_psr4_and_a_require_resolves_as_a_path() {
    assert_imports(
        &[
            (
                "src/App.php",
                "<?php\nnamespace App;\n\nuse App\\Foo\\Bar;\nrequire_once 'lib/util.php';\n\nclass App {}\n",
            ),
            ("src/App/Foo/Bar.php", "<?php\nnamespace App\\Foo;\nclass Bar {}\n"),
            ("lib/util.php", "<?php\nfunction util() { return 1; }\n"),
        ],
        &[
            ("src/App.php", "lib/util.php"),
            ("src/App.php", "src/App/Foo/Bar.php"),
        ],
    );
}

#[test]
fn a_lua_require_resolves_a_dotted_module_path() {
    assert_imports(
        &[
            ("src/main.lua", "local u = require(\"app.util\")\n"),
            (
                "src/app/util.lua",
                "return { f = function() return 1 end }\n",
            ),
        ],
        &[("src/main.lua", "src/app/util.lua")],
    );
}

#[test]
fn a_nix_import_resolves_a_directory_to_its_default() {
    assert_imports(
        &[
            ("default.nix", "let lib = import ./lib; in lib\n"),
            ("lib/default.nix", "{ x = 1; }\n"),
        ],
        &[("default.nix", "lib/default.nix")],
    );
}

#[test]
fn a_solidity_import_resolves_a_relative_path() {
    assert_imports(
        &[
            (
                "contracts/Token.sol",
                "pragma solidity ^0.8.0;\nimport \"./Ownable.sol\";\ncontract Token {}\n",
            ),
            (
                "contracts/Ownable.sol",
                "pragma solidity ^0.8.0;\ncontract Ownable {}\n",
            ),
        ],
        &[("contracts/Token.sol", "contracts/Ownable.sol")],
    );
}

#[test]
fn an_erlang_include_resolves_through_the_include_directory() {
    assert_imports(
        &[
            (
                "src/app.erl",
                "-module(app).\n-include(\"records.hrl\").\nrun() -> ok.\n",
            ),
            ("include/records.hrl", "-record(r, {a}).\n"),
        ],
        &[("src/app.erl", "include/records.hrl")],
    );
}

#[test]
fn a_dart_package_import_resolves_under_lib() {
    assert_imports(
        &[
            (
                "lib/app.dart",
                "import 'package:app/helper.dart';\nvoid main() {}\n",
            ),
            ("lib/helper.dart", "int help() => 1;\n"),
        ],
        &[("lib/app.dart", "lib/helper.dart")],
    );
}

#[test]
fn a_terraform_module_source_reaches_every_tf_file_in_the_directory() {
    assert_imports(
        &[
            (
                "main.tf",
                "module \"vpc\" {\n  source = \"./modules/vpc\"\n}\n",
            ),
            (
                "modules/vpc/main.tf",
                "resource \"aws_vpc\" \"v\" {\n  cidr = \"10.0.0.0/16\"\n}\n",
            ),
            (
                "modules/vpc/outputs.tf",
                "output \"id\" {\n  value = 1\n}\n",
            ),
        ],
        &[
            ("main.tf", "modules/vpc/main.tf"),
            ("main.tf", "modules/vpc/outputs.tf"),
        ],
    );
}

/// A registry address is not a path, and must not resolve against a
/// same-named local directory.
#[test]
fn a_terraform_registry_source_produces_no_edge() {
    assert_imports(
        &[
            (
                "main.tf",
                "module \"consul\" {\n  source = \"hashicorp/consul/aws\"\n}\n",
            ),
            (
                "hashicorp/consul/aws/main.tf",
                "output \"x\" {\n  value = 1\n}\n",
            ),
        ],
        &[],
    );
}

#[test]
fn a_cfml_include_resolves_to_its_template() {
    assert_imports(
        &[
            ("index.cfm", "<cfinclude template=\"header.cfm\">\n"),
            ("header.cfm", "<cfoutput>hi</cfoutput>\n"),
        ],
        &[("index.cfm", "header.cfm")],
    );
}

#[test]
fn a_scala_import_resolves_through_its_source_root() {
    assert_imports(
        &[
            (
                "src/main/scala/app/App.scala",
                "package app\n\nimport foo.bar.Baz\n\nobject App\n",
            ),
            (
                "src/main/scala/foo/bar/Baz.scala",
                "package foo.bar\n\nclass Baz\n",
            ),
        ],
        &[(
            "src/main/scala/app/App.scala",
            "src/main/scala/foo/bar/Baz.scala",
        )],
    );
}

#[test]
fn a_pascal_uses_clause_resolves_a_dotted_unit() {
    assert_imports(
        &[
            (
                "src/App.pas",
                "unit App;\n\ninterface\n\nuses App.Helpers;\n\nimplementation\n\nend.\n",
            ),
            (
                "src/App.Helpers.pas",
                "unit App.Helpers;\n\ninterface\n\nimplementation\n\nend.\n",
            ),
        ],
        &[("src/App.pas", "src/App.Helpers.pas")],
    );
}

#[test]
fn an_r_source_resolves_beside_the_script() {
    assert_imports(
        &[
            ("R/main.R", "library(dplyr)\nsource(\"helpers.R\")\n"),
            ("R/helpers.R", "help_fn <- function(x) x\n"),
        ],
        &[("R/main.R", "R/helpers.R")],
    );
}

/// The relative-only rule: `./util.h` must not match a root-level `util.h`.
///
/// Without it a specifier the author wrote as relative could be claimed by a
/// same-named file elsewhere, which is a confidently wrong edge rather than a
/// missing one.
#[test]
fn a_relative_specifier_is_not_resolved_against_the_repository_root() {
    assert_imports(
        &[
            (
                "src/deep/main.c",
                "#include \"./util.h\"\nint main(void){return 0;}\n",
            ),
            ("util.h", "int root(void);\n"),
            ("src/deep/other.c", "int other(void){return 0;}\n"),
        ],
        &[],
    );
}

// ---------------------------------------------------------------------------
// Rust `mod`. Found by measuring this repository rather than by auditing the
// capability table: Rust already declared `IMPORTS` for `use`, so every check
// that asked "does this language extract imports" answered yes while the
// statement that names a file had no handler. Before the fix, thirty-odd
// `langdecl/*.rs` and `langcalls/*.rs` modules — each declared by a `mod` line
// in its own parent — were reported as unwired candidates.
// ---------------------------------------------------------------------------

#[test]
fn a_rust_mod_declaration_wires_the_file_it_declares() {
    assert_imports(
        &[
            ("src/lib.rs", "pub mod parser;\n\npub fn run() {}\n"),
            ("src/parser.rs", "pub fn parse() {}\n"),
        ],
        &[("src/lib.rs", "src/parser.rs")],
    );
}

#[test]
fn a_rust_mod_declaration_resolves_a_directory_module() {
    assert_imports(
        &[
            ("src/lib.rs", "pub mod parser;\n"),
            ("src/parser/mod.rs", "pub fn parse() {}\n"),
        ],
        &[("src/lib.rs", "src/parser/mod.rs")],
    );
}

#[test]
fn a_rust_path_attribute_resolves_to_the_file_it_names() {
    assert_imports(
        &[
            (
                "src/lib.rs",
                "#[path = \"generated/tables.rs\"]\nmod tables;\n",
            ),
            ("src/generated/tables.rs", "pub const N: u8 = 1;\n"),
        ],
        &[("src/lib.rs", "src/generated/tables.rs")],
    );
}

/// The nested case: `mod a { mod b; }` declares `a/b.rs`, one directory deeper.
#[test]
fn a_nested_rust_mod_resolves_one_directory_deeper() {
    assert_imports(
        &[
            ("src/lib.rs", "pub mod outer {\n    pub mod inner;\n}\n"),
            ("src/outer/inner.rs", "pub fn f() {}\n"),
        ],
        &[("src/lib.rs", "src/outer/inner.rs")],
    );
}
