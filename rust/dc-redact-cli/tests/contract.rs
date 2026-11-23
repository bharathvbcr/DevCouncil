//! `dcredact` over real invocations: stdin to stdout, one JSON object, and the
//! same bytes the library produces.

use std::io::Write;
use std::process::{Command, Stdio};

use dc_redact::redact_secrets;
use serde_json::Value;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    json: Value,
}

fn run(args: &[&str], stdin: &[u8]) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcredact"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dcredact must start");
    let mut pipe = child.stdin.take().expect("stdin is piped");
    let body = stdin.to_vec();
    // A refusal can happen before the whole request is read, closing the pipe
    // under the writer; that is the behaviour under test, not a failure.
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(&body);
    });
    let output = child.wait_with_output().expect("dcredact must finish");
    writer.join().expect("writer thread");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert_eq!(
        stdout.matches('\n').count(),
        1,
        "stdout must be exactly one line: {stdout:?}"
    );
    let json: Value = serde_json::from_str(stdout.trim_end())
        .unwrap_or_else(|err| panic!("stdout is not one JSON object ({err}): {stdout:?}"));
    assert!(json.is_object(), "stdout must be an object: {stdout:?}");
    Run {
        code: output.status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        json,
    }
}

fn request(text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "text": text })).expect("request renders")
}

/// A credential assembled at run time, so this file is not one.
fn github_token() -> String {
    format!("{}{}{}", "gh", "p_", "Fx7Qa9Lm2Zt4Wc8Rk3Vn6Yb1Hd5Gj0PsFx7Q")
}

fn assert_refused(run: &Run, needle: &str) {
    assert_eq!(
        run.code,
        Some(2),
        "a refusal is exit 2; stderr: {}",
        run.stderr
    );
    assert_eq!(run.json["ok"], false);
    assert_eq!(
        run.json["redaction_ran"], false,
        "a refusal must say redaction did not run"
    );
    assert!(
        run.json.get("text").is_none(),
        "a refusal must never carry text: {}",
        run.stdout
    );
    let error = run.json["error"].as_str().expect("error is a string");
    assert!(error.contains(needle), "error {error:?} lacks {needle:?}");
}

#[test]
fn redacts_a_credential_and_says_redaction_ran() {
    let token = github_token();
    let text = format!("export GH_TOKEN={token}\nplain line\r\nsecond {token} again\n");
    let run = run(&["redact"], &request(&text));
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert_eq!(run.json["ok"], true);
    assert_eq!(run.json["id"], "dcredact");
    assert_eq!(run.json["component"], "dc-redact");
    assert_eq!(run.json["schema_version"], 1);
    assert_eq!(run.json["redaction_ran"], true);
    let out = run.json["text"].as_str().expect("text");
    assert!(!out.contains(&token), "the token survived: {out:?}");
    assert!(
        out.contains("plain line\r\n"),
        "line endings are kept: {out:?}"
    );
    assert_eq!(run.json["redacted_lines"], 2);
    // `GH_TOKEN=<token>` matches two shapes over one span (the vendor token
    // and a credential assigned to a named key). The earlier expectation named
    // only the first, which was the undercount this field used to have.
    assert_eq!(run.json["redacted_spans"], 2);
    assert_eq!(
        run.json["kinds"],
        serde_json::json!([
            "credential assigned to a named key",
            "github personal access token"
        ])
    );
    assert_eq!(run.json["input_bytes"], text.len());
    assert_eq!(run.json["output_bytes"], out.len());
}

/// A second vendor shape, assembled at run time like the first.
fn aws_key_id() -> String {
    format!("{}{}", "AK", "IAQYPD7W3KXN2MZB5T")
}

