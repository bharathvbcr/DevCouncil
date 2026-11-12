//! Rigor gate tests for Lean 4 (`.lean`) Trusted Computing Base (TCB) hole detection.

use std::io::Write;
use std::process::{Command, Stdio};

use dc_verify::parse_unified;
use dc_verify::rigor::{GATE_LEAN_TCB, Severity, Strength, detect_lean_tcb, detect_lean_tcb_with};

fn added_lean_diff(path: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for l in lines {
        diff.push('+');
        diff.push_str(l);
        diff.push('\n');
    }
    diff
}

#[test]
fn detect_lean_tcb_blocks_every_soundness_escape_and_never_allows_stub_suppression() {
    let cases = [
        (
            "sorry tactic",
            "-- allow-stub: proof deferred\ntheorem bad (n : Nat) : n = n + 1 := by\n  sorry\n",
        ),
        (
            "kernel sorryAx",
            "theorem bad : False := sorryAx False false\n",
        ),
        ("admit tactic", "theorem bad : False := by admit\n"),
        (
            "native_decide",
            "theorem bad : 2 + 2 = 4 := by native_decide\n",
        ),
        (
            "Lean.ofReduceBool",
            "theorem bad : True := Lean.ofReduceBool true true rfl\n",
        ),
        (
            "Lean.trustCompiler",
            "theorem bad : True := Lean.trustCompiler True\n",
        ),
        ("axiom", "axiom unverified_assumption : False\n"),
        ("opaque", "opaque hidden_witness : Nat\n"),
        (
            "unsafe def",
            "unsafe def reinterpretCast (x : Nat) : Nat := x\n",
        ),
        (
            "@[extern]",
            "@[extern \"c_sha512\"]\ndef sha512 (x : Nat) : Nat := x\n",
        ),
        (
            "@[implemented_by]",
            "def fast (x : Nat) : Nat := x\n@[implemented_by fast]\ndef spec (x : Nat) : Nat := x\n",
        ),
        ("@[csimp]", "@[csimp] theorem subst_eq : id = id := rfl\n"),
        (
            "unclosed block comment",
            "theorem ok : True := trivial\n/- unclosed comment hiding sorry\n",
        ),
        (
            "elab tactic escape",
            "elab \"forge\" : tactic => pure ()\ntheorem bad : False := by forge\n",
        ),
        (
            "macro_rules escape",
            "macro_rules | `(tactic| cheat) => `(tactic| trivial)\n",
        ),
        (
            "#eval escape",
            "#eval (1 + 1)\ntheorem ok : True := trivial\n",
        ),
        (
            "@[tactic] attribute escape",
            "@[tactic myTac]\ndef myTacImpl : Nat := 0\n",
        ),
    ];

    for (label, body) in cases {
        let diff = added_lean_diff("specs/Spec.lean", body);
        let parsed = parse_unified(&diff).expect("parse unified diff");
        let findings = detect_lean_tcb(&parsed);
        assert!(
            findings.iter().any(|f| {
                f.gate == GATE_LEAN_TCB
                    && f.severity == Severity::Blocking
                    && f.strength == Strength::Proven
            }),
            "{label}: expected blocking Proven lean_tcb finding, got {findings:?}"
        );
    }
}

#[test]
fn detect_lean_tcb_accepts_sound_proofs_and_ignores_comment_or_string_mentions() {
    let clean = "\
import Init.Data.BitVec

/- Nested comment /- mentioning sorry admit axiom @[extern] -/ still comment -/
-- Single-line comment mentioning sorryAx and native_decide
namespace Proto

def msg : String := \"sorry admit native_decide axiom\"

theorem msg_refl : msg = msg := by
  rfl

end Proto
";
    let diff = added_lean_diff("specs/Proto.lean", clean);
    let parsed = parse_unified(&diff).expect("parse unified diff");
    let findings = detect_lean_tcb(&parsed);
    assert!(
        findings.is_empty(),
        "clean Lean 4 proof must emit zero lean_tcb findings, got {findings:?}"
    );
}

