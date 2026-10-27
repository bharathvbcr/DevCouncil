//! `dcverify` — the analysis plane's verifier, over a JSON-on-stdio boundary.
//!
//! Same contract as `dcstore`, for the same reason: the Go execution plane
//! crosses to Rust by process rather than by cgo, so `CGO_ENABLED=0`, simple
//! cross-compilation, and the single static binary all survive.
//!
//! It reads a unified diff on stdin and prints one JSON object on stdout.
//! Crucially, a diff it cannot parse is exit 2 with an error, never an empty
//! result: an empty finding list means "these gates ran and found nothing", and
//! a caller cannot be allowed to read "could not run" as that.

use std::process::ExitCode;

use dc_verify::bounded::{MAX_CHECK_BYTES, decode_lossy, read_bounded};
use dc_verify::rigor::{
    Finding, Severity, Strength, detect_stubs_report, intersect_coverage, scan_secrets,
};
use dc_verify::{classify_scope, normalize_candidate, parse_unified};

#[path = "../evidence_cli.rs"]
mod evidence_cli;

// A disconnected consumer is a transport failure, not a Rust panic. Keep
// diagnostics off this JSON protocol, including the error response path.
macro_rules! respond {
    ($($argument:tt)*) => {{
        use std::io::Write as _;
        let mut out = std::io::stdout().lock();
        if out.write_fmt(format_args!("{}\n", format_args!($($argument)*)))
            .and_then(|()| out.flush()).is_err() {
            return ExitCode::FAILURE;
        }
    }};
}

/// Replaces the default panic hook, which prints the panic message.
///
/// A message can carry data from the diff: a string-slicing panic prints up to
/// 256 bytes of the string it sliced, and the Go host copies this process's
/// stderr into its errors. So the hook prints where the panic happened and
/// nothing it carried. Writing is best effort: a closed stderr must not turn a
/// panic into a second one.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write as _;
        let place = match info.location() {
            Some(at) => format!("{}:{}", at.file(), at.line()),
            None => "an unknown location".to_string(),
        };
        let _ = writeln!(std::io::stderr(), "dcverify panicked at {place}");
    }));
}

/// Debug builds only: panics with the value of `DCVERIFY_TEST_PANIC_WITH` in
/// the message, so a test can plant a secret in a panic and look for it on
/// stderr. Compiled out of release builds, which have no such surface.
#[cfg(debug_assertions)]
fn test_panic() {
    if let Ok(planted) = std::env::var("DCVERIFY_TEST_PANIC_WITH") {
        panic!("planted payload: {planted}");
    }
}

#[cfg(not(debug_assertions))]
fn test_panic() {}

