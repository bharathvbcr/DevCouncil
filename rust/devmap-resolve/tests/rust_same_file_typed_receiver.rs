//! A Rust local typed by a type its own file declares dispatches to that file's
//! method, even when other files declare a type of the same name.
//!
//! `type_methods` is keyed by the *bare* type name, so `Repo.zzwrite` declared
//! in two files is two hits and `receiver_type_method` refused both: the Go
//! package-block rule was the only namesake arm, and Rust had none. That left
//! every test module that builds its own `struct Repo` / `Harness` / `Fixture`
//! beside its own `impl` without a single caller edge whenever any other file
//! used the same name.
//!
//! Rust settles it the way Go does, and more strictly: a module cannot both
//! declare `Repo` and import another `Repo` (E0255), so a bare `Repo` written in
//! a file that declares one is that file's. The arm is deliberately narrow:
//!
//! * the type is declared in the call's own file,
//! * the method hit is in that same file (an `impl Repo` in another module does
//!   not count),
//! * the type was written **unqualified** in the binding (`other::Repo::new()`
//!   names another module's `Repo` even where this file declares its own).

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{
    Resolution, ResolutionKind, ResolutionResult, ResolvedEdge, UnresolvedClass,
};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn write_calls<'a>(result: &'a ResolutionResult, source: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_symbol == source
                && format!("{:?}", edge.edge_kind) == "Calls"
                && edge.target_symbol.ends_with("Repo.zzwrite")
        })
        .collect()
}

fn unresolved_write(result: &ResolutionResult, source: &str) -> Vec<UnresolvedClass> {
    result
        .unresolved
        .iter()
        .filter(|row| row.source_symbol == source && row.callee_name == "zzwrite")
        .map(|row| row.class.clone())
        .collect()
}

const REPO_A: &str = "\
pub struct Repo;
impl Repo {
    pub fn new() -> Self { Repo }
    pub fn zzwrite(&self) {}
}
pub fn t_a() {
    let r = Repo::new();
    r.zzwrite();
}
";

const REPO_B: &str = "\
pub struct Repo;
impl Repo {
    pub fn new() -> Self { Repo }
    pub fn zzwrite(&self) {}
}
pub fn t_b() {
    let r = Repo::new();
    r.zzwrite();
}
";

#[test]
fn each_file_with_its_own_repo_binds_its_own_method() {
    let result = resolve(&[("src/a.rs", REPO_A), ("src/b.rs", REPO_B)]);
    let a = write_calls(&result, "src/a.rs::t_a");
    let b = write_calls(&result, "src/b.rs::t_b");
    assert_eq!(a.len(), 1, "a: {a:?} / unresolved {:?}", result.unresolved);
    assert_eq!(a[0].target_symbol, "src/a.rs::Repo.zzwrite");
    assert_eq!(
        a[0].resolution.as_deref().map(Resolution::kind),
        Some(ResolutionKind::ReceiverType)
    );
    assert_eq!(b.len(), 1, "b: {b:?}");
    assert_eq!(b[0].target_symbol, "src/b.rs::Repo.zzwrite");
}

#[test]
fn a_file_that_declares_no_repo_stays_unresolved() {
    let outsider = "\
pub fn t_c() {
    let r = Repo::new();
    r.zzwrite();
}
";
    let result = resolve(&[
        ("src/a.rs", REPO_A),
        ("src/b.rs", REPO_B),
        ("src/c.rs", outsider),
    ]);
    assert!(write_calls(&result, "src/c.rs::t_c").is_empty());
    assert_eq!(
        unresolved_write(&result, "src/c.rs::t_c"),
        vec![UnresolvedClass::UninferredReceiver]
    );
}

