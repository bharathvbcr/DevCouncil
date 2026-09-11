//! The adversarial spellings of every file-liveness rule.
//!
//! Each rule here removes a path from *every* file-level verdict at once —
//! `unwired_candidates`, `unreachable_files`, and the dead-cluster file roll-up
//! — so an over-match is not a cosmetic nuisance: it hides a stranded module
//! behind a filename. The table below is therefore two-sided everywhere. For
//! every spelling that must match there is a neighbouring one that must not,
//! and the near-misses are the point: `x.env` and `environment.ts`,
//! `testdata/` the directory and `testdata` the file, `foo.d.ts` and a
//! directory called `d.ts`, `index.ts` the barrel and `index.ts` the module.
//!
//! Fail-open is the direction every rule takes when it cannot decide. A rule
//! that declines produces one finding a reader can dismiss; a rule that
//! over-reaches produces silence, and silence is indistinguishable from a
//! clean repository.

use devmap_extract::extract_file;
use devmap_extract::languages::{
    detect_language, is_indexable_source, liveness_unit_for_language, non_code_path_reason,
    LivenessUnit,
};
use devmap_extract::model::{FileLiveness, WiringKind};
use devmap_extract::wiring::{
    extract_wiring_annotations, has_shebang, is_ambient_declaration, is_fixture_path, is_test_path,
    package_marker_reason, tool_config_reason,
};
use std::path::Path;

/// The verdict `Extraction::file_liveness()` must reach for `(path, source)`.
fn liveness(path: &str, source: &str) -> FileLiveness {
    extract_file(path, source).file_liveness()
}

fn assert_not_code(path: &str, source: &str) {
    match liveness(path, source) {
        FileLiveness::NotCode { reason } => assert!(!reason.is_empty(), "{path}: blank reason"),
        other => panic!("{path} must be NotCode, got {other:?}"),
    }
}

fn assert_exempt(path: &str, source: &str, expected: WiringKind) {
    match liveness(path, source) {
        FileLiveness::Exempt { kind, reason } => {
            assert_eq!(kind, expected, "{path} is exempt for the wrong reason");
            assert!(!reason.is_empty(), "{path}: blank reason");
        }
        other => panic!("{path} must be Exempt({expected:?}), got {other:?}"),
    }
}