fn main() -> ExitCode {
    install_panic_hook();
    test_panic();
    let args = match collect_args() {
        Ok(args) => args,
        Err(message) => {
            respond!("{{\"ok\":false,\"error\":{}}}", quote(&message));
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(json) => {
            respond!("{json}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            respond!("{{\"ok\":false,\"error\":{}}}", quote(&message));
            ExitCode::from(2)
        }
    }
}

/// Collects the command line, refusing an argument that is not valid Unicode
/// rather than dying on it.
///
/// `std::env::args()` panics on such an argument: exit 101, a backtrace on
/// stderr, and nothing on stdout, which the Go client can only report as a
/// verifier that failed with no reason. A rigor check that could not run must
/// not be indistinguishable from one that ran — and the paths this binary is
/// handed come from a diff and from a working tree, where Unix filenames are
/// bytes rather than Unicode.
fn collect_args() -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    for (index, arg) in std::env::args_os().skip(1).enumerate() {
        match arg.into_string() {
            Ok(value) => args.push(value),
            Err(_) => return Err(format!("argument {index} is not valid UTF-8")),
        }
    }
    Ok(args)
}

const KNOWN_FLAGS: &[&str] = &["planned", "coverage", "root"];
const IDENTITY: &str = "dc-verify";
const COMPONENT_ID: &str = "dcverify";
const SCHEMA_VERSION: u32 = 1;

fn is_version_arg(arg: &str) -> bool {
    matches!(arg, "--version" | "-V" | "-v" | "version")
}

/// What `--version` answers.
///
/// `CARGO_PKG_VERSION` and never a literal: every crate in this workspace takes
/// `version.workspace = true`, so the one number in `rust/Cargo.toml` reaches
/// here and a release has no second place to forget. Until this existed the
/// binary answered `--version` with `unknown flag --version`, so a deployed
/// verifier's version was simply not observable — the package said 0.2.3 and
/// the program could not be asked.
///
/// JSON, like every other outcome on this boundary: a caller that parses each
/// line must not have to special-case this one.
fn version_object() -> String {
    format!(
        "{{\"ok\":true,\"id\":{},\"component\":{},\"version\":{}}}",
        quote(COMPONENT_ID),
        quote(IDENTITY),
        quote(env!("CARGO_PKG_VERSION"))
    )
}

fn run(args: &[String]) -> Result<String, String> {
    if args.iter().any(|arg| is_version_arg(arg)) {
        return Ok(version_object());
    }
    if args.first().is_some_and(|arg| arg == "evidence-check") {
        return evidence_cli::run(&args[1..]);
    }
    let mut planned: Vec<String> = Vec::new();
    let mut coverage_path: Option<String> = None;
    let mut root: Option<String> = None;
    let mut positional: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(name) = arg.strip_prefix("--") {
            if !KNOWN_FLAGS.contains(&name) {
                return Err(format!(
                    "unknown flag --{name} (known: {})",
                    KNOWN_FLAGS.join(", ")
                ));
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("flag --{name} needs a value"))?;
            // Planned paths arrive newline-separated so a path containing a
            // comma or a space survives the boundary intact.
            if name == "coverage" {
                coverage_path = Some(value.clone());
            } else if name == "root" {
                // Absolute LCOV paths reduce against this; without it the
                // working directory is used, which a host driving the binary
                // from elsewhere cannot rely on.
                root = Some(value.clone());
            } else {
                planned = value
                    .lines()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            i += 2;
        } else {
            positional.push(arg);
            i += 1;
        }
    }

    match positional.first().copied() {
        Some("health") => Ok(format!(
            "{{\"ok\":true,\"id\":{},\"component\":{},\"version\":{},\"verifier\":{},\"schema_version\":{},\"evidence_schema_versions\":[1]}}",
            quote(COMPONENT_ID),
            quote(IDENTITY),
            quote(env!("CARGO_PKG_VERSION")),
            quote(IDENTITY),
            SCHEMA_VERSION
        )),
        Some("check") | None => check(
            &planned,
            coverage_path.as_deref(),
            root.as_deref().map(std::path::Path::new),
        ),
        Some(other) => Err(format!(
            "unknown command {other:?} (check, health, evidence-check, --version, -V, -v, version)"
        )),
    }
}

fn check(
    planned: &[String],
    coverage_path: Option<&str>,
    root: Option<&std::path::Path>,
) -> Result<String, String> {
    // Read as bytes and decode lossily, rather than requiring UTF-8.
    //
    // `read_to_string` refuses a stream that is not valid UTF-8, and git will
    // hand this one raw bytes whenever a changed file is text by git's rule —
    // no NUL in the first 8000 bytes — but not Unicode, which is every latin-1
    // source file and every file carrying one stray byte. The refusal
    // propagated as "the rigor gates did not run", the caller recorded that as
    // a degradation, and a degradation produces no blocking finding: the
    // secret scanner, the stub detector and the coverage gate were all skipped
    // for the whole change while the report still said passed.
    //
    // That made a single byte a way to switch the credential scanner off, which
    // is the opposite of what a scanner is for. Decoding lossily keeps the
    // check running. Nothing it looks for is lost in the substitution: the
    // secrets it matches are ASCII, and U+FFFD replaces the offending bytes
    // in place without adding or removing a line, so both the patterns and the
    // line numbers survive.
    //
    // The read stops at MAX_CHECK_BYTES (32 MiB, the host's git diff cap).
    // One byte over is `exceeds N bytes` and is not parsed. Valid UTF-8 is
    // borrowed; only a lossy decode allocates a second copy.
    let raw = read_bounded(std::io::stdin(), "the diff")?;
    let diff = decode_lossy(&raw);

    let files = parse_unified(&diff).map_err(|e| {
        format!(
            // Never the line: it is diff content, and a diff can hold a
            // credential. The number and the reason are what is actionable.
            "the diff could not be parsed at line {}: {}",
            e.line_number, e.reason
        )
    })?;

    let scope = classify_scope(&files, planned);
    let mut findings = scan_secrets(&files);
    let stubs = detect_stubs_report(&files, &|file| read_post_image(root, &file.path));
    findings.extend(stubs.findings);
    // A binary marker with no hunk carries none of the file's bytes. A text
    // hunk elsewhere in the same diff does not make that path scanned.
    let mut binary_degraded = Vec::new();
    scan_binary_paths(
        &diff,
        &files,
        &scope.in_scope,
        root,
        &mut findings,
        &mut binary_degraded,
    );

    // Coverage measurements, when the caller supplied them. A missing file is
    // fatal rather than silently treated as "no coverage": a caller that asked
    // for coverage and got "everything unmeasured" because of a typo'd path
    // would read the same report as one whose tests genuinely ran nothing.
    //
    // Read as bytes under MAX_CHECK_BYTES and decoded lossily. A non-UTF-8
    // `SF:` path must not drop findings already computed; the path that no
    // longer matches lands in coverage_unmeasured. Past the cap is an error,
    // and that error is named before the file is parsed — a parse error would
    // quote the body.
    let measurements = match coverage_path {
        None => Vec::new(),
        Some(path) => {
            let raw = read_capped_file(path)?;
            let text = decode_lossy(&raw);
            dc_verify::coverage::parse_with_root(&text, root)
                .map_err(|e| format!("parsing the coverage file {path}: {e}"))?
        }
    };
    let coverage = intersect_coverage(&files, &measurements);
    // Unconditional, and never skipped: the measurement needs nothing but the
    // diff every other gate already read, so there is no state in which it
    // could not run and therefore no `substance_skipped_reason` to invent.
    let substance = dc_verify::substance::measure(&files);

    Ok(format!(
        "{{\"ok\":true,\"files\":{},\"stub_ast_parsed_files\":{},\"in_scope\":{},\"orphans\":{},\
         \"untouched_planned\":{},\"findings\":{},\"coverage_unmeasured\":{},\"coverage_gaps\":{},\
         \"coverage_skipped_by_type\":{},\"substance\":{},\"stub_ast_degraded\":{}}}",
        // Changed paths, not diff entries: a rename is two, and scope
        // classification judged both.
        dc_verify::changed_paths(&files).count(),
        // The skipped- and assert-free-test checks run only on parsed files;
        // the host names them as applied only when this is above zero.
        stubs.parsed_files,
        string_array(&scope.in_scope),
        string_array(&scope.orphans),
        string_array(&scope.untouched_planned),
        findings_array(&findings),
        string_array(&coverage.unmeasured),
        coverage_gaps(&coverage),
        // Reported even though nothing blocks on it. A file the gate did not
        // ask about used to leave the reply with no trace, which reads exactly
        // like a file that was measured and clean.
        string_array(&coverage.skipped_by_type),
        substance_object(&substance),
        // Files the tree could not analyse, which took the weaker substring
        // checks. Additive: a host that does not read it sees the reply it
        // always did.
        string_array(&{
            let mut degraded: Vec<String> = stubs
                .unparsed
                .iter()
                .map(|u| format!("{}: {}", u.path, u.reason))
                .collect();
            degraded.extend(binary_degraded);
            degraded
        }),
    ))
}

/// Coverage file, bounded by [`MAX_CHECK_BYTES`]. The length of a regular
/// file is taken from symlink metadata before any read, so a sparse file past
/// the cap is not allocated. A symlink is refused and not opened: following
/// one would report the target's measurements as if they were this path.
/// A character device or fifo has no trustworthy length, so the bounded read
/// is the cap.
fn read_capped_file(path: &str) -> Result<Vec<u8>, String> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("reading the coverage file {path}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("refusing to follow a symlink: {path}"));
    }
    let kind = meta.file_type();
    if kind.is_file() {
        if meta.len() > MAX_CHECK_BYTES as u64 {
            return Err(format!("the coverage file exceeds {MAX_CHECK_BYTES} bytes"));
        }
    } else if !unbounded_device(&kind) {
        return Err(format!("the coverage file {path} is not a regular file"));
    }
    let file =
        std::fs::File::open(path).map_err(|e| format!("reading the coverage file {path}: {e}"))?;
    let again = std::fs::symlink_metadata(path)
        .map_err(|e| format!("reading the coverage file {path}: {e}"))?;
    if again.file_type().is_symlink() {
        return Err(format!("refusing to follow a symlink: {path}"));
    }
    read_bounded(file, "the coverage file")
}

/// A character device or fifo has no trustworthy length, so the bounded read
/// is the cap. Other targets have no such files in this check.
fn unbounded_device(kind: &std::fs::FileType) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        kind.is_char_device() || kind.is_fifo()
    }
    #[cfg(not(unix))]
    {
        let _ = kind;
        false
    }
}

