//! Coverage ingestion.
//!
//! The verifier's diff↔coverage gate needs to know which lines a test suite
//! actually executed. Nothing produces that in a single universal format, so
//! this reads the two that matter here — Go's `-coverprofile` and LCOV, which
//! covers Rust via grcov/llvm-cov as well as most other ecosystems — and
//! normalises both into the line sets the intersection consumes.
//!
//! One rule governs the parsing, and it is the same one the rest of the
//! verifier follows: **a file that could not be parsed is an error, never an
//! empty coverage set.** An empty set means "this file was measured and nothing
//! ran", which is the strongest possible finding; producing it from a malformed
//! input would turn a broken pipeline into a wall of false gaps, and the
//! opposite mistake — treating a parse failure as "no data, skip" — would make
//! a coverage gate that silently checks nothing.

use std::collections::BTreeMap;

use crate::rigor::FileCoverage;

/// The widest covered span one profile block may declare. Real blocks are
/// functions; anything past this is a corrupt or hostile profile line, refused
/// rather than materialised.
const MAX_COVERED_SPAN: u64 = 1_000_000;

/// The most line numbers one report may materialise, across every block of
/// every file.
///
/// [`MAX_COVERED_SPAN`] bounds one block and nothing bounded their sum, which
/// is not the same guarantee: a profile of N blocks each just inside the
/// per-block limit costs N times that limit. Measured on overlapping
/// 1,000,000-line blocks for a single file, 601 bytes of input took 19.7 s and
/// 85 MB, and 12.3 KB took 284 s and 1.6 GB — roughly 130 KB of memory per byte
/// of input. That is reachable rather than theoretical, because the coverage
/// path is one the executor generally controls (`--coverage` points into the
/// repo), and its consequence was worse than the memory: the Go client bounds
/// the verifier at 30 s, so a crafted profile reliably pushed `secret_scan`,
/// `stub_detection` and `diff_coverage` into "did not run" — which is recorded
/// as degradation, not as a blocking finding. A file nobody could parse must
/// not be a way to switch three gates off quietly.
///
/// Four million is far past any real measurement — it is a codebase of four
/// million executed lines — and bounds this parse to about 16 MB and a few
/// milliseconds. Exceeding it is an error, following the same rule as every
/// other malformed input here: an error, never a measurement of nothing.
const MAX_COVERED_LINES: u64 = 4_000_000;

/// A coverage file that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageError {
    pub line_number: usize,
    pub line: String,
    pub reason: &'static str,
}

impl std::fmt::Display for CoverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}: {} ({:?})",
            self.line_number, self.reason, self.line
        )
    }
}

impl std::error::Error for CoverageError {}

/// Parses a coverage report, detecting the format from its content.
pub fn parse(raw: &str) -> Result<Vec<FileCoverage>, CoverageError> {
    parse_with_root(raw, None)
}

/// Like [`parse`], but with the repository root supplied by the caller — the
/// binary passes it so absolute LCOV paths (llvm-cov/grcov emit those) reduce
/// to the repo-relative form the diff uses.
pub fn parse_with_root(
    raw: &str,
    root: Option<&std::path::Path>,
) -> Result<Vec<FileCoverage>, CoverageError> {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() {
        // An empty file is not an empty measurement. It means the run produced
        // nothing, which is a broken pipeline rather than a finding about the
        // code, and reporting every added line as uncovered would bury that.
        return Err(CoverageError {
            line_number: 0,
            line: String::new(),
            reason: "the coverage file is empty; no measurement was recorded",
        });
    }
    if trimmed.starts_with("mode:") {
        parse_go(raw, root)
    } else if trimmed.starts_with("TN:") || trimmed.starts_with("SF:") {
        parse_lcov(raw, root)
    } else {
        Err(CoverageError {
            line_number: 1,
            line: trimmed.lines().next().unwrap_or_default().to_string(),
            reason: "unrecognised coverage format (expected a Go coverprofile starting `mode:` or LCOV starting `TN:`/`SF:`)",
        })
    }
}

