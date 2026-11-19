//! Vendor key prefixes whose families the table claimed but missed a member of.
//!
//! The legacy `sk-` rule needs an alphanumeric body, so OpenAI's service-account
//! keys (`sk-svcacct-…`, a body with `-` and `_`) passed it. Groq, Atlassian and
//! Databricks keys had no entry at all.
//!
//! Formats come from the rules other open scanners publish, since none of these
//! vendors documents its key shape; each source is named beside its entry in
//! `SECRET_PATTERNS`. Every fixture is built from pieces with `concat!`, so no
//! line of this file is a credential to a scanner that reads it, and every body
//! is made of repeated filler, never a real key.

use dc_redact::{contains_secret, find_secret, redact_secrets};

/// (name the gate reports, the whole fixture token, its body after the prefix)
fn fixtures() -> Vec<(&'static str, String, String)> {
    let svcacct_body = "Ab1_".repeat(30);
    let gsk_body = "Qw3rTy9L".repeat(7)[..52].to_string();
    let atlassian_body = "Zx8-Cv_b=".repeat(21)[..186].to_string();
    let dapi_body = "0f1e2d3c".repeat(4);
    vec![
        (
            "openai service account api key",
            format!("{}{svcacct_body}", concat!("sk-", "svcacct-")),
            svcacct_body,
        ),
        (
            "groq api key",
            format!("{}{gsk_body}", concat!("gs", "k_")),
            gsk_body,
        ),
        (
            "atlassian api token",
            format!("{}{atlassian_body}", concat!("ATA", "TT3")),
            atlassian_body,
        ),
        (
            "databricks personal access token",
            format!("{}{dapi_body}", concat!("da", "pi")),
            dapi_body,
        ),
    ]
}

#[test]
fn each_missed_family_member_is_found_named_and_redacted_in_every_carrier() {
    let mut failures = Vec::new();
    for (name, token, body) in fixtures() {
        for (place, line) in [
            ("own line", token.clone()),
            ("log line", format!("DEBUG request signed with {token} ok")),
            (
                "json value",
                format!("{{\"detail\":\"{token}\",\"code\":401}}"),
            ),
            (
                "shell header",
                format!("curl -H 'X-Key: {token}' https://example.invalid"),
            ),
        ] {
            match find_secret(&line) {
                Some(found) if found.name == name => {}
                other => failures.push(format!(
                    "{name} / {place}: find_secret gave {:?}",
                    other.map(|m| m.name)
                )),
            }
            let redacted = redact_secrets(&line);
            if redacted.contains(&body)
                || contains_secret(&redacted)
                || redact_secrets(&redacted) != redacted
            {
                failures.push(format!("{name} / {place}: bad redaction {redacted:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The shapes the prefixes share with ordinary text and identifiers.
#[test]
fn identifiers_prose_and_short_values_are_not_keys() {
    let long_hex = "0f1e2d3c".repeat(4);
    let mut lines = vec![
        // A prefix inside a longer word is a word.
        format!("risk-svcacct-{}", "Ab1_".repeat(30)),
        format!("task_gsk_{}", "Qw3rTy9L".repeat(7)),
        format!("badapi{long_hex}"),
        // Prose that names the prefixes.
        "service account keys begin sk-svcacct- and Groq keys begin gsk_".to_string(),
        "Databricks tokens look like dapi followed by hex".to_string(),
        // Too short for the family.
        "sk-svcacct-abc".to_string(),
        "gsk_abc123".to_string(),
        "ATATT3xyz".to_string(),
        format!("{}{}", concat!("da", "pi"), &long_hex[..16]),
        // The right prefix with a body of the wrong alphabet: an identifier.
        format!(
            "{}{}",
            concat!("gs", "k_"),
            "config_value_for_each_worker_in_region_x_y"
        ),
        format!(
            "{}{}",
            concat!("da", "pi"),
            "ClientFactoryBuilderHelperForEveryTenant"
        ),
    ];
    lines.push(format!("{}{long_hex}-x", concat!("da", "pi")));
    let found: Vec<_> = lines.iter().filter(|line| contains_secret(line)).collect();
    assert!(found.is_empty(), "false positives: {found:?}");
}

#[test]
fn a_databricks_token_may_carry_a_numeric_suffix() {
    let token = format!("{}{}-2", concat!("da", "pi"), "0f1e2d3c".repeat(4));
    let redacted = redact_secrets(&format!("DATABRICKS_TOKEN={token}"));
    assert!(
        !redacted.contains("0f1e2d3c0f1e2d3c") && !redacted.contains("-2"),
        "{redacted}"
    );
}
