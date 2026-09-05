//! Function application in Nix (SC34), and why HCL and CFML were not closed
//! in the same pass.
//!
//! Nix reached `extract_node`'s generic arm and recovered nothing from it:
//! measured with `extract_file` on the realistic `flake.nix` below, immediately
//! before `langdecl::nix` and `langcalls::nix` existed, **1 symbol** (the
//! `File` node) and **0 calls**.
//!
//! "Is a call graph meaningful in a lazy expression language" is a fair
//! question, and the answer here is a count rather than an opinion. Of the
//! **10** `apply_expression` nodes in that flake, **3** apply a lambda bound in
//! the same file. That is what earns the module, and the last two tests record
//! why the same question got the opposite answer for HCL and CFML.

use devmap_extract::model::Extraction;
use devmap_extract::{extract_file, langcalls::CALL_EXTRACTION_LANGUAGES};

/// A flake of the shape most repositories actually contain.
const FLAKE: &str = r#"{
  description = "demo";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
      mkShellFor = pkgs: pkgs.mkShell {
        packages = [ pkgs.cargo pkgs.rustc ];
      };
      version = builtins.substring 0 8 self.lastModifiedDate;
    in
    {
      packages = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "demo";
            inherit version;
            src = ./.;
          };
        });
      devShells = forAllSystems (system: {
        default = mkShellFor nixpkgs.legacyPackages.${system};
      });
    };
}
"#;

/// The binding forms whose names are not a single bare identifier.
const BINDINGS: &str = r#"let
  helper = x: x + 1;
  services.foo.start = cfg: helper cfg;
  "${weird}" = y: y;
  plain = "not a lambda";
  inherit (builtins) length;
in {
  a = services.foo.start 1;
  b = helper 2;
}
"#;

fn qualified_names(extraction: &Extraction) -> Vec<String> {
    let mut names: Vec<String> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect();
    names.sort();
    names
}

fn targets(extraction: &Extraction) -> Vec<String> {
    let mut out: Vec<String> = extraction
        .calls
        .iter()
        .map(|call| match &call.receiver_expr {
            Some(receiver) => format!("{receiver}::{}", call.callee_name),
            None => call.callee_name.clone(),
        })
        .collect();
    out.sort();
    out
}

fn edges(extraction: &Extraction) -> Vec<String> {
    let mut out: Vec<String> = extraction
        .calls
        .iter()
        .map(|call| {
            format!(
                "{} -> {}",
                call.caller_symbol.as_deref().unwrap_or("<file>"),
                call.callee_name
            )
        })
        .collect();
    out.sort();
    out
}

/// Every symbol a call or reference is attributed to must be one the emitter
/// produced, and — because a `Contains` edge is built from `parent_symbol` —
/// every non-file parent must be one too.
fn dangling_sources(extraction: &Extraction) -> Vec<String> {
    let emitted = qualified_names(extraction);
    let mut dangling: Vec<String> = extraction
        .calls
        .iter()
        .filter_map(|call| call.caller_symbol.clone())
        .chain(
            extraction
                .references
                .iter()
                .filter_map(|reference| reference.enclosing_symbol.clone()),
        )
        .chain(
            extraction
                .symbols
                .iter()
                .filter_map(|symbol| symbol.parent_symbol.clone()),
        )
        .filter(|source| !emitted.contains(source))
        .collect();
    dangling.sort();
    dangling.dedup();
    dangling
}

