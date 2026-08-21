//! Liveness for Go symbols that exist only as mutually exclusive build variants.
//!
//! Go forbids two package-level declarations of one name, so a package that
//! declares `configureProcessGroup` in two files can only compile because those
//! files are mutually exclusive — `//go:build unix` against `//go:build !unix`.
//! Exactly one reaches any given build, and a call naming that identity reaches
//! whichever one compiled. Every variant is live.
//!
//! The resolver cannot see that. It finds N definitions of one name, cannot
//! choose, and emits `AmbiguousGlobal`; liveness then downgraded every candidate
//! to `only_ambiguous_callers`, which reads as "this might be dead" about code
//! guaranteed to be running. Measured on a Go-heavy external corpus, this shape
//! was **all 16** of its non-exempt findings — process-group setup, build
//! locking, and the terminal raw-mode and signal layers, every one of them on a
//! live call path.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    analyze_liveness(&extractions, &resolution)
}

fn report_for<'a>(reports: &'a [DeadSymbolReport], file: &str) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|report| report.file_path == file)
}

const CALLER: &str = concat!(
    "package store\n",
    "type Store struct{}\n",
    "func (s *Store) Spawn() { configureProcessGroup() }\n",
    "func (s *Store) trulyDead() int { return 1 }\n",
);
const UNIX: &str = concat!(
    "//go:build unix\n",
    "\n",
    "package store\n",
    "func configureProcessGroup() {}\n",
);
const OTHER: &str = concat!(
    "//go:build !unix\n",
    "\n",
    "package store\n",
    "func configureProcessGroup() {}\n",
);

/// Every variant of a called identity is exempt, and says why.
#[test]
fn a_called_build_variant_is_exempt_in_every_file_that_declares_it() {
    let reports = reports(&[
        ("pkg/store/store.go", CALLER),
        ("pkg/store/procgroup_unix.go", UNIX),
        ("pkg/store/procgroup_other.go", OTHER),
    ]);
    for file in [
        "pkg/store/procgroup_unix.go",
        "pkg/store/procgroup_other.go",
    ] {
        let found = report_for(&reports, file)
            .unwrap_or_else(|| panic!("{file} must appear in the report at all: {reports:?}"));
        assert!(
            found.is_exempt,
            "{file} is on a live call path on its own platform; reporting it is a \
             proposal to delete working code: {found:?}"
        );
        assert_eq!(
            found.exemption_reason.as_deref(),
            Some(GO_BUILD_VARIANT_REASON),
            "the exemption must name the build constraint, not a generic blanket"
        );
    }
}

/// The exemption explains an *ambiguity*, never an absence of callers.
///
/// This is the check that keeps the rule from being a blanket amnesty for
/// build-tagged files. Two variants nothing calls are dead on every platform,
/// and collapsing this case into the one above would silently disable dead-code
/// detection for every `_unix.go`/`_windows.go` pair in a repository.
#[test]
fn build_variants_that_nothing_calls_are_still_confidently_dead() {
    let orphan_unix = concat!(
        "//go:build unix\n",
        "\n",
        "package store\n",
        "func neverCalledAnywhere() int { return 1 }\n",
    );
    let orphan_other = concat!(
        "//go:build !unix\n",
        "\n",
        "package store\n",
        "func neverCalledAnywhere() int { return 2 }\n",
    );
    let reports = reports(&[
        ("pkg/store/store.go", CALLER),
        ("pkg/store/orphan_unix.go", orphan_unix),
        ("pkg/store/orphan_other.go", orphan_other),
    ]);
    for file in ["pkg/store/orphan_unix.go", "pkg/store/orphan_other.go"] {
        let found = report_for(&reports, file)
            .unwrap_or_else(|| panic!("{file} must be reported: {reports:?}"));
        assert!(
            !found.is_exempt && found.confidence > 0.5,
            "nothing calls this identity on any platform, so it is dead on every \
             one of them: {found:?}"
        );
    }
}