/// What `dcverify` writes between a path and its reason when the secret scan
/// could not read a binary file's bytes.
///
/// `stub_ast_degraded` is the one degradation list on the wire, and the JSON
/// contract is public, so this kind of entry has no key of its own: the kind is
/// in the text, and the Go host (`dcverify.BinaryScanSkippedMarker`) reads it
/// from there. An entry that said only "path: reason" read as a stub-detection
/// fallback, and an operator could not tell a credential scan had skipped a
/// file.
const BINARY_SCAN_SKIPPED: &str = ": secret scan skipped (";

fn binary_scan_skipped(path: &str, reason: &str) -> String {
    format!("{path}{BINARY_SCAN_SKIPPED}binary file not read: {reason})")
}

/// Planned paths whose diff section is a binary marker and no hunk.
///
/// Their bytes are not in the diff. Read the working-tree file under the
/// same cap, without following a symlink, and secret-scan it. A path that
/// cannot be read degrades the verdict instead of scanning clean. A path the
/// diff deletes has no working-tree file, so there is nothing to read and it
/// is not asked for.
///
/// A file in a language the stub checks parse is also stub-scanned, from the
/// bytes just read. git marks a source file binary when it holds a NUL byte or
/// when `.gitattributes` says `-diff`, and then prints no hunk, so the stub
/// scan the diff feeds would otherwise never see it. The file's lines are all
/// treated as added: the diff does not say which changed. A file the tree
/// could not analyse, or too large for it, is reported degraded like any other.
fn scan_binary_paths(
    diff: &str,
    files: &[dc_verify::FileDiff],
    in_scope: &[String],
    root: Option<&std::path::Path>,
    findings: &mut Vec<Finding>,
    degraded: &mut Vec<String>,
) {
    for section in binary_sections_without_hunks(diff) {
        // The diff parser makes one entry per `diff --git` line, in order, and
        // has already named the file (rename and copy lines included), so the
        // path comes from there and not from re-reading the header.
        let Some(parsed) = files.get(section) else {
            degraded.push(binary_scan_skipped(
                "a binary section",
                "the diff parser did not name it",
            ));
            continue;
        };
        let normalized = normalize_candidate(&parsed.path);
        if !in_scope.iter().any(|p| p == &normalized) {
            continue;
        }
        if !parsed.added_lines.is_empty() {
            continue;
        }
        match read_binary_post_image(root, &normalized) {
            Ok(text) => {
                scan_binary_text(&normalized, &text, findings, degraded);
            }
            Err(reason) => degraded.push(binary_scan_skipped(&normalized, &reason)),
        }
    }
}