#[test]
fn detect_lean_tcb_with_post_image_flags_added_extern_attr_on_existing_def() {
    // Existing file has `def kdf : Nat := 1`; diff adds `@[extern "evil_kdf"]` above it.
    let diff = "\
diff --git a/specs/Kdf.lean b/specs/Kdf.lean
--- a/specs/Kdf.lean
+++ b/specs/Kdf.lean
@@ -1,2 +1,3 @@
 namespace Kdf
+@[extern \"evil_kdf\"]
 def kdf : Nat := 1
";
    let post = "namespace Kdf\n@[extern \"evil_kdf\"]\ndef kdf : Nat := 1\n";
    let parsed = parse_unified(diff).expect("parse unified diff");
    let findings = detect_lean_tcb_with(&parsed, &|_| Some(post.to_string()));
    assert_eq!(findings.len(), 1, "expected 1 finding, got {findings:?}");
    assert_eq!(findings[0].gate, GATE_LEAN_TCB);
    assert_eq!(findings[0].line, 2);
}

#[test]
fn dcverify_cli_reports_lean_tcb_checked_and_blocking_finding_on_wire() {
    let diff = added_lean_diff(
        "specs/Broken.lean",
        "theorem broken : False := by\n  sorry\n",
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .args(["check", "--planned", "specs/Broken.lean"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dcverify");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(diff.as_bytes())
        .expect("write diff");
    let out = child.wait_with_output().expect("wait dcverify");
    assert!(out.status.success(), "dcverify failed: {:?}", out);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("parse wire json");
    assert_eq!(json["lean_tcb_checked"], serde_json::Value::Bool(true));
    let findings = json["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|f| f["gate"] == "lean_tcb"
            && f["severity"] == "blocking"
            && f["strength"] == "proven"),
        "expected blocking proven lean_tcb finding on wire, got {json}"
    );
}

#[cfg(unix)]
#[test]
fn dcverify_cli_honors_dc_verify_lean_kernel_require_and_auto() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "dcverify-lean-kernel-{}-{stamp}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let empty_bin = base.join("empty-bin");
    let mock_bin = base.join("mock-bin");
    let repo_dir = base.join("repo");
    fs::create_dir_all(&empty_bin).unwrap();
    fs::create_dir_all(&mock_bin).unwrap();
    fs::create_dir_all(repo_dir.join("specs")).unwrap();

    let clean_spec = "namespace Spec\ntheorem refl_ok : 1 = 1 := rfl\nend Spec\n";
    fs::write(repo_dir.join("specs/Clean.lean"), clean_spec).unwrap();
    let diff = added_lean_diff("specs/Clean.lean", clean_spec);

    let run_dcverify = |path_env: &str, mode: &str, extra: &[(&str, &str)]| -> serde_json::Value {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dcverify"));
        cmd.env("PATH", path_env)
            .env("DC_VERIFY_LEAN_KERNEL", mode)
            .args([
                "check",
                "--planned",
                "specs/Clean.lean",
                "--root",
                repo_dir.to_str().unwrap(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn dcverify");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(diff.as_bytes())
            .expect("write diff");
        let out = child.wait_with_output().expect("wait dcverify");
        assert!(out.status.success(), "dcverify failed: {:?}", out);
        serde_json::from_slice(&out.stdout).expect("parse json")
    };

    // 1. `DC_VERIFY_LEAN_KERNEL=require` with missing `lean` MUST emit blocking `lean_tcb` finding!
    let missing_json = run_dcverify(empty_bin.to_str().unwrap(), "require", &[]);
    let missing_findings = missing_json["findings"].as_array().unwrap();
    assert!(
        missing_findings
            .iter()
            .any(|f| f["gate"] == "lean_tcb" && f["severity"] == "blocking"),
        "DC_VERIFY_LEAN_KERNEL=require must fail closed when lean is missing: {missing_json}"
    );

    // 2. `DC_VERIFY_LEAN_KERNEL=auto` with missing `lean` falls back to lexical cleanly on sound file.
    let auto_missing_json = run_dcverify(empty_bin.to_str().unwrap(), "auto", &[]);
    assert_eq!(auto_missing_json["findings"].as_array().unwrap().len(), 0);

    // 3. Install mock `lean` + `lean4checker`.
    let write_exec = |p: PathBuf, s: &str| {
        fs::write(&p, s).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    };
    write_exec(
        mock_bin.join("lean"),
        "#!/bin/sh\nif [ \"$1\" = \"--print-libdir\" ]; then echo /tmp; exit 0; fi\nif [ \"$1\" = \"-o\" ]; then printf 'olean' > \"$2\"; exit 0; fi\nif [ \"${MOCK_BAD_AX:-0}\" = \"1\" ]; then\n  echo \"'Spec.refl_ok' depends on axioms: [sorryAx]\"\n  exit 0\nfi\necho \"'Spec.refl_ok' does not depend on any axioms\"\nexit 0\n",
    );
    write_exec(
        mock_bin.join("lean4checker"),
        "#!/bin/sh\nif [ \"$1\" != \"--fresh\" ]; then exit 2; fi\nif [ \"${MOCK_BAD_CHK:-0}\" = \"1\" ]; then echo 'bad env' >&2; exit 1; fi\nexit 0\n",
    );
    let sys_path = format!("{}:/bin:/usr/bin", mock_bin.display());

    // Passing `lean` + `lean4checker` under `require` emits 0 findings!
    let pass_json = run_dcverify(&sys_path, "require", &[]);
    assert_eq!(
        pass_json["findings"].as_array().unwrap().len(),
        0,
        "expected clean pass under require: {pass_json}"
    );

    // Hidden `sorryAx` caught by `#print axioms` under `require` (and `auto`) emits blocking `lean_tcb` finding!
    let ax_json = run_dcverify(&sys_path, "require", &[("MOCK_BAD_AX", "1")]);
    assert!(
        ax_json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["gate"] == "lean_tcb" && f["severity"] == "blocking"),
        "expected blocking lean_tcb finding on sorryAx: {ax_json}"
    );

    // `lean4checker --fresh` rejection emits blocking `lean_tcb` finding!
    let chk_json = run_dcverify(&sys_path, "require", &[("MOCK_BAD_CHK", "1")]);
    assert!(
        chk_json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["gate"] == "lean_tcb" && f["severity"] == "blocking"),
        "expected blocking lean_tcb finding on lean4checker rejection: {chk_json}"
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn dcverify_cli_real_lean_kernel_and_lean4checker_when_required() {
    if !std::env::var("DC_VERIFY_LEAN_KERNEL")
        .ok()
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("require"))
    {
        return;
    }

    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static REAL_SEQ: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let repo_dir = std::env::temp_dir().join(format!(
        "dcverify-real-lean-{}-{stamp}-{}",
        std::process::id(),
        REAL_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo_dir.join("specs")).unwrap();

    // Case A: Sound proof must pass real `lean`, `#print axioms`, and `lean4checker --fresh`.
    let valid_spec =
        "namespace LiveCheck\ntheorem add_comm_two : 1 + 2 = 2 + 1 := rfl\nend LiveCheck\n";
    fs::write(repo_dir.join("specs/Valid.lean"), valid_spec).unwrap();
    let valid_diff = added_lean_diff("specs/Valid.lean", valid_spec);

    let mut ok_child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .env("DC_VERIFY_LEAN_KERNEL", "require")
        .args([
            "check",
            "--planned",
            "specs/Valid.lean",
            "--root",
            repo_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dcverify valid");
    ok_child
        .stdin
        .take()
        .unwrap()
        .write_all(valid_diff.as_bytes())
        .unwrap();
    let ok_out = ok_child.wait_with_output().expect("wait dcverify valid");
    assert!(ok_out.status.success(), "dcverify valid failed: {ok_out:?}");
    let ok_json: serde_json::Value = serde_json::from_slice(&ok_out.stdout).unwrap();
    assert_eq!(
        ok_json["findings"].as_array().unwrap().len(),
        0,
        "real lean + lean4checker --fresh must accept sound spec: {ok_json}"
    );

    // Case B: Lexically clean false theorem (`1 = 2 := rfl`) must be convicted by the Lean kernel.
    let invalid_spec = "namespace LiveCheck\ntheorem unsound_refl : 1 = 2 := rfl\nend LiveCheck\n";
    fs::write(repo_dir.join("specs/Invalid.lean"), invalid_spec).unwrap();
    let invalid_diff = added_lean_diff("specs/Invalid.lean", invalid_spec);

    let mut bad_child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .env("DC_VERIFY_LEAN_KERNEL", "require")
        .args([
            "check",
            "--planned",
            "specs/Invalid.lean",
            "--root",
            repo_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dcverify invalid");
    bad_child
        .stdin
        .take()
        .unwrap()
        .write_all(invalid_diff.as_bytes())
        .unwrap();
    let bad_out = bad_child.wait_with_output().expect("wait dcverify invalid");
    assert!(
        bad_out.status.success(),
        "dcverify invalid failed: {bad_out:?}"
    );
    let bad_json: serde_json::Value = serde_json::from_slice(&bad_out.stdout).unwrap();
    assert!(
        bad_json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["gate"] == "lean_tcb" && f["severity"] == "blocking"),
        "real Lean kernel must convict lexically clean `1 = 2 := rfl`: {bad_json}"
    );

    // Case C: Unbuilt sound sibling `.lean` import must be staged, compiled, and accepted!
    let base_spec = "def baseStep (n : Nat) : Nat := n\n";
    let derived_spec = "import BaseMod\ntheorem derived_step (n : Nat) : baseStep n = n := rfl\n";
    fs::write(repo_dir.join("specs/BaseMod.lean"), base_spec).unwrap();
    fs::write(repo_dir.join("specs/DerivedMod.lean"), derived_spec).unwrap();
    let derived_diff = added_lean_diff("specs/DerivedMod.lean", derived_spec);
    let mut sib_child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .env("DC_VERIFY_LEAN_KERNEL", "require")
        .args([
            "check",
            "--planned",
            "specs/DerivedMod.lean",
            "--root",
            repo_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dcverify sibling");
    sib_child
        .stdin
        .take()
        .unwrap()
        .write_all(derived_diff.as_bytes())
        .unwrap();
    let sib_out = sib_child.wait_with_output().expect("wait dcverify sibling");
    assert!(sib_out.status.success(), "dcverify sibling failed: {sib_out:?}");
    let sib_json: serde_json::Value = serde_json::from_slice(&sib_out.stdout).unwrap();
    assert_eq!(
        sib_json["findings"].as_array().unwrap().len(),
        0,
        "unbuilt sound sibling Lean module import must pass kernel + lean4checker: {sib_json}"
    );

    // Case D: `private theorem` smuggling a custom `axiom` from an imported sibling module
    // must be convicted by the real Lean 4 Environment audit!
    fs::write(repo_dir.join("specs/SmuggleAx.lean"), "axiom hiddenBadAx : False\n").unwrap();
    let priv_smuggle = "import SmuggleAx\nprivate theorem secret_false : False := hiddenBadAx\ntheorem public_true : 1 = 1 := rfl\n";
    fs::write(repo_dir.join("specs/PrivateSmuggle.lean"), priv_smuggle).unwrap();
    let priv_diff = added_lean_diff("specs/PrivateSmuggle.lean", priv_smuggle);
    let mut priv_child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .env("DC_VERIFY_LEAN_KERNEL", "require")
        .args([
            "check",
            "--planned",
            "specs/PrivateSmuggle.lean",
            "--root",
            repo_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dcverify private smuggle");
    priv_child
        .stdin
        .take()
        .unwrap()
        .write_all(priv_diff.as_bytes())
        .unwrap();
    let priv_out = priv_child
        .wait_with_output()
        .expect("wait dcverify private smuggle");
    assert!(priv_out.status.success());
    let priv_json: serde_json::Value = serde_json::from_slice(&priv_out.stdout).unwrap();
    assert!(
        priv_json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["gate"] == "lean_tcb" && f["severity"] == "blocking"),
        "real Lean 4 Environment audit must convict private theorem custom-axiom smuggling: {priv_json}"
    );

    let _ = fs::remove_dir_all(&repo_dir);
}