/// Parses `go test -coverprofile` output.
///
/// Each line after the header is `name.go:startLine.startCol,endLine.endCol
/// numStmts count`. A block with a non-zero count means every line it spans
/// executed; the intersection asks about individual lines, so the block is
/// expanded.
fn parse_go(raw: &str, root: Option<&std::path::Path>) -> Result<Vec<FileCoverage>, CoverageError> {
    let modules = root.map(go_modules).unwrap_or_default();
    let mut covered: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut seen_any = false;
    let mut materialised: u64 = 0;

    for (idx, line) in raw.lines().enumerate() {
        let lineno = idx + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with("mode:") {
            continue;
        }
        let bad = |reason: &'static str| CoverageError {
            line_number: lineno,
            line: line.to_string(),
            reason,
        };

        let (location, counts) = line
            .rsplit_once(' ')
            .ok_or_else(|| bad("expected `location numStmts count`"))?;
        let count: u64 = counts
            .parse()
            .map_err(|_| bad("execution count is not a number"))?;
        let (location, _stmts) = location
            .rsplit_once(' ')
            .ok_or_else(|| bad("expected a statement count before the execution count"))?;
        let (path, span) = location
            .split_once(':')
            .ok_or_else(|| bad("expected `path:span`"))?;
        let (start, end) = span
            .split_once(',')
            .ok_or_else(|| bad("expected `start,end` in the span"))?;

        let start_line: u32 = start
            .split('.')
            .next()
            .unwrap_or_default()
            .parse()
            .map_err(|_| bad("start line is not a number"))?;
        let end_line: u32 = end
            .split('.')
            .next()
            .unwrap_or_default()
            .parse()
            .map_err(|_| bad("end line is not a number"))?;
        if end_line < start_line {
            return Err(bad("the block ends before it starts"));
        }
        // One profile line naming a span of billions of lines used to be
        // materialised line by line — a corrupt or hostile profile could
        // allocate gigabytes before the intersection ever ran. A block wider
        // than any real source file is malformed input, and malformed input is
        // an error rather than a measurement of nothing.
        let span = u64::from(end_line) - u64::from(start_line) + 1;
        if span > MAX_COVERED_SPAN {
            return Err(bad("the block spans more lines than any real source file"));
        }

        seen_any = true;
        // The path is module-qualified; the diff is repo-relative.
        let entry = covered
            .entry(normalise_go_path(path, &modules))
            .or_default();
        if count > 0 {
            // Counted before the lines are pushed, not after: the point of the
            // bound is that the allocation never happens, so checking it once
            // the vector had already grown would be a report of the problem
            // rather than a defence against it.
            materialised = materialised.saturating_add(span);
            if materialised > MAX_COVERED_LINES {
                return Err(bad(
                    "the profile's blocks together cover more lines than any real measurement",
                ));
            }
            for n in start_line..=end_line {
                entry.push(n);
            }
        }
    }

    if !seen_any {
        return Err(CoverageError {
            line_number: 0,
            line: String::new(),
            reason: "the profile has a header but no coverage blocks; the run measured nothing",
        });
    }
    Ok(finish(covered))
}

/// A Go module found under the repository root: its import path and the
/// directory holding its `go.mod`, repo-relative with `/` separators ("" for
/// the root module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoModule {
    pub path: String,
    pub dir: String,
}

/// Directories never searched for a `go.mod`: they hold other projects' modules
/// or build output, and a module declared there is not this repository's.
const SKIPPED_DIRS: &[&str] = &["vendor", "node_modules", "target", "testdata"];
const MAX_MODULE_DEPTH: usize = 8;
const MAX_MODULE_DIRS: usize = 5_000;
const MAX_GO_MOD_BYTES: u64 = 64 * 1024;

