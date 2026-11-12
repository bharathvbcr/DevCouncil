//! End-to-end CLI tests for `devmap lean-tcb`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "devmap-lean-tcb-{label}-{}-{stamp}-{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create scratch dir");
        Self(root)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn devmap_lean_tcb_reports_sound_and_unsound_specs_and_enforces_strict_exit() {
    let scratch = ScratchDir::new("audit");
    let specs = scratch.0.join("specs");
    fs::create_dir_all(&specs).expect("mkdir specs");

    let clean_file = specs.join("Sound.lean");
    fs::write(
        &clean_file,
        "namespace Sound\ndef idNat (n : Nat) : Nat := n\ntheorem idNat_eq (n : Nat) : idNat n = n := rfl\nend Sound\n",
    )
    .expect("write Sound.lean");

    let bin = env!("CARGO_BIN_EXE_devmap");
    let root_str = scratch.0.to_str().unwrap();

    // 1. Sound repository passes `--strict --json` and explicitly discloses lexical tier.
    let ok_out = Command::new(bin)
        .args(["--root", root_str, "lean-tcb", "--strict", "--json"])
        .output()
        .expect("run devmap lean-tcb");
    assert!(
        ok_out.status.success(),
        "expected exit 0 on sound spec, stderr={}",
        String::from_utf8_lossy(&ok_out.stderr)
    );
    let ok_json: serde_json::Value =
        serde_json::from_slice(&ok_out.stdout).expect("parse sound json");
    assert_eq!(ok_json["sound"], true);
    assert_eq!(ok_json["verification_tier"], "lexical_only");
    assert_eq!(ok_json["walk_incomplete"], false);
    assert_eq!(ok_json["files_audited"], 1);
    assert_eq!(ok_json["unsound_files"], 0);
    assert_eq!(ok_json["files"][0]["verification_tier"], "lexical_only");

    // 2. Adding a spec with `sorry` and `@[extern]` fails `--strict` and surfaces both in JSON.
    let bad_file = specs.join("Unsound.lean");
    fs::write(
        &bad_file,
        "@[extern \"c_fn\"]\ndef cHook (n : Nat) : Nat := n\ntheorem fake : False := by sorry\n",
    )
    .expect("write Unsound.lean");

    let report_out = Command::new(bin)
        .args(["--root", root_str, "lean-tcb", "--json"])
        .output()
        .expect("run devmap lean-tcb report");
    assert!(report_out.status.success());
    let bad_json: serde_json::Value =
        serde_json::from_slice(&report_out.stdout).expect("parse unsound json");
    assert_eq!(bad_json["sound"], false);
    assert_eq!(bad_json["unsound_files"], 1);

    let strict_out = Command::new(bin)
        .args(["--root", root_str, "lean-tcb", "--strict", "--json"])
        .output()
        .expect("run devmap lean-tcb --strict");
    assert!(
        !strict_out.status.success(),
        "expected non-zero exit under --strict on unsound Lean TCB"
    );

    // 3. Symlinked `.lean` file marks `walk_incomplete = true` and `sound = false`.
    fs::remove_file(&bad_file).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&clean_file, specs.join("Alias.lean")).expect("symlink");
        let sym_out = Command::new(bin)
            .args(["--root", root_str, "lean-tcb", "--json"])
            .output()
            .expect("run devmap lean-tcb with symlink");
        let sym_json: serde_json::Value =
            serde_json::from_slice(&sym_out.stdout).expect("parse symlink json");
        assert_eq!(sym_json["walk_incomplete"], true);
        assert_eq!(sym_json["sound"], false);
    }
}

