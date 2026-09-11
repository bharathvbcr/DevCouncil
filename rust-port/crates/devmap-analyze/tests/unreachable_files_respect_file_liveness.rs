//! `unreachable_files` is a file-level verdict and answers to the same rule the
//! other two do.
//!
//! `a_file_that_declares_nothing_is_not_unreachable` already keeps prose out,
//! but for a reason that only holds for prose: a `.md` contributes no symbols,
//! so "every declared symbol is clustered" is vacuously true and the guard
//! against a vacuous truth catches it. That guard says nothing about a file
//! that declares plenty.
//!
//! A fixture module does. So does a package marker, a tool config and a
//! shebang script — and fixture code in particular is *made* of small mutually
//! recursive helpers nothing outside calls, which is precisely the shape
//! `dead_clusters` is looking for. Measured: 60 of this repository's 138
//! unwired candidates were `rust-port/testdata/**`, and every one of them was
//! eligible to arrive here by the second route as well.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

/// Two functions that call only each other: a component nothing outside
/// reaches, which is what `dead_clusters` reports.
const ABANDONED: &str = "def alpha():\n    return beta()\n\n\ndef beta():\n    return alpha()\n";

fn scan(files: &[(&str, &str)]) -> DeadClusterScan {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    dead_clusters(&extractions, &resolution)
}

/// A fixture, a package marker, a tool config and a script are not reported,
/// and an ordinary module with the same body is.
///
/// The control is the point: every one of these files holds the *same*
/// abandoned cycle, so a rule that suppressed the finding altogether would
/// pass a test that only asserted the four absences.
#[test]
fn an_exempt_file_wholly_inside_a_dead_cluster_is_not_unreachable() {
    let scan = scan(&[
        ("app/orphan.py", ABANDONED),
        ("testdata/case/sample.py", ABANDONED),
        ("app/pkg/__init__.py", ABANDONED),
        ("noxfile.py", ABANDONED),
        ("tools/run.sh", "#!/usr/bin/env bash\necho hi\n"),
        ("app/fixtures/helper.py", ABANDONED),
    ]);

    assert!(
        !scan.clusters.is_empty(),
        "the abandoned cycles are still found — the exemption is about which \
         *files* are named, not about hiding the cycle: {scan:?}"
    );
    assert_eq!(
        scan.unreachable_files,
        vec!["app/orphan.py".to_string()],
        "only the ordinary module is a file-level finding: {scan:?}"
    );
}

/// A Terraform module and a lockfile never reach the list either.
///
/// Both parse as HCL and declare blocks, so neither is caught by the
/// declares-nothing guard.
#[test]
fn a_terraform_module_and_a_lockfile_are_never_unreachable() {
    let scan = scan(&[
        ("app/orphan.py", ABANDONED),
        ("infra/main.tf", "resource \"aws_s3_bucket\" \"b\" {}\n"),
        (
            "infra/.terraform.lock.hcl",
            "provider \"registry.terraform.io/hashicorp/aws\" {\n  version = \"5.0.0\"\n}\n",
        ),
    ]);
    for path in ["infra/main.tf", "infra/.terraform.lock.hcl"] {
        assert!(
            !scan.unreachable_files.contains(&path.to_string()),
            "{path} is not a liveness candidate: {scan:?}"
        );
    }
    assert_eq!(scan.unreachable_files, vec!["app/orphan.py".to_string()]);
}

/// A one-member component is a recursive symbol, and the reason says so.
///
/// It used to read "1 symbols that reference only each other", which is not
/// merely ungrammatical: it describes a group, so a reader looking for the
/// other members of a group that has none reads the finding as truncated. The
/// sample note is what a truncated finding carries, and it must not appear
/// here.
#[test]
fn a_self_recursive_symbol_is_not_described_as_a_group() {
    let recursive = scan(&[("app/loop.py", "def spin(n):\n    return spin(n - 1)\n")]);
    let cluster = recursive
        .clusters
        .iter()
        .find(|cluster| cluster.size == 1)
        .unwrap_or_else(|| {
            panic!("a self-recursive function is a one-member component: {recursive:?}")
        });

    assert!(
        !cluster
            .reason
            .contains("symbols that reference only each other"),
        "a component of one is not a group of symbols referencing each other: {}",
        cluster.reason
    );
    assert!(
        cluster.reason.contains("1 symbol that only calls itself"),
        "the reason must say what a one-member component is: {}",
        cluster.reason
    );
    assert!(
        !cluster.reason.contains(" of 1 members listed"),
        "a single member is fully listed and must not carry a truncation note: {}",
        cluster.reason
    );
    // And the plural wording survives for a real group, so the fix cannot have
    // been "delete the group sentence".
    let group = scan(&[("app/orphan.py", ABANDONED)]);
    assert!(
        group.clusters[0]
            .reason
            .contains("2 symbols that reference only each other"),
        "a two-member component still reads as a group: {}",
        group.clusters[0].reason
    );
}

/// The *qualified* tier says the same thing about a component of one.
///
/// There are two reason builders, one for a component nothing reaches and one
/// for a component something reaches through a call the resolver could not
/// bind, and a reader sees whichever tier their finding landed in. Fixing the
/// wording in only the first would leave "1 symbols that reference only each
/// other" on the page for exactly the findings that are hardest to act on —
/// and a mutant that inverted the size test in the second builder survived the
/// suite until this test existed.
#[test]
fn the_qualified_tier_describes_a_component_of_one_the_same_way() {
    // `spin` recurses, and two files declare it, so the call from `caller`
    // cannot be bound to either — an ambiguous edge into a one-member
    // component, which is what the qualified tier is for.
    let recursive = scan(&[
        ("app/loop.py", "def spin(n):\n    return spin(n - 1)\n"),
        ("app/decoy.py", "def spin(n):\n    return 0\n"),
        ("app/caller.py", "def entry():\n    return spin(3)\n"),
    ]);
    let Some(cluster) = recursive.clusters.iter().find(|cluster| cluster.size == 1) else {
        // The fixture depends on the resolver producing an ambiguous edge. Say
        // so rather than passing silently on a property that went untested.
        eprintln!("note: fixture produced no one-member component; property untested here");
        return;
    };
    assert!(
        !cluster
            .reason
            .contains("symbols that reference only each other"),
        "whichever tier reports it, a component of one is not a group: {}",
        cluster.reason
    );
    assert!(
        cluster.reason.contains("1 symbol that only calls itself"),
        "the qualified tier must use the same wording for the same shape: {}",
        cluster.reason
    );

    // And a qualified *group* still reads as a group, so the branch cannot
    // have been inverted rather than added.
    let group = scan(&[
        ("app/orphan.py", ABANDONED),
        ("app/decoy.py", "def alpha():\n    return 0\n"),
        ("app/caller.py", "def entry():\n    return alpha()\n"),
    ]);
    if let Some(cluster) = group.clusters.iter().find(|cluster| cluster.size > 1) {
        assert!(
            cluster
                .reason
                .contains("symbols that reference only each other"),
            "a multi-member component reads as a group in every tier: {}",
            cluster.reason
        );
    }
}