/// Secret-scans, and for a source file stub-scans, the bytes of one binary-marked path.
fn scan_binary_text(
    path: &str,
    text: &str,
    findings: &mut Vec<Finding>,
    degraded: &mut Vec<String>,
) {
    let mut line_no = 0u32;
    for line in text.split_inclusive('\n') {
        line_no = line_no.saturating_add(1);
        let line = line.trim_end_matches(['\n', '\r']);
        if let Some(secret) = dc_redact::find_secret(line) {
            findings.push(Finding {
                gate: "secret_scan",
                severity: Severity::Blocking,
                strength: Strength::Derived,
                path: path.to_string(),
                line: line_no.max(1),
                evidence: secret.redacted,
                message: format!(
                    "added line contains what looks like a {}; a credential in a commit is \
                     compromised even after it is deleted, because the object stays in history",
                    secret.name
                ),
            });
        }
    }
    if dc_verify::stub_ast::Lang::for_path(path).is_none() {
        return;
    }
    if text.len() > dc_verify::stub_ast::MAX_SOURCE_BYTES {
        degraded.push(format!(
            "{path}: {}",
            dc_verify::stub_ast::Unparsed::TooLarge.as_str()
        ));
        return;
    }
    let whole = dc_verify::FileDiff {
        path: path.to_string(),
        old_path: None,
        status: dc_verify::ChangeStatus::Added,
        added_lines: text
            .lines()
            .enumerate()
            .map(|(i, line)| (u32::try_from(i + 1).unwrap_or(u32::MAX), line.to_string()))
            .collect(),
        removed_lines: Vec::new(),
    };
    let report = detect_stubs_report(&[whole], &|_| None);
    findings.extend(report.findings);
    degraded.extend(
        report
            .unparsed
            .iter()
            .map(|u| format!("{}: {}", u.path, u.reason)),
    );
}

