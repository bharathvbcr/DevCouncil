//! A script's entry-point claim is about the file, not about every symbol in it.
//!
//! A Python file carrying a shebang, an `if __name__ == "__main__"` guard or
//! the name `__main__.py` gets a file-scoped `ScriptEntry` annotation. Go and
//! Rust were narrowed so the annotation stops exempting every symbol of the
//! file (their entry function is exempt by name); Python was not, so an
//! uncalled module constant or helper in a guarded script was never reported
//! (the `v5_accounting.py::F_POSITIONS` shape from the audit).
//!
//! The entry function stays live without the file-level claim: the guarded
//! `main()` is a real call edge from module-level code, a `[project.scripts]`
//! target is a symbol-scoped `ConfigEntryPoint`, and a click command is a
//! decorator-contract exemption. Each is asserted here beside the dead twin.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

/// A finding for `symbol` that nothing spares: what an agent would act on.
fn finding<'a>(
    reports: &'a [DeadSymbolReport],
    file: &str,
    symbol: &str,
) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|report| {
        report.file_path == file && report.symbol_name == symbol && !report.is_exempt
    })
}

const SCRIPT: &str = "\
#!/usr/bin/env python3
import helpers

F_POSITIONS = 3
USED_CONSTANT = 5


def helper_nobody_calls():
    return 1


def used_helper():
    return USED_CONSTANT


def main():
    helpers.shared()
    return used_helper()


if __name__ == \"__main__\":
    main()
";

const HELPERS: &str = "\
def shared():
    return 1


def unused_shared():
    return 2
";

#[test]
fn an_uncalled_constant_in_a_guarded_script_is_reported() {
    let reports = reports(&[("tools/v5.py", SCRIPT), ("tools/helpers.py", HELPERS)]);
    assert!(
        finding(&reports, "tools/v5.py", "F_POSITIONS").is_some(),
        "the script's file-level entry claim hid an uncalled module constant: {reports:#?}"
    );
}

#[test]
fn an_uncalled_helper_in_a_guarded_script_is_reported() {
    let reports = reports(&[("tools/v5.py", SCRIPT), ("tools/helpers.py", HELPERS)]);
    assert!(
        finding(&reports, "tools/v5.py", "helper_nobody_calls").is_some(),
        "{reports:#?}"
    );
}

#[test]
fn the_guarded_call_target_and_what_it_reaches_stay_live() {
    let reports = reports(&[("tools/v5.py", SCRIPT), ("tools/helpers.py", HELPERS)]);
    for live in ["main", "used_helper", "USED_CONSTANT"] {
        assert!(
            finding(&reports, "tools/v5.py", live).is_none(),
            "`{live}` is reached from the guarded entry and must not be reported: {reports:#?}"
        );
    }
    assert!(
        finding(&reports, "tools/helpers.py", "shared").is_none(),
        "{reports:#?}"
    );
    assert!(
        finding(&reports, "tools/helpers.py", "unused_shared").is_some(),
        "the control: an uncalled sibling of an imported helper stays dead: {reports:#?}"
    );
}

#[test]
fn a_module_entry_file_reports_its_dead_symbols_but_not_its_entry() {
    let reports = reports(&[(
        "pkg/__main__.py",
        "ORPHAN = 1\n\n\ndef orphan():\n    return 2\n\n\ndef main():\n    return 3\n\n\nmain()\n",
    )]);
    assert!(
        finding(&reports, "pkg/__main__.py", "ORPHAN").is_some(),
        "{reports:#?}"
    );
    assert!(
        finding(&reports, "pkg/__main__.py", "orphan").is_some(),
        "{reports:#?}"
    );
    assert!(
        finding(&reports, "pkg/__main__.py", "main").is_none(),
        "{reports:#?}"
    );
}

#[test]
fn a_console_script_target_in_a_shebang_script_stays_exempt() {
    // `run` is reached only by the generated launcher; the file also carries a
    // shebang, so it has a file-level `ScriptEntry` too. The symbol-level
    // declaration must carry the entry on its own.
    let manifest = "[project]\nname = \"fx\"\n\n[project.scripts]\nfx = \"pkg.cli:run\"\n";
    let cli = "#!/usr/bin/env python3\nDEAD_CONSTANT = 1\n\n\ndef run():\n    return 0\n\n\ndef stranded():\n    return 1\n";
    let reports = reports(&[
        ("pyproject.toml", manifest),
        ("pkg/__init__.py", ""),
        ("pkg/cli.py", cli),
    ]);
    assert!(
        finding(&reports, "pkg/cli.py", "run").is_none(),
        "{reports:#?}"
    );
    assert!(
        finding(&reports, "pkg/cli.py", "stranded").is_some(),
        "{reports:#?}"
    );
    assert!(
        finding(&reports, "pkg/cli.py", "DEAD_CONSTANT").is_some(),
        "{reports:#?}"
    );
}

#[test]
fn a_click_command_in_a_guarded_script_stays_exempt() {
    let source = "\
import click


@click.command()
def hello():
    click.echo(\"hi\")


if __name__ == \"__main__\":
    hello()
";
    let reports = reports(&[("tools/cli.py", source)]);
    assert!(
        finding(&reports, "tools/cli.py", "hello").is_none(),
        "{reports:#?}"
    );
}

/// The other common spellings of the guarded call: the entry is the argument
/// of `sys.exit`, and subcommands are handed over as values.
#[test]
fn the_other_guarded_entry_shapes_stay_live() {
    let source = "\
#!/usr/bin/env python3
import argparse
import sys

UNUSED_LIMIT = 9


def cmd_run(args):
    return 0


def cmd_never(args):
    return 1


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.set_defaults(func=cmd_run)
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == \"__main__\":
    sys.exit(main())
";
    let reports = reports(&[("tools/run.py", source)]);
    for live in ["main", "cmd_run"] {
        assert!(
            finding(&reports, "tools/run.py", live).is_none(),
            "`{live}` is live: {reports:#?}"
        );
    }
    for dead in ["UNUSED_LIMIT", "cmd_never"] {
        assert!(
            finding(&reports, "tools/run.py", dead).is_some(),
            "`{dead}` is dead: {reports:#?}"
        );
    }
}

/// The same rule for the other languages whose top-level entry call is an
/// edge: a bin script's uncalled helper is reported, its `main` is not.
#[test]
fn javascript_and_typescript_bin_scripts_report_their_dead_helpers() {
    for path in ["bin/tool.js", "bin/tool.ts"] {
        let source = "#!/usr/bin/env node\nfunction helperDead() { return 1; }\nfunction main() { return 2; }\nmain();\n";
        let reports = reports(&[(path, source)]);
        assert!(
            finding(&reports, path, "helperDead").is_some(),
            "{path}: {reports:#?}"
        );
        assert!(
            finding(&reports, path, "main").is_none(),
            "{path}: {reports:#?}"
        );
    }
}

/// A language whose entry has no symbol-level evidence keeps the file-level
/// exemption: a private Ruby method in a shebang script is still spared.
#[test]
fn a_language_without_symbol_level_entry_evidence_keeps_the_file_exemption() {
    let source = "#!/usr/bin/env ruby\nclass Tool\n  def run\n    1\n  end\n  private\n  def helper_dead\n    2\n  end\nend\nTool.new.run\n";
    let reports = reports(&[("bin/tool.rb", source)]);
    assert!(
        finding(&reports, "bin/tool.rb", "Tool.helper_dead").is_none(),
        "{reports:#?}"
    );
}
