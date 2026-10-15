//! Adversarial tests for the parse-budget retry.

use devmap_extract::{
    extract_all_with_budgets, extract_file, extract_file_with_budget, FileRef, ParseBudgets,
};
use std::time::Duration;

const FORCE_RETRY: ParseBudgets = ParseBudgets {
    first_pass: Duration::ZERO,
    retry: Duration::from_secs(60),
};

const NOTEBOOK: &str = r#"{"cells":[{"cell_type":"code","metadata":{},"source":["def a():\n","    return 1\n"],"outputs":[],"execution_count":null}],"metadata":{"kernelspec":{"language":"python","name":"python3"}},"nbformat":4,"nbformat_minor":5}"#;

/// Every budget refusal site must be recognised, or the retry silently skips it.
#[test]
fn a_notebook_refused_for_budget_is_recognised_as_a_budget_refusal() {
    let refused = extract_file_with_budget("nb.ipynb", NOTEBOOK, Duration::ZERO);
    assert!(refused.is_parse_failure());
    println!("NOTEBOOK REASON: {:?}", refused.parse_outcome);
    assert!(
        refused.is_budget_refusal(),
        "notebook budget refusal is not stamped, so the retry never re-runs it"
    );
}

#[test]
fn a_notebook_refused_in_the_parallel_pass_is_recovered() {
    let files = [FileRef {
        path: "nb.ipynb",
        source: NOTEBOOK,
    }];
    let (out, report) = extract_all_with_budgets(&files, None, FORCE_RETRY, None);
    assert!(
        !out[0].is_parse_failure(),
        "notebook stayed refused; report {report:?}"
    );
}

/// Recovered-by-retry must be indistinguishable from a first-pass parse at the
/// production budget (not merely from another long-budget parse).
#[test]
fn recovered_equals_first_pass_at_the_default_budget_across_languages() {
    let fixtures: &[(&str, &str)] = &[
        ("a.py", "import os\nclass C:\n    def m(self):\n        return os.getcwd()\n\ndef f():\n    return C().m()\n"),
        ("b.rs", "use std::fmt;\nstruct S;\nimpl S { fn m(&self) -> u8 { helper() } }\nfn helper() -> u8 { 1 }\n"),
        ("c.go", "package p\nimport \"fmt\"\ntype T struct{}\nfunc (t T) M() { fmt.Println(h()) }\nfunc h() int { return 1 }\n"),
        ("d.ts", "import { x } from './y';\nexport class A { m(): number { return x(); } }\nexport function g() { return new A().m(); }\n"),
        ("e.java", "package p;\nimport java.util.List;\nclass E { void m() { n(); } void n() {} }\n"),
        ("f.js", "const a = require('a');\nfunction f() { return a.g(); }\nmodule.exports = { f };\n"),
    ];
    let files: Vec<FileRef> = fixtures
        .iter()
        .map(|(path, source)| FileRef { path, source })
        .collect();
    let (out, _) = extract_all_with_budgets(&files, None, FORCE_RETRY, None);
    for (ext, (path, source)) in out.iter().zip(fixtures) {
        let first_pass = extract_file(path, source);
        assert!(!first_pass.is_parse_failure(), "{path} baseline failed");
        assert!(
            !ext.is_parse_failure(),
            "{path} not recovered: {:?}",
            ext.parse_outcome
        );
        assert_eq!(
            serde_json::to_value(ext).unwrap(),
            serde_json::to_value(&first_pass).unwrap(),
            "{path}"
        );
    }
}