#[test]
fn counts_every_span_and_names_every_kind_on_one_line() {
    let github = github_token();
    let aws = aws_key_id();
    let text = format!("export A={github} B={aws}\nplain\nthird {github}\n");
    let run = run(&["redact"], &request(&text));
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    let out = run.json["text"].as_str().expect("text");
    assert!(!out.contains(&github) && !out.contains(&aws), "{out:?}");
    assert_eq!(run.json["redacted_lines"], 2);
    assert_eq!(
        run.json["redacted_spans"], 3,
        "two spans on line one and one on line three: {}",
        run.stdout
    );
    assert_eq!(
        run.json["kinds"],
        serde_json::json!(["aws access key id", "github personal access token"]),
        "{}",
        run.stdout
    );
}

/// A chain of JWTs joined by `-` hides one credential behind the next, so each
/// redaction pass can only reach the last. Past the library's pass cap the line
/// is redacted whole, and the span count is then a lower bound on what it held.
#[test]
fn a_line_redacted_whole_after_the_pass_cap_counts_the_pass_spans_and_the_whole_line() {
    let jwt = format!(
        "{}{}.{}.{}",
        "ey",
        "JhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
        "dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
    );
    let chain = vec![jwt.as_str(); 20].join("-");
    let text = format!("seen {chain}\n");
    let run = run(&["redact"], &request(&text));
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    let out = run.json["text"].as_str().expect("text");
    assert!(!out.contains("JhbGci"), "a token survived: {out:?}");
    assert_eq!(
        out.matches("chars)").count(),
        1,
        "one whole-line marker: {out:?}"
    );
    assert_eq!(run.json["redacted_lines"], 1);
    assert_eq!(
        run.json["redacted_spans"], 17,
        "16 pass spans plus the whole-line replacement: {}",
        run.stdout
    );
    assert!(
        !run.json["kinds"].as_array().expect("kinds").is_empty(),
        "{}",
        run.stdout
    );
}

#[test]
fn clean_text_still_reports_that_redaction_ran() {
    let run = run(&["redact"], &request("nothing to hide here\n"));
    assert_eq!(run.code, Some(0));
    assert_eq!(run.json["redaction_ran"], true);
    assert_eq!(run.json["text"], "nothing to hide here\n");
    assert_eq!(run.json["redacted_lines"], 0);
    assert_eq!(run.json["redacted_spans"], 0);
    assert_eq!(run.json["kinds"], serde_json::json!([]));
}

#[test]
fn no_command_means_redact() {
    let run = run(&[], &request("hello"));
    assert_eq!(run.code, Some(0));
    assert_eq!(run.json["redaction_ran"], true);
}

#[test]
fn empty_text_is_a_valid_request() {
    let run = run(&["redact"], &request(""));
    assert_eq!(run.code, Some(0));
    assert_eq!(run.json["text"], "");
    assert_eq!(run.json["redaction_ran"], true);
}

#[test]
fn text_over_the_limit_is_refused_loudly() {
    let text = "a".repeat(dc_redact_cli::MAX_TEXT_BYTES + 1);
    let run = run(&["redact"], &request(&text));
    assert_refused(&run, "limit");
}

#[test]
fn text_at_the_limit_is_accepted() {
    let text = "a".repeat(dc_redact_cli::MAX_TEXT_BYTES);
    let run = run(&["redact"], &request(&text));
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert_eq!(run.json["redaction_ran"], true);
}

#[test]
fn a_request_over_the_read_bound_is_refused_not_truncated() {
    // Valid JSON whose text is tiny, padded past the read bound with
    // whitespace: a reader that stopped at the bound would parse a shorter
    // request and run it.
    let mut body = br#"{"text":"x"}"#.to_vec();
    body.extend(std::iter::repeat_n(
        b' ',
        dc_redact_cli::MAX_REQUEST_BYTES as usize,
    ));
    let run = run(&["redact"], &body);
    assert_refused(&run, "larger than");
}