#[test]
fn a_repo_whose_method_is_declared_only_in_another_files_impl_stays_unresolved() {
    // d.rs declares `Repo` but not `zzwrite`; the two hits are a.rs's and b.rs's.
    let declares_only = "\
pub struct Repo;
pub fn t_d() {
    let r = Repo::new();
    r.zzwrite();
}
";
    let result = resolve(&[
        ("src/a.rs", REPO_A),
        ("src/b.rs", REPO_B),
        ("src/d.rs", declares_only),
    ]);
    assert!(
        write_calls(&result, "src/d.rs::t_d").is_empty(),
        "{:?}",
        write_calls(&result, "src/d.rs::t_d")
    );
}

#[test]
fn a_qualified_constructor_names_another_modules_repo() {
    // `other::Repo::new()` is not this file's `Repo`, even though this file
    // declares one with the same method.
    let qualified = "\
pub struct Repo;
impl Repo {
    pub fn zzwrite(&self) {}
}
pub fn t_q() {
    let r = other::Repo::new();
    r.zzwrite();
}
pub fn t_q2(r: &other::Repo) {
    r.zzwrite();
}
";
    let result = resolve(&[
        ("src/a.rs", REPO_A),
        ("src/b.rs", REPO_B),
        ("src/q.rs", qualified),
    ]);
    for source in ["src/q.rs::t_q", "src/q.rs::t_q2"] {
        assert!(
            write_calls(&result, source).is_empty(),
            "{source}: {:?}",
            write_calls(&result, source)
        );
    }
}

#[test]
fn a_bare_constructor_other_than_new_and_a_bare_parameter_type_both_bind() {
    let ctor = "\
pub struct Repo;
impl Repo {
    pub fn init() -> Self { Repo }
    pub fn zzwrite(&self) {}
}
pub fn t_i() {
    let r = Repo::init();
    r.zzwrite();
}
pub fn t_p(r: &Repo) {
    r.zzwrite();
}
";
    let result = resolve(&[("src/i.rs", ctor), ("src/b.rs", REPO_B)]);
    for source in ["src/i.rs::t_i", "src/i.rs::t_p"] {
        let edges = write_calls(&result, source);
        assert_eq!(edges.len(), 1, "{source}: {edges:?}");
        assert_eq!(edges[0].target_symbol, "src/i.rs::Repo.zzwrite");
    }
}

#[test]
fn a_cfg_duplicated_repo_in_one_file_still_binds_that_files_method() {
    // Both cfg variants mint the one qualified name `Repo.zzwrite`, so the file
    // has one answer; the namesake in b.rs is not it.
    let dup = "\
#[cfg(unix)]
pub struct Repo;
#[cfg(unix)]
impl Repo {
    pub fn new() -> Self { Repo }
    pub fn zzwrite(&self) {}
}
#[cfg(not(unix))]
pub struct Repo;
#[cfg(not(unix))]
impl Repo {
    pub fn new() -> Self { Repo }
    pub fn zzwrite(&self) {}
}
pub fn t_e() {
    let r = Repo::new();
    r.zzwrite();
}
";
    let result = resolve(&[("src/e.rs", dup), ("src/b.rs", REPO_B)]);
    let e = write_calls(&result, "src/e.rs::t_e");
    assert!(
        e.iter().all(|edge| edge.target_symbol == "src/e.rs::Repo.zzwrite") && !e.is_empty(),
        "{e:?}"
    );
}

#[test]
fn two_repos_in_one_file_in_different_modules_abstain() {
    let nested = "\
mod left {
    pub struct Repo;
    impl Repo {
        pub fn new() -> Self { Repo }
        pub fn zzwrite(&self) {}
    }
}
mod right {
    pub struct Repo;
    impl Repo {
        pub fn new() -> Self { Repo }
        pub fn zzwrite(&self) {}
    }
}
pub fn t_n() {
    let r = Repo::new();
    r.zzwrite();
}
";
    let result = resolve(&[("src/n.rs", nested), ("src/b.rs", REPO_B)]);
    let n: Vec<_> = write_calls(&result, "src/n.rs::t_n")
        .into_iter()
        .map(|edge| edge.target_symbol.clone())
        .collect();
    assert!(n.is_empty(), "{n:?}");
}