/// Pre-fix: `symbols == ["flake.nix"]`, `calls == []`.
#[test]
fn nix_recovers_the_lambdas_and_the_applications_that_were_both_absent() {
    let extraction = extract_file("flake.nix", FLAKE);
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "flake.nix".to_string(),
            "flake.nix::forAllSystems".to_string(),
            "flake.nix::mkShellFor".to_string(),
            "flake.nix::outputs".to_string(),
        ],
        "only a binding whose value is a lambda is a declaration; `description`, \
         `systems`, `version` and the two `default` derivations are not"
    );
    assert_eq!(
        edges(&extraction),
        vec![
            "flake.nix::forAllSystems -> genAttrs".to_string(),
            "flake.nix::mkShellFor -> mkShell".to_string(),
            "flake.nix::outputs -> buildRustPackage".to_string(),
            "flake.nix::outputs -> forAllSystems".to_string(),
            "flake.nix::outputs -> forAllSystems".to_string(),
            "flake.nix::outputs -> mkShellFor".to_string(),
            "flake.nix::outputs -> substring".to_string(),
        ],
        "three of these name a lambda bound in this same file, which is the \
         measurement that earned this module"
    );
}

/// Currying makes one call several nodes, and only the innermost names the
/// function. `builtins.substring 0 8 self.lastModifiedDate` is three nested
/// `apply_expression`s and must contribute exactly one edge.
#[test]
fn nix_records_a_curried_application_exactly_once() {
    let extraction = extract_file("flake.nix", FLAKE);
    assert_eq!(
        targets(&extraction)
            .iter()
            .filter(|target| target.ends_with("substring"))
            .count(),
        1,
        "{:?}",
        targets(&extraction)
    );
    assert!(
        targets(&extraction).contains(&"builtins::substring".to_string()),
        "the receiver is the segment before the callee: {:?}",
        targets(&extraction)
    );
    assert!(
        targets(&extraction).contains(&"rustPlatform::buildRustPackage".to_string()),
        "a three-segment path is reached through its last qualifier, not through \
         the whole path: {:?}",
        targets(&extraction)
    );
}

/// A dotted binding keeps its whole path as its name and is contained by the
/// file.
///
/// Splitting it into owner + name promotes it to a `Method` and makes the
/// resolver emit a `Contains` edge from `f.nix::services.foo` — a symbol Nix
/// cannot declare, because an attribute-path prefix is not a declaration. This
/// pins the absence of that dangling edge.
#[test]
fn nix_does_not_invent_an_owner_for_a_dotted_binding() {
    let extraction = extract_file("bindings.nix", BINDINGS);
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "bindings.nix".to_string(),
            "bindings.nix::helper".to_string(),
            "bindings.nix::services.foo.start".to_string(),
        ],
        "`plain` is not a lambda, `\"${{weird}}\"` is not an identity, and \
         `inherit` declares nothing"
    );
    assert_eq!(
        dangling_sources(&extraction),
        Vec::<String>::new(),
        "no edge and no containment may name a symbol that was never emitted"
    );
    assert!(
        edges(&extraction).contains(&"bindings.nix::services.foo.start -> helper".to_string()),
        "a call inside a dotted binding's body belongs to that binding: {:?}",
        edges(&extraction)
    );
}

/// No call, reference or containment may name a symbol the emitter did not
/// produce.
#[test]
fn no_nix_edge_is_dangling() {
    let mut attributed = 0;
    for (path, source) in [("flake.nix", FLAKE), ("bindings.nix", BINDINGS)] {
        let extraction = extract_file(path, source);
        assert_eq!(
            dangling_sources(&extraction),
            Vec::<String>::new(),
            "{path} named a symbol that was never emitted"
        );
        attributed += extraction
            .calls
            .iter()
            .filter(|call| {
                call.caller_symbol
                    .as_deref()
                    .is_some_and(|caller| caller.contains("::"))
            })
            .count();
    }
    assert!(
        attributed >= 8,
        "only {attributed} calls were attributed to a symbol rather than to the \
         file; the check would be holding vacuously"
    );
}