fn assert_candidate(path: &str, source: &str) {
    let actual = liveness(path, source);
    assert!(
        actual.is_candidate(),
        "{path} is ordinary code and must stay a candidate, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// Environment files and lockfiles
// ---------------------------------------------------------------------------

/// Every spelling of an environment file, and the words that merely contain
/// `env`.
///
/// Both halves matter. A `.env` reaching the graph as code is a secret in an
/// artifact agents read; `environment.ts` excluded as an environment file is a
/// module nobody will ever be told is stranded.
#[test]
fn an_environment_file_is_data_and_a_word_containing_env_is_not() {
    for path in [
        ".env",
        "web/.env",
        "config/.env.production",
        ".env.local",
        ".envrc",
        "prod.env",
        "x.env",
        "deploy/staging.env",
        // Case is not a disguise.
        "config/.ENV",
        "Config/.Env.Production",
        "PROD.ENV",
        // A Windows separator is still a separator.
        r"config\.env.production",
    ] {
        assert!(
            non_code_path_reason(path).is_some(),
            "{path} is an environment file"
        );
    }
    for path in [
        "src/environment.ts",
        "src/env.ts",
        "src/envelope.py",
        "src/env/loader.py",
        // A directory named `.env` does not make its contents data. The rule
        // reads the basename, which is the file.
        "src/parse_env.rs",
    ] {
        assert!(
            non_code_path_reason(path).is_none(),
            "{path} is code that mentions env, not an environment file"
        );
    }
}

/// A lockfile, under every ecosystem's spelling — including the one that
/// parses as code.
///
/// `.terraform.lock.hcl` is why this rule exists at the *path* level rather
/// than in the language table: it parses as HCL, declares `provider` blocks,
/// and every gate that asks which engine ran gets "a grammar read this" with
/// full confidence.
#[test]
fn a_lockfile_is_data_whatever_grammar_reads_it() {
    for path in [
        "Cargo.lock",
        "flake.lock",
        "poetry.lock",
        "package-lock.json",
        "npm/package-lock.json",
        "infra/.terraform.lock.hcl",
        "Gemfile.lock",
        "composer.lock",
        "CARGO.LOCK",
    ] {
        assert!(non_code_path_reason(path).is_some(), "{path} is a lockfile");
    }
    for path in [
        "src/lock.rs",
        "src/locking.py",
        "src/file_lock.ts",
        "src/lockfile_parser.go",
        // A `.hcl` that is not a lockfile is a directory unit, not data — a
        // different verdict, and this rule must not claim it.
        "infra/backend.hcl",
    ] {
        assert!(
            non_code_path_reason(path).is_none(),
            "{path} is code about locks, not a lockfile"
        );
    }
    // And the one that matters end to end.
    assert_not_code(
        "infra/.terraform.lock.hcl",
        "provider \"registry.terraform.io/hashicorp/aws\" {\n  version = \"5.0.0\"\n}\n",
    );
}

/// Terraform's two file kinds get two different verdicts, and neither is a
/// finding.
#[test]
fn terraform_splits_into_a_directory_unit_and_its_data() {
    assert_exempt(
        "infra/main.tf",
        "resource \"aws_s3_bucket\" \"b\" {}\n",
        WiringKind::DirectoryUnit,
    );
    assert_not_code("infra/vars.tfvars", "region = \"us-east-1\"\n");
    assert_not_code("infra/vars.tfvars.json", "{\"region\": \"us-east-1\"}\n");
    // The reason names the directory, which is what makes the exemption
    // auditable: a reader can go and look at the folder it points to.
    let FileLiveness::Exempt { reason, .. } = liveness("infra/modules/net/main.tf", "locals {}\n")
    else {
        panic!("a `.tf` file is exempt");
    };
    assert!(
        reason.contains("infra/modules/net"),
        "the reason must name the directory it claims is the unit: {reason}"
    );
}

// ---------------------------------------------------------------------------
// Data formats
// ---------------------------------------------------------------------------

/// The exclusion of prose no longer depends on which engine ran.
///
/// This is the whole point of the `liveness_unit` column. Before it, prose was
/// excluded by `grammar_read_this_file()` — a question about the engine — and
/// linking a YAML or JSON grammar tomorrow would re-create the historical bug
/// in which every `.md`, `.json` and `.yaml` in every repository was a
/// delete-this suggestion.
#[test]
fn every_data_format_is_data_by_language_and_not_by_engine() {
    for path in [
        "README.md",
        "docs/guide.markdown",
        "data/config.json",
        "ci/pipeline.yaml",
        "ci/pipeline.yml",
        "docker-compose.yml",
        "config/settings.toml",
        "web/index.html",
        "web/page.htm",
        "web/app.css",
        "web/app.scss",
        "web/app.less",
    ] {
        let language = detect_language(Path::new(path));
        assert_eq!(
            liveness_unit_for_language(language),
            LivenessUnit::Data,
            "{path} ({language}) must be data by language"
        );
        assert_not_code(path, "x\n");
    }
    // An unknown extension answers `generic`, which is data, and is not
    // indexable in the first place — belt and braces, in that order.
    assert_eq!(liveness_unit_for_language("generic"), LivenessUnit::Data);

    // The reason is what the published histogram is keyed on and what the HTML
    // detail panel shows, so it has to say something a reader can act on. A
    // non-empty placeholder satisfies "not blank" and tells them nothing;
    // `Some("xyzzy")` survived the suite on exactly that gap.
    let data_reason = LivenessUnit::Data
        .not_code_reason()
        .expect("the data unit declares a reason");
    assert!(
        data_reason.contains("declares nothing"),
        "the reason must state why the file is outside the population, which \
         is that it declares nothing to strand: {data_reason:?}"
    );
    assert!(
        data_reason.contains("data") || data_reason.contains("prose"),
        "and it must name the category: {data_reason:?}"
    );
    // The other two units are not exclusions and must not invent one.
    assert_eq!(LivenessUnit::Module.not_code_reason(), None);
    assert_eq!(LivenessUnit::Directory.not_code_reason(), None);
    for path in ["LICENSE", "docs/NOTES.YML", "thing.qqq", "Dockerfile.dev"] {
        assert!(
            !is_indexable_source(path),
            "{path} is not indexed at all, so no liveness question is asked of it"
        );
        assert_eq!(
            liveness_unit_for_language(detect_language(Path::new(path))),
            LivenessUnit::Data,
            "{path} answers `generic`, which is data — the second bound behind \
             indexability"
        );
    }
}

/// SQL is code with a capability gap, not data.
///
/// The distinction is what keeps `excluded_import_blind` meaning what it says.
/// Folding SQL into `Data` would make the two counters agree by deleting the
/// question, and `only_source_files_are_charged_as_import_blind` pins the
/// answer.
#[test]
fn sql_is_a_module_with_no_import_extractor_not_a_data_file() {
    assert_eq!(liveness_unit_for_language("sql"), LivenessUnit::Module);
    assert_candidate("db/schema.sql", "SELECT helper(name) FROM widgets;\n");
}

/// Go is a `Module` here even though a Go package is a directory.
///
/// The resolver emits a `package:<dir>/<pkg>` node and `unwired_candidates`
/// reads it back, so the directory question *is* answered: a file in an
/// imported package is cleared, and a package nothing imports is still
/// reported. Marking Go `Directory` would blanket-exempt every Go file and
/// delete the second finding, which
/// `a_go_package_nothing_imports_is_still_a_candidate` refuses.
#[test]
fn go_is_a_module_here_because_its_directory_question_is_answered_elsewhere() {
    assert_eq!(liveness_unit_for_language("go"), LivenessUnit::Module);
    assert_candidate(
        "orphanpkg/orphan.go",
        "package orphanpkg\n\nfunc Idle() {}\n",
    );
}

// ---------------------------------------------------------------------------
// Shebang
// ---------------------------------------------------------------------------

/// `#!` is the first two bytes of the first line, and nothing else counts.
#[test]
fn a_shebang_is_the_first_line_and_tolerates_a_bom_and_crlf() {
    for source in [
        "#!/usr/bin/env bash\necho hi\n",
        "#!/bin/sh\n",
        "#!/usr/bin/env node\r\nconsole.log(1);\r\n",
        // UTF-8 BOM: an editor writes it and the three bytes would otherwise
        // sit in front of the marker.
        "\u{feff}#!/usr/bin/env python3\nprint(1)\n",
        // No trailing newline at all.
        "#!/bin/sh",
    ] {
        assert!(has_shebang(source), "must be a shebang: {source:?}");
    }
    for source in [
        "",
        "\n#!/bin/sh\n",
        "# comment\n#!/bin/sh\n",
        " #!/bin/sh\n",
        "#comment\n",
        "#\n",
        "#!",
        "//#!/bin/sh\n",
    ] {
        // `"#!"` alone is a degenerate but real shebang line; everything else
        // here is not one.
        let expected = source == "#!";
        assert_eq!(
            has_shebang(source),
            expected,
            "shebang verdict wrong for {source:?}"
        );
    }
}

/// A shebang exempts the file, and a `#!` in a data file does not turn it into
/// code.
#[test]
fn a_shebang_exempts_a_script_and_does_not_rescue_a_data_file() {
    assert_exempt(
        "tools/release.sh",
        "#!/usr/bin/env bash\necho hi\n",
        WiringKind::ScriptEntry,
    );
    assert_exempt(
        "hack/build.mjs",
        "#!/usr/bin/env node\nconsole.log(1);\n",
        WiringKind::ScriptEntry,
    );
    assert_exempt(
        "hack/report.py",
        "#!/usr/bin/env python3\ndef go():\n    pass\n",
        WiringKind::ScriptEntry,
    );
    // A Markdown file whose first line happens to start `#!` is still prose,
    // and must not acquire a `ScriptEntry` annotation on the way past.
    assert_not_code("docs/shebangs.md", "#!/bin/sh is how a script starts\n");
    assert!(
        !extract_wiring_annotations("docs/shebangs.md", "#!/bin/sh\n")
            .iter()
            .any(|annotation| annotation.kind == WiringKind::ScriptEntry),
        "prose must not be annotated as an executable script"
    );
    // And a shell script with no shebang is still an ordinary candidate: the
    // rule reads the file, not the extension.
    assert_candidate("tools/lib.sh", "helper() { echo hi; }\n");
}

/// `__main__.py` has no shebang and is still run by name.
#[test]
fn a_dunder_main_module_is_a_script_entry_without_a_shebang() {
    assert_exempt("pkg/__main__.py", "run()\n", WiringKind::ScriptEntry);
    assert_candidate("pkg/main_.py", "def run():\n    pass\n");
    assert_candidate("pkg/__main_.py", "def run():\n    pass\n");
}

// ---------------------------------------------------------------------------
// Package markers and barrels
// ---------------------------------------------------------------------------

/// Every `__init__.py`, whatever is in it.
///
/// `looks_like_reexport_init` clears only the re-export-*only* shape, and an
/// empty or docstring-only marker is the commoner one: 35 of them were unwired
/// candidates on this repository.
#[test]
fn a_python_package_marker_is_exempt_whatever_it_contains() {
    for source in [
        "",
        "\"\"\"Package.\"\"\"\n",
        "from .core import run\n",
        "__all__ = [\"run\"]\n",
        "class Registry:\n    pass\n",
        "import os\n\nVERSION = os.environ.get(\"V\")\n",
    ] {
        assert!(
            !liveness("app/__init__.py", source).is_candidate(),
            "every package marker is exempt, whatever it holds: {source:?}"
        );
    }
    // A re-export-only marker earns the more specific kind, and a
    // docstring-only one earns the general one. That is precedence, not the
    // order the rules run in — see `FILE_SCOPED_EXEMPT_KINDS`.
    assert_exempt(
        "app/__init__.py",
        "from .core import run\n",
        WiringKind::ReExportPackage,
    );
    assert_exempt(
        "app/__init__.py",
        "\"\"\"Package.\"\"\"\n",
        WiringKind::PackageMarker,
    );
    assert!(package_marker_reason("app/pkg/__init__.py", "").is_some());
    assert!(package_marker_reason("java/com/x/package-info.java", "").is_some());
    // A near-miss name is a module.
    for path in ["app/_init_.py", "app/__init_.py", "app/init.py"] {
        assert!(
            package_marker_reason(path, "").is_none(),
            "{path} is not the package marker"
        );
    }
    // `__init__.py` as a *directory* name is nonsense but must not throw the
    // basename read.
    assert!(package_marker_reason("app/__init__.py/real.py", "").is_none());
}

/// A barrel `index.ts` is one only when it really is one.
#[test]
fn a_barrel_index_is_exempt_and_an_index_holding_code_is_not() {
    for source in [
        "export * from './core';\n",
        "export { run } from './core';\n",
        "// the public surface\nexport * from './a';\nexport * from './b';\n",
        // A JSDoc header. Its continuation lines start with `*`, not with
        // `/*`, and skipping those is a separate arm of the skip condition —
        // one that survived the suite until this line was added, because every
        // other barrel fixture used `//` comments.
        "/**\n * The public surface.\n */\nexport * from './a';\n",
        "import './side-effect';\nexport * from './a';\n",
        "export type { Config } from './types';\n",
    ] {
        assert_exempt("app/widgets/index.ts", source, WiringKind::PackageMarker);
    }
    for source in [
        "export const run = () => 1;\n",
        "export * from './a';\nexport const extra = 2;\n",
        // A re-export spread over two lines is not recognised, and the file
        // stays a candidate. That is the fail-open direction: one finding too
        // many, never one too few.
        "export {\n  run,\n} from './core';\n",
        // Nothing re-exported at all.
        "import './a';\n",
    ] {
        assert!(
            liveness("app/widgets/index.ts", source).is_candidate(),
            "an index holding real code is ordinary product code: {source:?}"
        );
    }
    // A file that is not named `index` is never a barrel, however it is
    // written.
    assert_candidate("app/widgets/public.ts", "export * from './core';\n");
}

// ---------------------------------------------------------------------------
// Tool configuration
// ---------------------------------------------------------------------------

/// The by-convention table, and the words that merely look like it.
#[test]
fn a_tool_config_is_matched_on_the_compound_suffix_not_a_substring() {
    for path in [
        "vite.config.ts",
        "vitest.config.mts",
        "playwright.config.ts",
        "postcss.config.js",
        "tailwind.config.js",
        "eslint.config.js",
        "web/jest.config.cjs",
        "vitest.setup.ts",
        "src/setupTests.ts",
        "webpack.conf.js",
        "build.gradle",
        "android/build.gradle.kts",
        "settings.gradle.kts",
        "Package.swift",
        "ios/Podfile",
        "fastlane/Fastfile",
        "Gemfile",
        "Rakefile",
        "Brewfile",
        "Vagrantfile",
        "Jenkinsfile",
        "Snakefile",
        "setup.py",
        "noxfile.py",
        "fabfile.py",
        "manage.py",
        "app/wsgi.py",
        "app/asgi.py",
        "gunicorn.conf.py",
        "locustfile.py",
        "docs/conf.py",
        "alembic/env.py",
        "db/migrations/env.py",
    ] {
        assert!(
            tool_config_reason(path).is_some(),
            "{path} is read by a tool because of what it is called"
        );
    }
    for path in [
        // Substrings, every one of which is ordinary product code.
        "src/configure.ts",
        "src/config.ts",
        "src/configLoader.ts",
        "src/setupNetwork.ts",
        "src/setup.ts",
        "src/reconf.js",
        // The narrow-scoped names outside the directory that reserves them.
        "app/conf.py",
        "app/env.py",
        "src/settings/env.py",
        // Right name, wrong ecosystem: a `.config.` marker only counts on a
        // JS/TS suffix, or every `x.config.yaml` becomes an entry root.
        "deploy/app.config.yaml",
        "deploy/app.config.xml",
        // Near-misses on the exact names.
        "src/manage.ts",
        "src/setup.py.bak",
        "Makefile",
    ] {
        assert!(
            tool_config_reason(path).is_none(),
            "{path} is ordinary code and must stay a candidate"
        );
    }
}

/// A tool config exempts the file and is an entry root; an ambient declaration
/// exempts the file and is not.
#[test]
fn an_ambient_declaration_is_exempt_without_claiming_to_be_an_entry_point() {
    assert_exempt(
        "vite.config.ts",
        "export default {};\n",
        WiringKind::ToolConfig,
    );
    assert_exempt(
        "src/globals.d.ts",
        "declare const x: number;\n",
        WiringKind::AmbientDeclaration,
    );
    for path in ["src/globals.d.ts", "types/api.d.ts", "a.b.c.d.ts"] {
        assert!(is_ambient_declaration(path), "{path} is ambient");
    }
    for path in [
        // The suffix needs a stem in front of it.
        "d.ts",
        "src/d.ts",
        // And a *dotfile* called `.d.ts` is exactly as long as the suffix, with
        // nothing in front of it. The length bound is `>` and not `>=` for this
        // one file, which is the whole difference between "a declaration file"
        // and "a file whose entire name is the marker".
        ".d.ts",
        "src/.d.ts",
        // A directory called `d.ts` does not make its contents ambient.
        "src/d.ts/index.ts",
        "src/app.ts",
        "src/app.dts",
        "src/appd.ts",
    ] {
        assert!(!is_ambient_declaration(path), "{path} is not ambient");
    }
    // The shortest name that *is* one, so the bound cannot be widened either.
    assert!(is_ambient_declaration("a.d.ts"));
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Fixture trees, by directory segment only.
///
/// 60 of this repository's 138 unwired candidates were `rust-port/testdata/**`.
#[test]
fn a_fixture_directory_is_matched_on_segments_and_not_on_the_filename() {
    for path in [
        "testdata/fixture.py",
        "rust-port/testdata/capabilities/x.ts",
        "app/fixtures/sample.py",
        "app/__fixtures__/sample.ts",
        "app/test-data/sample.py",
        "app/test_data/sample.py",
        "web/__snapshots__/Button.snap.ts",
        "api/golden/expected.py",
        "examples/demo.py",
        "example/demo.py",
        // Case is not a disguise here either.
        "App/Fixtures/Sample.cs",
        "App/TestData/Sample.cs",
        // Windows separators.
        r"app\testdata\sample.py",
    ] {
        assert!(is_fixture_path(path), "{path} is a fixture tree");
    }
    for path in [
        // The last segment is the *file*, not a directory it sits in — the
        // same trap `a_file_named_like_the_jvm_test_directory_is_not_a_test_path`
        // documents.
        "app/testdata",
        "app/fixtures",
        "app/examples",
        // Words that contain a fixture directory's name.
        "app/testdata_loader/run.py",
        "app/fixtures_helper/run.py",
        "app/exampleapp/run.py",
        "app/my_examples/run.py",
    ] {
        assert!(!is_fixture_path(path), "{path} is not a fixture tree");
    }
    assert_exempt(
        "testdata/fixture.py",
        "def sample():\n    return 2\n",
        WiringKind::Fixture,
    );
}

/// The fixture rule is additive: `is_test_path` is unchanged.
///
/// It is pinned equal to `devcouncil.indexing.wiring.is_test_path` by a parity
/// test, so widening it would break the parity rather than fix the finding.
#[test]
fn the_fixture_rule_does_not_widen_the_test_path_rule() {
    for path in [
        "testdata/fixture.py",
        "app/fixtures/sample.py",
        "examples/demo.py",
        "web/__snapshots__/Button.snap.ts",
    ] {
        assert!(
            !is_test_path(path),
            "{path} must reach the fixture rule, not the test-path rule"
        );
    }
    // And everything `is_test_path` claimed, it still claims.
    for path in [
        "tests/test_thing.py",
        "pkg/foo_test.go",
        "app/__tests__/helper.js",
    ] {
        assert!(is_test_path(path), "{path} is still a test path");
    }
}

// ---------------------------------------------------------------------------
// The whole predicate
// ---------------------------------------------------------------------------

/// An exemption is a claim about *this* file, and the two annotations whose
/// target is somewhere else must never be read as one.
///
/// `DynamicImport` records what this file reaches, and `ConfigEntryPoint` names
/// a symbol in another file. Reading either as a file-scoped exemption would
/// clear every module that lazily imports anything — which is most of them.
#[test]
fn an_annotation_pointing_elsewhere_does_not_exempt_the_file_that_carries_it() {
    let lazy = extract_file(
        "app/router.py",
        "import importlib\n\n\ndef load():\n    return importlib.import_module(\"app.plugin\")\n",
    );
    assert!(
        lazy.wiring
            .iter()
            .any(|annotation| annotation.kind == WiringKind::DynamicImport),
        "fixture precondition: the file must carry a dynamic reference"
    );
    assert!(
        lazy.file_liveness().is_candidate(),
        "a file that lazily imports something is not thereby exempt: {:?}",
        lazy.file_liveness()
    );

    let manifest = extract_file(
        "pyproject.toml",
        "[project]\nname = \"fx\"\n\n[project.scripts]\nfx = \"pkg.cli:main\"\n",
    );
    assert!(
        manifest
            .wiring
            .iter()
            .any(|annotation| annotation.kind == WiringKind::ConfigEntryPoint),
        "fixture precondition: the manifest must name a console script"
    );
    // The manifest is data by language, so the entry-point annotations cannot
    // be what clears it — and a `ConfigEntryPoint` on a code file would not
    // clear that either.
    assert!(matches!(
        manifest.file_liveness(),
        FileLiveness::NotCode { .. }
    ));
}

/// When a file earns several exemptions, the one reported is fixed by
/// declared precedence — not by the order the rules happen to run in.
///
/// `manage.py` is the ordinary case: Django's own template writes an
/// `if __name__ == "__main__"` guard, so the file is both a `ScriptEntry` and
/// a `ToolConfig`, and `extract_wiring_annotations` pushes the first of those
/// several rules before the second. Reading `Extraction::wiring` in order
/// would therefore report "Script / binary entry point" for a file whose real
/// answer is "Django management entry point" — and moving a rule up that
/// function would silently change what every artifact says about a file.
///
/// Two mutants survived the suite before this test existed: `&&` to `||` and
/// `==` to `!=` in the annotation lookup. Both leave the *verdict* correct and
/// change only which reason is attached, which is exactly the kind of quiet
/// wrongness a count-based assertion cannot see — the file is still exempt,
/// and the artifact now attributes it to the wrong rule.
#[test]
fn several_exemptions_resolve_by_precedence_and_not_by_rule_order() {
    let source = "def main():\n    pass\n\n\nif __name__ == \"__main__\":\n    main()\n";
    let extraction = extract_file("manage.py", source);
    let kinds: Vec<WiringKind> = extraction
        .wiring
        .iter()
        .filter(|annotation| annotation.target_symbol == "manage.py")
        .map(|annotation| annotation.kind)
        .collect();
    assert!(
        kinds.contains(&WiringKind::ScriptEntry) && kinds.contains(&WiringKind::ToolConfig),
        "fixture precondition: this file must earn both exemptions, in that \
         push order: {kinds:?}"
    );
    assert!(
        kinds.iter().position(|k| *k == WiringKind::ScriptEntry)
            < kinds.iter().position(|k| *k == WiringKind::ToolConfig),
        "fixture precondition: the rules must push in the order precedence \
         overrides, or this test cannot tell the two apart: {kinds:?}"
    );
    assert_exempt("manage.py", source, WiringKind::ToolConfig);

    // The other direction of the same rule: an author's explicit declaration
    // outranks every inference, and it is pushed *after* several of them.
    let vendored = "// devcouncil: allow-unwired\nexport const x = 1;\n";
    assert_exempt("vendor/lib/thing.ts", vendored, WiringKind::AllowUnwired);
}

/// The label is three-valued and every verdict has one.
#[test]
fn every_verdict_publishes_exactly_one_label() {
    let cases = [
        ("README.md", "# x\n", "not_applicable"),
        ("app/__init__.py", "", "exempt"),
        ("app/orphan.py", "def run():\n    pass\n", "candidate"),
    ];
    for (path, source, expected) in cases {
        assert_eq!(liveness(path, source).label(), expected, "{path}");
    }
    // `is_candidate` and the label cannot disagree.
    for (path, source, _) in cases {
        let verdict = liveness(path, source);
        assert_eq!(
            verdict.is_candidate(),
            verdict.label() == "candidate",
            "{path}: the label and the predicate must be one answer"
        );
    }
}

/// A path with no directory part, an empty path, and a path that is nothing but
/// separators do not panic and do not become exemptions by accident.
#[test]
fn degenerate_paths_answer_without_panicking() {
    for path in ["", "/", "//", "./", "a", ".", "..", "   "] {
        // `non_code_path_reason` is the one that reads a basename out of the
        // string, so it is the one that can index past the end.
        let _ = non_code_path_reason(path);
        let _ = tool_config_reason(path);
        let _ = is_fixture_path(path);
        let _ = is_ambient_declaration(path);
        let _ = package_marker_reason(path, "");
    }
    assert!(
        non_code_path_reason("").is_none(),
        "an empty path is not a lockfile"
    );
    assert!(
        non_code_path_reason("/").is_none(),
        "a bare separator has no basename to read"
    );
}
