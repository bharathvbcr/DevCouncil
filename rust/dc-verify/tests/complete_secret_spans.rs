use dc_verify::rigor::{contains_secret, detect_stubs, redact_secrets, scan_secrets};
use dc_verify::{ChangeStatus, FileDiff};

#[test]
fn every_family_checks_later_candidates_and_redacts_distinct_values() {
    for prefix in [
        "sk-ant-",
        "sk-proj-",
        "sk_live_",
        "sk-",
        "xai-",
        "AIza",
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "ghr_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xapp-",
        "AKIA",
        "ASIA",
        "hf_",
        "npm_",
    ] {
        let first = format!("{prefix}{}", "A".repeat(48));
        let second = format!("{prefix}{}", "B".repeat(48));
        let line = format!("é {prefix}short \"{first}\", '{second}' {first}\n");
        assert!(
            contains_secret(&line),
            "invalid prefix hid later {prefix} candidates"
        );
        let redacted = redact_secrets(&line);
        assert!(
            !redacted.contains(&first) && !redacted.contains(&second),
            "a distinct {prefix} value survived redaction"
        );
        assert_eq!(
            redact_secrets(&redacted),
            redacted,
            "redaction must be idempotent for {prefix}"
        );
        assert!(
            !contains_secret(&redacted),
            "{prefix} replacement must not look like a credential"
        );
        assert!(redacted.starts_with("é ") && redacted.ends_with('\n'));
    }
}

#[test]
fn invalid_first_prefix_cannot_leak_a_later_value_through_evidence() {
    let secret = format!("ghp_{}", "A".repeat(36));
    let file = FileDiff {
        path: "src/example.rs".into(),
        old_path: None,
        status: ChangeStatus::Modified,
        added_lines: vec![(1, format!("// TODO ghp_short '{secret}'"))],
        removed_count: 0,
    };
    assert_eq!(scan_secrets(std::slice::from_ref(&file)).len(), 1);
    let stubs = detect_stubs(&[file]);
    assert_eq!(stubs.len(), 1);
    assert!(stubs[0].evidence.contains("withheld"));
    assert!(!stubs[0].evidence.contains(&secret));
}

#[test]
fn mixed_families_and_private_headers_redact_without_rewriting_safe_text() {
    let text = format!(
        "-----BEGIN CERTIFICATE----- -----BEGIN RSA PRIVATE KEY-----\n'ghp_{}' 'sk-ant-{}'",
        "A".repeat(48),
        "B".repeat(48)
    );
    let redacted = redact_secrets(&text);
    assert!(redacted.contains("-----BEGIN CERTIFICATE-----"));
    assert!(!redacted.contains(&"A".repeat(48)) && !redacted.contains(&"B".repeat(48)));
    assert_eq!(redact_secrets(&redacted), redacted);
    assert!(!contains_secret(&redacted));
    let safe =
        "task-long-safe-name disk-long-safe-name risk-long-safe-name\n-----BEGIN PUBLIC KEY-----";
    assert!(!contains_secret(safe));
    assert_eq!(redact_secrets(safe), safe);
}