#[test]
fn malformed_requests_are_refused() {
    for (label, body) in [
        ("not json", &b"not json"[..]),
        ("empty", &b""[..]),
        ("blank", &b"  \n"[..]),
        ("array", &b"[1]"[..]),
        ("missing text", &br#"{}"#[..]),
        ("text is not a string", &br#"{"text":5}"#[..]),
        ("text is null", &br#"{"text":null}"#[..]),
        ("unknown field", &br#"{"text":"a","extra":1}"#[..]),
        ("duplicate-free but truncated", &br#"{"text":"a""#[..]),
        ("invalid utf-8", &b"{\"text\":\"\xff\"}"[..]),
    ] {
        let run = run(&["redact"], body);
        assert_eq!(run.code, Some(2), "{label}: {}", run.stdout);
        assert_eq!(run.json["ok"], false, "{label}");
        assert_eq!(run.json["redaction_ran"], false, "{label}");
        assert!(run.json.get("text").is_none(), "{label}: {}", run.stdout);
    }
}

#[test]
fn an_unknown_command_is_refused_without_reading_stdin() {
    let run = run(&["frobnicate"], b"");
    assert_refused(&run, "unknown command");
}

#[test]
fn version_and_health_answer_without_a_request() {
    for flag in ["--version", "-V", "-v", "version"] {
        let run = run(&[flag], b"");
        assert_eq!(run.code, Some(0), "{flag}");
        assert_eq!(run.json["id"], "dcredact");
        assert_eq!(run.json["component"], "dc-redact");
        assert_eq!(run.json["version"], env!("CARGO_PKG_VERSION"));
    }
    let health = run(&["health"], b"");
    assert_eq!(health.code, Some(0));
    assert_eq!(health.json["ok"], true);
    assert_eq!(health.json["id"], "dcredact");
    assert_eq!(health.json["schema_version"], 1);
    assert_eq!(
        health.json["limits"]["max_text_bytes"],
        dc_redact_cli::MAX_TEXT_BYTES
    );
    assert_eq!(
        health.json["limits"]["max_request_bytes"],
        dc_redact_cli::MAX_REQUEST_BYTES
    );
}

/// Every line of the dc-verify secret corpus (the labelled secret and clean
/// lines the secret gate is measured on) plus the Google OAuth fixtures, run
/// through the binary one text at a time and as a single multi-line text.
#[test]
fn output_equals_the_library_over_the_existing_fixtures() {
    let corpus = include_str!("../../dc-verify/tests/corpus/secrets.tsv");
    let holdout = include_str!("../../dc-verify/tests/corpus/secrets_holdout.tsv");
    let holdout_2 = include_str!("../../dc-verify/tests/corpus/secrets_holdout_2.tsv");
    let mut lines: Vec<String> = [corpus, holdout, holdout_2]
        .iter()
        .flat_map(|file| file.lines())
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.splitn(4, '\t').nth(3).map(str::to_string))
        .collect();
    for token in [
        concat!("ya", "29.", "a0FAKEfake-Token_notReal.0123456789abcdefXYZ"),
        concat!("ya", "29.", "dr.", "FAKEfake-0123456789abcdefghijKLMN"),
        concat!("1/", "/", "0gFAKEfakeRefresh-Token_notReal0123456789ab"),
    ] {
        lines.push(token.to_string());
        lines.push(format!(r#"{{"detail":"{token}","code":401}}"#));
        lines.push(format!(
            "GET https://example.invalid/cb?v={token}&scope=email HTTP/1.1"
        ));
    }
    assert!(
        lines.len() > 100,
        "the corpus did not load: {} lines",
        lines.len()
    );

    let mut redacted_any = false;
    for line in &lines {
        let run = run(&["redact"], &request(line));
        assert_eq!(run.code, Some(0), "{line:?}: {}", run.stdout);
        let want = redact_secrets(line);
        redacted_any |= want != *line;
        assert_eq!(
            run.json["text"],
            want.as_str(),
            "binary and library disagree on {line:?}"
        );
        assert_eq!(run.json["redaction_ran"], true);
    }
    assert!(
        redacted_any,
        "the corpus redacted nothing, so this proved nothing"
    );

    let whole = lines.join("\n") + "\n";
    let run = run(&["redact"], &request(&whole));
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.json["text"], redact_secrets(&whole).as_str());
}