/// HCL is **not** closed, and this records the measurement rather than the
/// opinion.
///
/// Terraform and OpenTofu have no user-defined function syntax: every
/// `function_call` node in a `.tf` file names a language built-in (`lower`,
/// `merge`, `templatefile`, `map`), which is declared nowhere in any corpus, so
/// **no** call edge could ever resolve. The real dependency graph in HCL is the
/// reference graph between blocks — `local.name`, `var.env`,
/// `data.aws_ami.base.id`, `aws_instance.web.private_ip` — which is a reference
/// edge, not a call, and already reaches the graph as `Name` references.
///
/// This test fails if someone gives HCL call extraction, which is the point:
/// the decision should be re-argued, not drifted into.
#[test]
fn hcl_is_deliberately_left_without_a_call_graph() {
    let source = r#"variable "env" { type = string }
locals {
  name = lower("App-${var.env}")
}
resource "aws_instance" "web" {
  tags = merge(var.tags, { Name = local.name })
}
"#;
    let extraction = extract_file("main.tf", source);
    assert!(
        !CALL_EXTRACTION_LANGUAGES.contains(&"hcl"),
        "HCL has no user-defined function syntax; every callee would be a \
         built-in that resolves to nothing"
    );
    assert!(extraction.calls.is_empty(), "{:?}", targets(&extraction));
    // The blocks are still symbols and the cross-block dependencies are still
    // references, so nothing is lost by declining the call graph.
    assert!(
        qualified_names(&extraction).contains(&"main.tf::variable.env".to_string()),
        "{:?}",
        qualified_names(&extraction)
    );
    assert!(
        extraction
            .references
            .iter()
            .any(|reference| reference.name == "local"),
        "the block-to-block dependency is a reference, and it is recorded"
    );
}

/// CFML is **not** closed, and the reason is in the grammar, not in the
/// language.
///
/// `tree-sitter-cfml` parses the *tag* dialect and treats every script region
/// as one opaque token: a script-syntax `component { … }` is
/// `(program (component_file (cf_component_content)))` and a `<cfscript>` block
/// is `(cf_script_tag (cf_script_content))`. Script syntax is the dominant
/// modern dialect, so a CFML call extractor would report coverage while seeing
/// nothing at all in most real files — a check that could not run reporting
/// what a check that ran and passed reports.
#[test]
fn cfml_is_deliberately_left_without_a_call_graph() {
    assert!(
        !CALL_EXTRACTION_LANGUAGES.contains(&"cfml"),
        "the grammar cannot see inside a script-syntax component"
    );
    for (path, source) in [
        (
            "Worker.cfc",
            "component {\n  function f() { return 1; }\n}\n",
        ),
        (
            "page.cfm",
            "<cfscript>\n  function f(a) { return a; }\n  x = f(1);\n</cfscript>\n",
        ),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| symbol.qualified_name == path),
            "{path}: the grammar yields an opaque body, so there is no \
             declaration to attribute a call to: {:?}",
            qualified_names(&extraction)
        );
    }
}

/// The coverage list must never claim a language the dispatcher does not route,
/// and must never omit one it does.
#[test]
fn the_coverage_list_and_the_dispatcher_agree_for_nix_hcl_and_cfml() {
    for (language, path, source) in [
        ("nix", "w.nix", "let f = x: x; in { a = f 1; }\n"),
        ("hcl", "w.tf", "locals {\n  a = lower(\"X\")\n}\n"),
        ("cfml", "w.cfm", "<cfset x = helper(1)>\n"),
    ] {
        let listed = CALL_EXTRACTION_LANGUAGES.contains(&language);
        let extracted = !extract_file(path, source).calls.is_empty();
        assert_eq!(
            listed, extracted,
            "{language}: coverage list says {listed}, extraction says {extracted}"
        );
    }
}

/// Extraction is a pure function of the source, and unparseable input must not
/// panic or dangle.
#[test]
fn nix_extraction_is_deterministic_and_survives_broken_input() {
    let first = format!("{:?}", extract_file("flake.nix", FLAKE).calls);
    for _ in 0..3 {
        assert_eq!(
            first,
            format!("{:?}", extract_file("flake.nix", FLAKE).calls)
        );
    }
    let broken = extract_file("bad.nix", "let { { { in x:\n");
    assert_eq!(dangling_sources(&broken), Vec::<String>::new());
}
