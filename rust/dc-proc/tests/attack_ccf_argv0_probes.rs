//! Adversarial round on the `Resolved` arg0 seam (lane compiler-cheap-fixes,
//! 3adb8038): the lane's claim is that a program `resolve_outside` found can
//! only be started through `Resolved::command`, under the name it was looked
//! up by.
//!
//! `git_with_program` resolves `git`, then probes it (`git version`,
//! `git config --get-regexp`) before building the command. A `git` that is a
//! link to a multiplexer that reads `argv[0]` (mise and asdf-rs shims are
//! exactly this shape, as is rustup's `rust-analyzer`) must be probed under the
//! same name it will be run under, or the probe asks the multiplexer instead of
//! git and the read is refused before it starts.
#![cfg(unix)]
#![allow(
    unsafe_code,
    reason = "the one test sets PATH before any other thread exists"
)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

fn compile_git_proxy(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let source = dir.join("proxy.rs");
    let program = dir.join("proxy");
    std::fs::write(
        &source,
        r#"fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let name = std::path::Path::new(&args[0])
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name != "git" {
        eprintln!("usage: started as {name}, which is not git; nothing ran");
        std::process::exit(2);
    }
    let rest: Vec<String> = args[1..].iter().map(|a| a.to_string_lossy().into_owned()).collect();
    if rest.first().map(String::as_str) == Some("version") {
        println!("git version 2.50.0");
        std::process::exit(0);
    }
    if rest.iter().any(|a| a == "config") {
        std::process::exit(1);
    }
    println!("RAN as git");
}
"#,
    )
    .unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(rustc)
        .args(["--edition", "2021", "-o"])
        .arg(&program)
        .arg(&source)
        .output()
        .expect("rustc runs");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    program
}

#[test]
fn a_git_reached_through_a_multiplexer_link_is_probed_under_the_name_it_runs_as() {
    let base = std::env::temp_dir().join(format!("attack-ccf-argv0-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let proxy = compile_git_proxy(&base.join("real"));
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(&proxy, bin.join("git")).unwrap();
    // SAFETY: this target holds one test; no other thread reads the environment.
    unsafe {
        std::env::set_var("PATH", &bin);
    }

    for args in [&["rev-parse", "HEAD"][..], &["status"][..]] {
        let command = dc_proc::git_with_program(OsStr::new("git"), &root, args);
        let mut command = match command {
            Ok(command) => command,
            Err(failure) => panic!(
                "git {args:?}: refused before it ran, because the probe started the link's \
                 target under its own name: {failure}"
            ),
        };
        let output = command.output().expect("the proxy starts");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}