/// Finds the Go modules under `root` by reading each `go.mod`'s `module` line.
///
/// Bounded in depth and in directories visited, and it never follows a
/// symlink, so a crafted tree cannot turn reading a coverage profile into a
/// walk of the machine. A `go.mod` it cannot read or parse is skipped: the
/// files that module would have claimed then report unmeasured, which is the
/// visible outcome.
pub fn go_modules(root: &std::path::Path) -> Vec<GoModule> {
    let mut found = Vec::new();
    let mut visited = 0usize;
    let mut stack: Vec<(std::path::PathBuf, Vec<String>)> = vec![(root.to_path_buf(), Vec::new())];
    while let Some((dir, rel)) = stack.pop() {
        visited += 1;
        if visited > MAX_MODULE_DIRS {
            break;
        }
        if let Some(path) = read_module_path(&dir.join("go.mod")) {
            found.push(GoModule {
                path,
                dir: rel.join("/"),
            });
        }
        if rel.len() >= MAX_MODULE_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if !kind.is_dir() || name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let mut next = rel.clone();
            next.push(name);
            stack.push((entry.path(), next));
        }
    }
    found
}

fn read_module_path(go_mod: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(go_mod).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(go_mod)
        .ok()?
        .take(MAX_GO_MOD_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("module")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let name = rest.trim().trim_matches('"');
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// Maps a Go coverprofile path to the repo-relative path the diff uses.
///
/// A profile records `<module>/<package>/<file>.go`. With the repository's
/// modules known, the longest module path that prefixes the entry is replaced
/// by that module's directory, so `github.com/x/y/pkg/a.go` in the root module
/// `github.com/x/y` is `pkg/a.go`, and in a module `github.com/x/y/sub` living
/// in `sub/` it is `sub/pkg/a.go`. An entry no module claims is kept verbatim —
/// it then matches nothing and is reported unmeasured, rather than being
/// guessed into some other file's coverage.
///
/// With no module information at all (no root, or no `go.mod` under it) the
/// first segment is dropped as before: a one-segment module name is the only
/// shape that guess is right for, and a wrong guess is also reported
/// unmeasured.
fn normalise_go_path(path: &str, modules: &[GoModule]) -> String {
    if modules.is_empty() {
        return match path.split_once('/') {
            Some((_module, rest)) if rest.contains('/') || rest.ends_with(".go") => {
                rest.to_string()
            }
            _ => path.to_string(),
        };
    }
    let best = modules
        .iter()
        .filter(|m| {
            path.strip_prefix(m.path.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|m| m.path.len());
    match best {
        Some(m) => {
            let rest = &path[m.path.len() + 1..];
            if m.dir.is_empty() {
                rest.to_string()
            } else {
                format!("{}/{rest}", m.dir)
            }
        }
        None => path.to_string(),
    }
}

/// Parses LCOV, the format grcov, llvm-cov, and most JS tooling emit.
///
/// Only `SF:` (source file) and `DA:` (line, hits) are read. The function and
/// branch records carry no line-level information the intersection can use, and
/// silently ignoring records is safe here in a way it is not elsewhere: an
/// unknown record cannot make a covered line look uncovered.
fn parse_lcov(
    raw: &str,
    root: Option<&std::path::Path>,
) -> Result<Vec<FileCoverage>, CoverageError> {
    let mut covered: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut seen_any = false;
    // One DA record materialises one line, so this bound is nearly the input's
    // own size — but it is applied for the same reason as in the Go parser, and
    // by the same rule, so neither format has a path to unbounded growth that
    // the other has closed.
    let mut materialised: u64 = 0;

    for (idx, line) in raw.lines().enumerate() {
        let lineno = idx + 1;
        let line = line.trim();
        let bad = |reason: &'static str| CoverageError {
            line_number: lineno,
            line: line.to_string(),
            reason,
        };

        if let Some(path) = line.strip_prefix("SF:") {
            let path = path.trim();
            if path.is_empty() {
                return Err(bad("SF record names no file"));
            }
            covered.entry(normalise_lcov_path(path, root)).or_default();
            current = Some(normalise_lcov_path(path, root));
            continue;
        }
        if let Some(record) = line.strip_prefix("DA:") {
            let file = current
                .as_ref()
                .ok_or_else(|| bad("a DA record appeared before any SF record"))?;
            let (line_no, hits) = record
                .split_once(',')
                .ok_or_else(|| bad("expected `DA:line,hits`"))?;
            let line_no: u32 = line_no
                .trim()
                .parse()
                .map_err(|_| bad("DA line number is not a number"))?;
            // Some producers append a checksum as a third field.
            let hits: u64 = hits
                .split(',')
                .next()
                .unwrap_or_default()
                .trim()
                .parse()
                .map_err(|_| bad("DA hit count is not a number"))?;
            seen_any = true;
            if hits > 0 {
                materialised += 1;
                if materialised > MAX_COVERED_LINES {
                    return Err(bad(
                        "the report covers more lines than any real measurement",
                    ));
                }
                covered.entry(file.clone()).or_default().push(line_no);
            }
            continue;
        }
        if line == "end_of_record" {
            current = None;
        }
    }

    if !seen_any {
        return Err(CoverageError {
            line_number: 0,
            line: String::new(),
            reason: "the LCOV report has no DA records; the run measured no lines",
        });
    }
    Ok(finish(covered))
}

/// Makes an absolute LCOV path repo-relative where it obviously is one.
/// Normalises an LCOV `SF:` path to the repo-relative form the diff uses,
/// against `root` when one is known.
///
/// llvm-cov and grcov emit absolute paths (`SF:/repo/crates/dc-verify/src/lib.rs`),
/// which never matched the repo-relative diff paths — every file landed in
/// `coverage_unmeasured`, training operators to ignore the unmeasured wall. An
/// absolute path is reduced by stripping a matching root prefix; both the raw
/// and canonical spellings of the root are tried, because the root itself may
/// be reached through a symlinked ancestor (macOS /tmp → /private/tmp). A path
/// under no known root is kept verbatim: guessing a different file's coverage
/// is worse than reporting none.
pub fn normalise_lcov_path(path: &str, root: Option<&std::path::Path>) -> String {
    let trimmed = path.trim_start_matches("./");
    let absolute = std::path::Path::new(trimmed);
    // `starts_with('/')` was the absoluteness test, and it is only ever true on
    // Unix. Every `C:\...` path llvm-cov emits on Windows returned here
    // unreduced and landed in `coverage_unmeasured` — the exact wall this
    // function exists to prevent, on the one platform whose tests never ran.
    // `has_root` is true for `/x`, `C:\x`, `\\?\C:\x` and `\\server\share`.
    if !absolute.has_root() {
        return trimmed.to_string();
    }
    let candidates: Vec<std::path::PathBuf> = match root {
        Some(r) => vec![
            r.to_path_buf(),
            r.canonicalize().unwrap_or_else(|_| r.to_path_buf()),
        ],
        None => match std::env::current_dir() {
            Ok(cwd) => vec![cwd.clone(), cwd.canonicalize().unwrap_or(cwd)],
            Err(_) => Vec::new(),
        },
    };
    for base in candidates {
        // Compared by component rather than by string prefix, so the reduction
        // does not depend on the producer and the root agreeing on a separator;
        // Windows accepts both, and llvm-cov and the root need not match.
        let Ok(rest) = absolute.strip_prefix(&base) else {
            continue;
        };
        // Rejoined with `/` because that is the spelling the diff paths this is
        // matched against always use. Rebuilt from components rather than by
        // replacing separators, so a Unix filename that legitimately contains a
        // backslash is never rewritten.
        let rest: Vec<_> = rest
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect();
        // An `SF:` line naming the root itself reduces to nothing. Keeping the
        // path verbatim reports it unmeasured; returning "" would attach its
        // lines to an empty path and silently claim some other file's coverage.
        if !rest.is_empty() {
            return rest.join("/");
        }
    }
    trimmed.to_string()
}

fn finish(covered: BTreeMap<String, Vec<u32>>) -> Vec<FileCoverage> {
    covered
        .into_iter()
        .map(|(path, mut lines)| {
            lines.sort_unstable();
            lines.dedup();
            FileCoverage {
                path,
                covered_lines: lines,
            }
        })
        .collect()
}
