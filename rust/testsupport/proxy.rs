// A native program that decides what it is from `argv[0]`, for tests of the
// process seam. Included with
//
//     include!("../../testsupport/proxy.rs");
//
// from a test file (path relative to the including file under
// rust/<crate>/tests or rust/<crate>/src).
//
// A rustup proxy is the real case: `~/.cargo/bin/rust-analyzer` is a link to
// `rustup`, which reads its own `argv[0]` to know which tool to be. A shell
// script cannot stand in for it — the kernel hands the interpreter the script's
// path, so `$0` is the resolved file whatever name the parent started it under
// — so the stand-in is compiled with the `rustc` that is running this test
// suite.

/// Compile a proxy into `dir` that, started under the file name `expected`,
/// writes its own `argv[0]` to `marker` and exits 0, and under any other name
/// prints a usage line naming that name to stderr and exits 2 (rustup's
/// behaviour, in miniature). Either way it prints `argv[0]` to stdout first.
/// Returns the compiled program's path.
pub(crate) fn compile_argv0_proxy(
    dir: &std::path::Path,
    expected: &str,
    marker: &std::path::Path,
) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let source = dir.join("proxy.rs");
    let program = dir.join("proxy");
    std::fs::write(
        &source,
        format!(
            r#"fn main() {{
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let text = argv0.to_string_lossy().into_owned();
    println!("{{text}}");
    let name = std::path::Path::new(&argv0)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name == {expected:?} {{
        std::fs::write({marker:?}, text.as_bytes()).unwrap();
        std::process::exit(0);
    }}
    eprintln!("usage: started as {{name}}, which this proxy is not; nothing ran");
    std::process::exit(2);
}}
"#,
            marker = marker.display().to_string(),
        ),
    )
    .unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(rustc)
        .args(["--edition", "2021", "-o"])
        .arg(&program)
        .arg(&source)
        .output()
        .expect("rustc runs the test suite, so it is here");
    assert!(
        output.status.success(),
        "compiling the argv[0] proxy: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    program
}