/// A genuine finding in an ordinary file of the same package is untouched.
#[test]
fn an_ordinary_dead_symbol_in_the_same_package_is_still_reported() {
    let reports = reports(&[
        ("pkg/store/store.go", CALLER),
        ("pkg/store/procgroup_unix.go", UNIX),
        ("pkg/store/procgroup_other.go", OTHER),
    ]);
    let found = reports
        .iter()
        .find(|report| report.symbol_name == "Store.trulyDead")
        .unwrap_or_else(|| panic!("Store.trulyDead must be reported: {reports:?}"));
    assert!(
        !found.is_exempt && found.confidence > 0.5,
        "the exemption must not leak to unconstrained files: {found:?}"
    );
}

/// Duplicates that carry **no** constraint are not build variants.
///
/// Two unconstrained files declaring one name is a package that does not
/// compile, or an extraction bug. Either way it is not evidence the symbol is
/// alive, and requiring *every* declaring file to carry a constraint is what
/// separates the two cases.
#[test]
fn unconstrained_duplicates_are_never_treated_as_build_variants() {
    let a = concat!(
        "package bad\n",
        "func dup() int { return 1 }\n",
        "func Use() int { return dup() }\n",
    );
    let b = concat!("package bad\n", "func dup() int { return 2 }\n");
    let reports = reports(&[("pkg/bad/a.go", a), ("pkg/bad/b.go", b)]);
    for report in &reports {
        assert_ne!(
            report.exemption_reason.as_deref(),
            Some(GO_BUILD_VARIANT_REASON),
            "no file here carries a build constraint: {report:?}"
        );
    }
}

/// The implicit `_GOOS` filename constraint counts, with no directive present.
///
/// Go applies it silently, so a repository that relies on it — `net_linux.go`
/// against `net_windows.go`, no `//go:build` line in either — has exactly the
/// same guarantee and must get the same answer.
#[test]
fn a_goos_filename_suffix_constrains_the_build_without_a_directive() {
    let caller = concat!(
        "package net\n",
        "type Dialer struct{}\n",
        "func (d *Dialer) Dial() { setSockOpt() }\n",
    );
    let reports = reports(&[
        ("pkg/net/dial.go", caller),
        (
            "pkg/net/sock_linux.go",
            "package net\nfunc setSockOpt() {}\n",
        ),
        (
            "pkg/net/sock_windows.go",
            "package net\nfunc setSockOpt() {}\n",
        ),
    ]);
    for file in ["pkg/net/sock_linux.go", "pkg/net/sock_windows.go"] {
        let found = report_for(&reports, file)
            .unwrap_or_else(|| panic!("{file} must appear in the report: {reports:?}"));
        assert!(
            found.is_exempt,
            "`_linux.go`/`_windows.go` carry Go's implicit constraint: {found:?}"
        );
    }
}

/// A filename that merely *looks* like a constraint is not one.
///
/// `unix` is a legal `//go:build` tag but not a `GOOS`, so `raw_unix.go` is
/// constrained by its directive and never by its name; `helpers_other.go` and
/// `linux.go` are not constrained at all. Reading any `_word.go` as a
/// constraint would exempt duplicate declarations across ordinary files.
#[test]
fn a_suffix_that_is_not_a_goos_or_goarch_does_not_constrain() {
    let caller = concat!("package p\n", "func Use() { helper() }\n");
    let reports = reports(&[
        ("pkg/p/use.go", caller),
        ("pkg/p/helpers_other.go", "package p\nfunc helper() {}\n"),
        ("pkg/p/helpers_extra.go", "package p\nfunc helper() {}\n"),
    ]);
    for report in &reports {
        assert_ne!(
            report.exemption_reason.as_deref(),
            Some(GO_BUILD_VARIANT_REASON),
            "`_other` and `_extra` are not GOOS/GOARCH values: {report:?}"
        );
    }
}