/// Indexes, among a diff's `diff --git` sections, of those that hold a binary
/// marker and no hunk and do not delete the file.
fn binary_sections_without_hunks(diff: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut index: Option<usize> = None;
    let mut saw_hunk = false;
    let mut saw_binary = false;
    let mut saw_deleted = false;
    let flush = |index: Option<usize>,
                 saw_hunk: bool,
                 saw_binary: bool,
                 saw_deleted: bool,
                 out: &mut Vec<usize>| {
        if let Some(i) = index
            && saw_binary
            && !saw_hunk
            && !saw_deleted
        {
            out.push(i);
        }
    };
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            flush(index, saw_hunk, saw_binary, saw_deleted, &mut out);
            index = Some(index.map_or(0, |i| i + 1));
            saw_hunk = false;
            saw_binary = false;
            saw_deleted = false;
            continue;
        }
        if index.is_none() {
            continue;
        }
        if line.starts_with("@@") {
            saw_hunk = true;
        }
        // Before the binary marker only: a `GIT binary patch` body is base85
        // and cannot spell these words, but the order is what makes it certain.
        if !saw_binary && line.starts_with("deleted file mode") {
            saw_deleted = true;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            saw_binary = true;
            if line.ends_with(" and /dev/null differ") {
                saw_deleted = true;
            }
        }
    }
    flush(index, saw_hunk, saw_binary, saw_deleted, &mut out);
    out
}

/// Metadata of `rel` under `root`, reached without following a symlink at any
/// component: every prefix is `lstat`ed, a symlink anywhere on the way is
/// refused, and every parent must be a directory. Checking only the last
/// component let `d -> /elsewhere` carry a read of `d/f` out of the repository.
///
/// `root` itself is the caller's choice and is not examined. Only a relative
/// path of plain names (and `.`) is accepted.
///
/// This narrows the window, it does not close it: the open that follows is by
/// path, and a descriptor-relative walk (`openat` with `O_NOFOLLOW` per
/// component) needs a dependency this crate does not carry. Callers repeat the
/// walk after reading, so a path swapped for a link during the read is refused
/// rather than believed.
fn lstat_within(
    root: &std::path::Path,
    rel: &std::path::Path,
) -> Result<(std::path::PathBuf, std::fs::Metadata), String> {
    use std::path::Component;
    let mut current = root.to_path_buf();
    let mut last: Option<std::fs::Metadata> = None;
    let mut components = rel.components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::CurDir => continue,
            Component::Normal(part) => current.push(part),
            _ => return Err("path escapes the working tree".to_string()),
        }
        let meta = std::fs::symlink_metadata(&current).map_err(|e| format!("cannot read: {e}"))?;
        if meta.file_type().is_symlink() {
            return Err("refusing to follow a symlink".to_string());
        }
        if components.peek().is_some() && !meta.is_dir() {
            return Err("a parent of the path is not a directory".to_string());
        }
        last = Some(meta);
    }
    last.map(|meta| (current, meta))
        .ok_or_else(|| "path escapes the working tree".to_string())
}