#[cfg(unix)]
#[test]
fn devmap_lean_tcb_kernel_and_lean4checker_tiers_fail_closed_on_missing_bad_axioms_and_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = ScratchDir::new("kernel-tiers");
    let specs = scratch.0.join("specs");
    fs::create_dir_all(&specs).expect("mkdir specs");
    fs::write(
        specs.join("Sound.lean"),
        "namespace Sound\ndef idNat (n : Nat) : Nat := n\ntheorem idNat_eq (n : Nat) : idNat n = n := rfl\nend Sound\n",
    )
    .expect("write Sound.lean");

    let empty_bin = ScratchDir::new("empty-bin");
    let mock_bin = ScratchDir::new("mock-bin");

    let devmap = env!("CARGO_BIN_EXE_devmap");
    let root_str = scratch.0.to_str().unwrap();

    // 1. Missing `lean` with `--kernel` stays honest (`lexical_only`), while `--require-kernel` fails closed!
    let auto_missing = Command::new(devmap)
        .env("PATH", &empty_bin.0)
        .args(["--root", root_str, "lean-tcb", "--kernel", "--json"])
        .output()
        .expect("run --kernel without lean");
    assert!(auto_missing.status.success());
    let auto_json: serde_json::Value = serde_json::from_slice(&auto_missing.stdout).unwrap();
    assert_eq!(auto_json["verification_tier"], "lexical_only");
    assert_eq!(auto_json["sound"], true);

    let req_missing = Command::new(devmap)
        .env("PATH", &empty_bin.0)
        .args(["--root", root_str, "lean-tcb", "--require-kernel", "--json"])
        .output()
        .expect("run --require-kernel without lean");
    assert!(
        !req_missing.status.success(),
        "--require-kernel must fail closed when lean is missing"
    );

    // 2. Install mock `lean` outside repo (emitting valid `.olean` and standard core axioms).
    // Without `lean4checker`: `--kernel` reports `"lean_kernel"`, while `--require-kernel` STILL fails closed!
    let write_exec = |path: &PathBuf, script: &str| {
        fs::write(path, script).expect("write mock script");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
    };
    let lean_path = mock_bin.0.join("lean");
    write_exec(
        &lean_path,
        "#!/bin/sh\nif [ \"$1\" = \"--print-libdir\" ]; then\n  echo /tmp\n  exit 0\nfi\nif [ \"$1\" = \"-o\" ]; then\n  printf 'olean' > \"$2\"\n  exit 0\nfi\ndriver=\"$1\"\nif [ \"${MOCK_LEAN_MODE:-ok}\" = \"sorry_ax\" ]; then\n  echo \"'Sound.idNat' does not depend on any axioms\"\n  echo \"'Sound.idNat_eq' depends on axioms: [propext, sorryAx]\"\n  exit 0\nfi\nif [ \"${MOCK_LEAN_MODE:-ok}\" = \"hang\" ]; then\n  (sleep 30) &\n  child=$!\n  if [ -n \"${MOCK_PID_FILE:-}\" ]; then echo \"$child\" > \"$MOCK_PID_FILE\"; fi\n  wait \"$child\"\n  exit 0\nfi\necho \"'Sound.idNat' does not depend on any axioms\"\necho \"'Sound.idNat_eq' depends on axioms: [propext,\n  Classical.choice,\n  Quot.sound]\"\nexit 0\n",
    );

    let sys_path = format!("{}:/bin:/usr/bin", mock_bin.0.display());
    let kernel_only = Command::new(devmap)
        .env("PATH", &sys_path)
        .args(["--root", root_str, "lean-tcb", "--kernel", "--json"])
        .output()
        .expect("run --kernel with lean only");
    assert!(
        kernel_only.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&kernel_only.stderr)
    );
    let kernel_only_json: serde_json::Value = serde_json::from_slice(&kernel_only.stdout).unwrap();
    assert_eq!(kernel_only_json["verification_tier"], "lean_kernel");
    assert_eq!(kernel_only_json["sound"], true);

    let req_no_checker = Command::new(devmap)
        .env("PATH", &sys_path)
        .args(["--root", root_str, "lean-tcb", "--require-kernel", "--json"])
        .output()
        .expect("run --require-kernel without lean4checker");
    assert!(
        !req_no_checker.status.success(),
        "--require-kernel must fail closed when lean4checker is missing"
    );

    // 3. Install mock `lean4checker`: both present & passing -> `"kernel_and_lean4checker"`, `sound == true`!
    let checker_path = mock_bin.0.join("lean4checker");
    write_exec(
        &checker_path,
        "#!/bin/sh\nif [ \"$1\" != \"--fresh\" ]; then\n  echo \"expected --fresh\" >&2\n  exit 2\nfi\nif [ \"${MOCK_CHECKER_FAIL:-0}\" = \"1\" ]; then\n  echo \"kernel replay rejected environment\" >&2\n  exit 1\nfi\nexit 0\n",
    );

    let both_ok = Command::new(devmap)
        .env("PATH", &sys_path)
        .args(["--root", root_str, "lean-tcb", "--require-kernel", "--json"])
        .output()
        .expect("run --require-kernel with lean + lean4checker");
    assert!(
        both_ok.status.success(),
        "expected exit 0, stderr={} stdout={}",
        String::from_utf8_lossy(&both_ok.stderr),
        String::from_utf8_lossy(&both_ok.stdout)
    );
    let both_json: serde_json::Value = serde_json::from_slice(&both_ok.stdout).unwrap();
    assert_eq!(both_json["verification_tier"], "kernel_and_lean4checker");
    assert_eq!(both_json["sound"], true);

    // 4. Non-standard axiom (`sorryAx`) in `#print axioms` output fails closed!
    let bad_ax = Command::new(devmap)
        .env("PATH", &sys_path)
        .env("MOCK_LEAN_MODE", "sorry_ax")
        .args(["--root", root_str, "lean-tcb", "--require-kernel", "--json"])
        .output()
        .expect("run --require-kernel with sorryAx");
    assert!(!bad_ax.status.success());
    let bad_ax_json: serde_json::Value = serde_json::from_slice(&bad_ax.stdout).unwrap();
    assert_eq!(bad_ax_json["sound"], false);

    // 5. `lean4checker --fresh` failure fails closed!
    let bad_chk = Command::new(devmap)
        .env("PATH", &sys_path)
        .env("MOCK_CHECKER_FAIL", "1")
        .args(["--root", root_str, "lean-tcb", "--require-kernel", "--json"])
        .output()
        .expect("run --require-kernel with failing lean4checker");
    assert!(!bad_chk.status.success());
    let bad_chk_json: serde_json::Value = serde_json::from_slice(&bad_chk.stdout).unwrap();
    assert_eq!(bad_chk_json["sound"], false);

    // 6. Hung `lean` child + grandchild (`sleep 30`) is killed by bounded timeout + process-group teardown!
    let pid_file = mock_bin.0.join("grandchild.pid");
    let hung = Command::new(devmap)
        .env("PATH", &sys_path)
        .env("MOCK_LEAN_MODE", "hang")
        .env("MOCK_PID_FILE", &pid_file)
        .env("DC_LEAN_KERNEL_TIMEOUT_MS", "250")
        .args(["--root", root_str, "lean-tcb", "--require-kernel", "--json"])
        .output()
        .expect("run --require-kernel with hung lean");
    assert!(!hung.status.success());
    if let Ok(pid_txt) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = pid_txt.trim().parse::<i32>() {
            std::thread::sleep(std::time::Duration::from_millis(100));
            // SAFETY: kill(pid, 0) only probes process existence without sending a signal.
            let alive = unsafe { libc::kill(pid, 0) == 0 };
            assert!(
                !alive,
                "grandchild pid {pid} survived lean kernel timeout process-group teardown"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn devmap_lean_tcb_skips_lakefile_on_walk_and_stages_unbuilt_sibling_imports() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = ScratchDir::new("sibling-imports");
    let specs = scratch.0.join("specs");
    fs::create_dir_all(&specs).expect("mkdir specs");

    // Root `lakefile.lean` build script uses Lake DSL metaprogramming/imports and is NOT a proof spec.
    fs::write(
        scratch.0.join("lakefile.lean"),
        "import Lake\nopen Lake DSL\npackage testpkg\n",
    )
    .expect("write lakefile.lean");

    fs::write(
        specs.join("Base.lean"),
        "def baseId (n : Nat) : Nat := n\n",
    )
    .expect("write Base.lean");
    fs::write(
        specs.join("Derived.lean"),
        "import Base\ntheorem derived_ok (n : Nat) : baseId n = n := rfl\n",
    )
    .expect("write Derived.lean");

    let mock_bin = ScratchDir::new("sibling-mock-bin");
    let write_exec = |path: &PathBuf, script: &str| {
        fs::write(path, script).expect("write mock script");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
    };
    // Mock `lean` refuses to compile any file containing `import Base` unless `Base.olean`
    // has ALREADY been compiled into the current scratch directory!
    write_exec(
        &mock_bin.0.join("lean"),
        "#!/bin/sh\nif [ \"$1\" = \"--print-libdir\" ]; then\n  echo /tmp\n  exit 0\nfi\nif [ \"$1\" = \"-o\" ]; then\n  out=\"$2\"\n  src=\"$3\"\n  if grep -q '^import Base' \"$src\"; then\n    if [ ! -f \"./Base.olean\" ]; then\n      echo \"missing precompiled ./Base.olean for import Base\" >&2\n      exit 1\n    fi\n  fi\n  printf 'olean' > \"$out\"\n  exit 0\nfi\ndriver=\"$1\"\ngrep '^#print axioms ' \"$driver\" | while read -r _ _ sym; do\n  clean=$(printf '%s' \"$sym\" | tr -d '«»')\n  echo \"'$clean' does not depend on any axioms\"\ndone\nexit 0\n",
    );
    write_exec(
        &mock_bin.0.join("lean4checker"),
        "#!/bin/sh\nif [ \"$1\" != \"--fresh\" ]; then exit 2; fi\nexit 0\n",
    );

    let devmap = env!("CARGO_BIN_EXE_devmap");
    let root_str = scratch.0.to_str().unwrap();
    let sys_path = format!("{}:/bin:/usr/bin", mock_bin.0.display());

    // 1. Directory walk skips `lakefile.lean` (`files_audited == 2`) and stages `Base.lean` -> `Base.olean`
    // before compiling `Derived.lean`, succeeding at `kernel_and_lean4checker` tier!
    let walk_out = Command::new(devmap)
        .env("PATH", &sys_path)
        .args(["--root", root_str, "lean-tcb", "--require-kernel", "--json"])
        .output()
        .expect("run devmap lean-tcb on multi-file unbuilt tree");
    assert!(
        walk_out.status.success(),
        "expected exit 0 on unbuilt sibling import tree, stderr={} stdout={}",
        String::from_utf8_lossy(&walk_out.stderr),
        String::from_utf8_lossy(&walk_out.stdout)
    );
    let walk_json: serde_json::Value = serde_json::from_slice(&walk_out.stdout).unwrap();
    assert_eq!(walk_json["sound"], true);
    assert_eq!(walk_json["files_audited"], 2);
    assert_eq!(walk_json["verification_tier"], "kernel_and_lean4checker");

    // 2. Explicitly targeting `lakefile.lean` still audits it (`files_audited == 1`).
    let explicit_out = Command::new(devmap)
        .args(["--root", root_str, "lean-tcb", "lakefile.lean", "--json"])
        .output()
        .expect("run devmap lean-tcb lakefile.lean");
    assert!(explicit_out.status.success());
    let explicit_json: serde_json::Value = serde_json::from_slice(&explicit_out.stdout).unwrap();
    assert_eq!(explicit_json["files_audited"], 1);
}
