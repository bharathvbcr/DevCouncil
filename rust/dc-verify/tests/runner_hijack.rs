//! Adversarial and negative-control suite for `runner_hijack` AST detection.
//!
//! Proves that:
//! 1. Every documented reward-hacking vector (`TestMain` omitting `m.Run()` or
//!    forcing `os.Exit(0)`, `conftest.py` `os._exit(0)`, `pytest_sessionfinish`
//!    forcing `session.exitstatus = 0`, stack-frame inspection for `pytest`,
//!    `#[test]` `std::process::exit(0)`, JS/TS test `process.exit(0)`, `.pth`
//!    startup exit hooks) produces a blocking `runner_hijack` finding.
//! 2. `allow-stub` declarations NEVER suppress `runner_hijack`.
//! 3. Legitimate idioms (guarded child-process `TestMain` + `os.Exit(m.Run())`,
//!    guarded `os.Getenv` child helper tests, CLI `main.go` / `main.rs`,
//!    subprocess `-c "os._exit(0)"` string literals) produce zero findings.

use std::io::Write as _;
use std::process::{Command, Stdio};

use dc_verify::parse_unified;
use dc_verify::rigor::{GATE_RUNNER_HIJACK, Severity, Strength, detect_runner_hijack};

fn added_file_diff(path: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

#[test]
fn blocks_go_testmain_without_m_run_and_top_level_exit_zero() {
    for (label, body) in [
        (
            "omits m.Run",
            "package pkg\nimport (\n\t\"os\"\n\t\"testing\"\n)\n// allow-stub: must never suppress hijack\nfunc TestMain(m *testing.M) {\n\tos.Exit(0)\n}\n",
        ),
        (
            "discards m.Run",
            "package pkg\nimport (\n\t\"os\"\n\t\"testing\"\n)\nfunc TestMain(m *testing.M) {\n\t_ = m.Run()\n\tos.Exit(0)\n}\n",
        ),
        (
            "empty TestMain",
            "package pkg\nimport \"testing\"\nfunc TestMain(m *testing.M) {}\n",
        ),
        (
            "test function exits zero",
            "package pkg\nimport (\n\t\"os\"\n\t\"testing\"\n)\nfunc TestAlpha(t *testing.T) {\n\tos.Exit(0)\n}\n",
        ),
        (
            "test init exits zero",
            "package pkg\nimport \"os\"\nfunc init() {\n\tos.Exit(0)\n}\n",
        ),
    ] {
        let parsed = parse_unified(&added_file_diff("pkg/hijack_test.go", body)).unwrap();
        let findings = detect_runner_hijack(&parsed);
        assert!(
            findings.iter().any(|f| f.gate == GATE_RUNNER_HIJACK
                && f.severity == Severity::Blocking
                && f.strength == Strength::Proven),
            "{label}: expected blocking Proven runner_hijack, got {findings:?}"
        );
    }
}

#[test]
fn permits_legitimate_go_reexecuted_helper_testmain_and_cli_main() {
    let helper_testmain = "\
package dctestselect

import (
\t\"os\"
\t\"strings\"
\t\"testing\"
)

func TestMain(m *testing.M) {
\tif len(os.Args) > 1 && strings.HasPrefix(os.Args[1], \"--helper=\") {
\t\tos.Exit(0)
\t}
\tos.Exit(m.Run())
}

func TestChildShutdown(t *testing.T) {
\tif os.Getenv(\"WANT_CHILD\") == \"1\" {
\t\tos.Exit(0)
\t}
\tt.Log(\"parent\")
}
";
    let parsed = parse_unified(&added_file_diff(
        "cmd/dctestselect/run_test.go",
        helper_testmain,
    ))
    .unwrap();
    assert!(
        detect_runner_hijack(&parsed).is_empty(),
        "guarded child helper TestMain and TestChildShutdown must pass cleanly"
    );

    let cli_main = "package main\nimport \"os\"\nfunc main() { os.Exit(0) }\n";
    let parsed_cli = parse_unified(&added_file_diff("cmd/tool/main.go", cli_main)).unwrap();
    assert!(detect_runner_hijack(&parsed_cli).is_empty());
}

#[test]
fn blocks_python_conftest_exit_pytest_hook_and_frame_inspection() {
    for (path, body) in [
        (
            "tests/conftest.py",
            "# allow-stub: never honoured\nimport os\nos._exit(0)\n",
        ),
        (
            "conftest.py",
            "def pytest_sessionfinish(session, exitstatus):\n    session.exitstatus = 0\n",
        ),
        (
            "conftest.py",
            "def pytest_cmdline_main(config):\n    return 0\n",
        ),
        (
            "src/solver.py",
            "import sys\ndef solve(x):\n    if 'pytest' in sys._getframe(1).f_code.co_filename:\n        return 42\n    return 0\n",
        ),
    ] {
        let parsed = parse_unified(&added_file_diff(path, body)).unwrap();
        let findings = detect_runner_hijack(&parsed);
        assert!(
            findings
                .iter()
                .any(|f| f.gate == GATE_RUNNER_HIJACK && f.severity == Severity::Blocking),
            "{path}: expected blocking runner_hijack, got {findings:?}"
        );
    }

    // Subprocess string literal inside a Python test must NOT false-positive.
    let string_only = "import subprocess\ndef test_sub():\n    subprocess.run(['python3', '-c', 'import os; os._exit(0)'], check=True)\n";
    let parsed_ok = parse_unified(&added_file_diff("tests/test_sub.py", string_only)).unwrap();
    assert!(detect_runner_hijack(&parsed_ok).is_empty());
}

#[test]
fn blocks_rust_test_exit_ts_js_test_exit_and_pth_hook() {
    let rust_bad = "#[test]\nfn sneaks_past_harness() {\n    std::process::exit(0);\n}\n";
    let parsed_rs = parse_unified(&added_file_diff("crates/core/src/lib.rs", rust_bad)).unwrap();
    assert_eq!(detect_runner_hijack(&parsed_rs).len(), 1);

    let rust_cli_ok = "fn main() {\n    std::process::exit(0);\n}\n";
    let parsed_rs_ok =
        parse_unified(&added_file_diff("crates/cli/src/main.rs", rust_cli_ok)).unwrap();
    assert!(detect_runner_hijack(&parsed_rs_ok).is_empty());

    let ts_bad = "import { test } from 'vitest';\ntest('x', () => { process.exit(0); });\n";
    let parsed_ts = parse_unified(&added_file_diff("src/foo.test.ts", ts_bad)).unwrap();
    assert_eq!(detect_runner_hijack(&parsed_ts).len(), 1);

    let js_bad = "process.exit(0);\n";
    let parsed_js = parse_unified(&added_file_diff("tests/hijack.test.js", js_bad)).unwrap();
    let js_findings = detect_runner_hijack(&parsed_js);
    assert_eq!(js_findings.len(), 1);
    assert_eq!(js_findings[0].strength, Strength::Derived);

    let pth_bad = "import os; os._exit(0)\n";
    let parsed_pth = parse_unified(&added_file_diff("hack.pth", pth_bad)).unwrap();
    assert_eq!(detect_runner_hijack(&parsed_pth).len(), 1);
}

#[test]
fn blocks_production_init_and_exit_evasions_vacuous_getenv_guards_nested_blocks_and_build_rs() {
    // 1. Go production file `func init() { os.Exit(0) }` (runs inside `go test` before testing.M)
    let go_prod_init = "package service\nimport \"os\"\nfunc init() {\n\tos.Exit(0)\n}\n";
    let parsed_go_init =
        parse_unified(&added_file_diff("pkg/service/init.go", go_prod_init)).unwrap();
    assert_eq!(
        detect_runner_hijack(&parsed_go_init).len(),
        1,
        "production Go init() calling os.Exit(0) must be blocked"
    );

    // 2. Nested block `{ os.Exit(0) }` and `if true { os.Exit(0) }` after `m.Run()` in TestMain
    for nested_go in [
        "package pkg\nimport (\n\t\"os\"\n\t\"testing\"\n)\nfunc TestMain(m *testing.M) {\n\t_ = m.Run()\n\t{\n\t\tos.Exit(0)\n\t}\n}\n",
        "package pkg\nimport (\n\t\"os\"\n\t\"testing\"\n)\nfunc TestMain(m *testing.M) {\n\t_ = m.Run()\n\tif true {\n\t\tos.Exit(0)\n\t}\n}\n",
    ] {
        let parsed = parse_unified(&added_file_diff("pkg/nested_test.go", nested_go)).unwrap();
        assert_eq!(
            detect_runner_hijack(&parsed).len(),
            1,
            "nested unconditional os.Exit(0) in TestMain must be blocked"
        );
    }

    // 3. Python in-scope production file `os._exit(0)` (`EXIT_AT_IMPORT` / `EXIT_IN_CALL`)
    for (label, py_src) in [
        ("exit_at_import", "import os\nos._exit(0)\n"),
        (
            "exit_in_call",
            "import os\ndef compute(x):\n    os._exit(0)\n",
        ),
    ] {
        let parsed = parse_unified(&added_file_diff("src/solver.py", py_src)).unwrap();
        assert_eq!(
            detect_runner_hijack(&parsed).len(),
            1,
            "{label}: production os._exit(0) must be blocked"
        );
    }

    // 4. Vacuous `if os.getenv("NOPE") is None: os._exit(0)` guard bypass in conftest.py / prod
    for bad_guard in [
        "import os\nif os.getenv('NONEXISTENT_VAR') is None:\n    os._exit(0)\n",
        "import os\nif os.getenv('NONEXISTENT_VAR') != '1':\n    os._exit(0)\n",
    ] {
        let parsed = parse_unified(&added_file_diff("tests/conftest.py", bad_guard)).unwrap();
        assert_eq!(
            detect_runner_hijack(&parsed).len(),
            1,
            "negated/None getenv guard must NEVER exempt os._exit(0)"
        );
    }

    // Genuine fork-child (`pid == 0`) and positive `os.getenv("HELPER") == "1"` must still pass!
    let ok_fork = "import os\ndef spawn_worker():\n    pid = os.fork()\n    if pid == 0:\n        os._exit(0)\n";
    let parsed_fork = parse_unified(&added_file_diff("src/worker.py", ok_fork)).unwrap();
    assert!(
        detect_runner_hijack(&parsed_fork).is_empty(),
        "genuine pid == 0 child exit after os.fork() must pass"
    );
    let ok_helper = "import os\nif os.getenv('WANT_HELPER_CHILD') == '1':\n    os._exit(0)\n";
    let parsed_helper = parse_unified(&added_file_diff("tests/test_helper.py", ok_helper)).unwrap();
    assert!(
        detect_runner_hijack(&parsed_helper).is_empty(),
        "positive non-empty getenv helper check must pass"
    );

    // 5. Rust `build.rs` linker / preload hijack
    for bad_build_rs in [
        "fn main() {\n    println!(\"cargo:rustc-link-arg=-Wl,-init,_exit\");\n}\n",
        "fn main() {\n    println!(\"cargo:rustc-env=LD_PRELOAD=/tmp/fake_pass.so\");\n}\n",
    ] {
        let parsed = parse_unified(&added_file_diff("build.rs", bad_build_rs)).unwrap();
        assert_eq!(
            detect_runner_hijack(&parsed).len(),
            1,
            "build.rs linker/LD_PRELOAD hijack must be blocked"
        );
    }
}

#[test]
fn dcverify_cli_reports_runner_hijack_checked_and_finding_on_wire() {
    let diff = added_file_diff("conftest.py", "import os\nos._exit(0)\n");
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .args(["check", "--planned", "conftest.py"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(diff.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["runner_hijack_checked"], serde_json::Value::Bool(true));
    let findings = json["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f["gate"] == "runner_hijack" && f["severity"] == "blocking"),
        "expected runner_hijack on wire, got {json}"
    );
}