/// Working-tree bytes of a binary path, or why they could not be scanned.
///
/// A symlink, at the last component or any parent, is refused and not opened.
/// The read is bounded by [`MAX_CHECK_BYTES`] before the file is pulled in:
/// metadata length first, then `take`. The result is lossy-decoded so a NUL
/// does not hide a credential that sits in the same file.
fn read_binary_post_image(root: Option<&std::path::Path>, path: &str) -> Result<String, String> {
    use std::io::Read as _;
    let root = root.ok_or_else(|| "no working tree root".to_string())?;
    if path.is_empty() {
        return Err("path escapes the working tree".to_string());
    }
    let rel = std::path::Path::new(path);
    let (full, meta) = lstat_within(root, rel)?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_CHECK_BYTES as u64 {
        return Err(format!("exceeds {MAX_CHECK_BYTES} bytes"));
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&full)
        .map_err(|e| format!("cannot read: {e}"))?
        .take(MAX_CHECK_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read: {e}"))?;
    if read > MAX_CHECK_BYTES {
        return Err(format!("exceeds {MAX_CHECK_BYTES} bytes"));
    }
    // The walk above is the open we refused to follow a link through. Repeat it
    // before believing the bytes, in case a component was replaced with a link.
    let (_, again) = lstat_within(root, rel)?;
    if !again.is_file() {
        return Err("refusing to follow a symlink".to_string());
    }
    Ok(decode_lossy(&bytes).into_owned())
}

/// Reads a changed file as it stands in the working tree under `root`, for
/// the AST stub checks.
///
/// `None` sends the file to the substring checks; it is never a clean answer.
/// The path comes from a diff, so it is refused unless it is relative, stays
/// inside `root` and passes through no symlink, and the read is bounded.
/// `detect_stubs_with` then believes the content only if the diff's added lines
/// are where the diff says they are.
fn read_post_image(root: Option<&std::path::Path>, path: &str) -> Option<String> {
    use std::io::Read as _;
    let root = root?;
    let (full, meta) = lstat_within(root, std::path::Path::new(path)).ok()?;
    if !meta.is_file() || meta.len() > dc_verify::stub_ast::MAX_SOURCE_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&full)
        .ok()?
        .take(dc_verify::stub_ast::MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > dc_verify::stub_ast::MAX_SOURCE_BYTES {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Renders the substance measurement.
///
/// `judged` is on the wire beside the counts rather than left for the caller to
/// re-derive from `added_lines`. The threshold that decides it is this crate's
/// to own — a second copy in the Go host would be a second policy, and the two
/// would answer differently the first time either moved.
///
/// No ratio: see `SubstanceReport`. The five class counts sum to `added_lines`,
/// which is what lets a reader check the arithmetic rather than trust it.
fn substance_object(report: &dc_verify::substance::SubstanceReport) -> String {
    let files: Vec<String> = report
        .files
        .iter()
        .map(|f| {
            format!(
                "{{\"path\":{},\"added_lines\":{},\"substantive_lines\":{}}}",
                quote(&f.path),
                f.added_lines,
                f.substantive_lines
            )
        })
        .collect();
    format!(
        "{{\"added_lines\":{},\"substantive_lines\":{},\"trivial\":{},\"moved\":{},\
         \"repeated\":{},\"generated\":{},\"judged\":{},\"low\":{},\"files\":[{}]}}",
        report.added_lines,
        report.substantive_lines,
        report.trivial,
        report.moved,
        report.repeated,
        report.generated,
        report.judged(),
        report.is_low(),
        files.join(","),
    )
}

/// Renders findings, each carrying both axes: how much it matters and how the
/// gate knows.
///
/// `strength` is additive, so `schema_version` stays 1 — the same rule dcgrep's
/// `index` object followed. A client that does not read the field sees exactly
/// what it saw before; one that does can stop treating a prefix match and an
/// executed profile as the same kind of evidence.
fn findings_array(findings: &[Finding]) -> String {
    let items: Vec<String> = findings
        .iter()
        .map(|f| {
            format!(
                "{{\"gate\":{},\"severity\":{},\"strength\":{},\"path\":{},\"line\":{},\
                 \"evidence\":{},\"message\":{}}}",
                quote(f.gate),
                quote(match f.severity {
                    Severity::Blocking => "blocking",
                    Severity::Advisory => "advisory",
                }),
                quote(f.strength.as_str()),
                quote(&f.path),
                f.line,
                quote(&f.evidence),
                quote(&f.message),
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn coverage_gaps(report: &dc_verify::rigor::CoverageReport) -> String {
    let items: Vec<String> = report
        .gaps
        .iter()
        .map(|g| {
            let lines: Vec<String> = g.uncovered_lines.iter().map(|n| n.to_string()).collect();
            format!(
                "{{\"path\":{},\"added_lines\":{},\"uncovered_lines\":[{}]}}",
                quote(&g.path),
                g.added_lines,
                lines.join(",")
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn string_array(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|v| quote(v)).collect();
    format!("[{}]", items.join(","))
}

/// Hand-rolled JSON string escaping, matching `dcstore`'s. The dependency-free
/// stance is deliberate: these binaries ship inside the harness, and a
/// serialisation crate is a supply-chain surface for output this simple.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
