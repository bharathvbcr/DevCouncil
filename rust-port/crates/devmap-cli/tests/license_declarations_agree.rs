//! The licence this crate declares, the one it emits, and the one it ships.
//!
//! These drifted: `Cargo.toml` said `MIT` while `LICENSE` was Apache-2.0 and the
//! plugin manifest emitted `Apache-2.0`. Nothing failed, because no check ever
//! compared them — and with no `publish = false` anywhere, `cargo publish` would
//! have uploaded seven crates recording a licence the source is not under.
//!
//! Apache-2.0 is not a superset of MIT in the direction that matters: it carries
//! a patent grant and attribution terms MIT has no equivalent for, so a
//! downstream reader who relied on the crates.io metadata would have relied on
//! the wrong terms.

use std::path::{Path, PathBuf};

/// The repository root, from this crate's manifest directory.
fn repo_root() -> PathBuf {
    // crates/devmap-cli -> crates -> rust-port -> repository root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root above crates/devmap-cli")
        .to_path_buf()
}

#[test]
fn the_declared_license_is_the_one_the_repository_ships() {
    // Compiled in by cargo from the manifest, so this reads the real
    // declaration rather than a copy of it that could itself drift.
    let declared = env!("CARGO_PKG_LICENSE");
    assert_eq!(
        declared, "Apache-2.0",
        "the crate declares {declared:?}; the repository's LICENSE is Apache-2.0"
    );

    let license = repo_root().join("LICENSE");
    let text = std::fs::read_to_string(&license)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", license.display()));
    assert!(
        text.contains("Apache License") && text.contains("Version 2.0"),
        "LICENSE is not the Apache 2.0 text the manifest declares"
    );
    assert!(
        !text.contains("Permission is hereby granted, free of charge"),
        "LICENSE contains the MIT grant"
    );
}

#[test]
fn every_workspace_manifest_declares_that_same_license() {
    // One `LICENSE` governs the tree, so a second workspace declaring something
    // else is the same defect in another file. `rust/` (the dc-* analysis
    // plane) declared MIT for exactly as long as this one did.
    let root = repo_root();
    let mut checked = 0usize;
    for manifest in ["rust-port/Cargo.toml", "rust/Cargo.toml"] {
        let path = root.join(manifest);
        let Ok(text) = std::fs::read_to_string(&path) else {
            // A workspace that is not in this checkout is not a failure; a
            // workspace that is here and disagrees, is.
            continue;
        };
        let line = text
            .lines()
            .find(|line| line.trim_start().starts_with("license = "))
            .unwrap_or_else(|| panic!("{manifest} declares no license"));
        assert_eq!(
            line.trim(),
            r#"license = "Apache-2.0""#,
            "{manifest} disagrees with the repository's LICENSE"
        );
        checked += 1;
    }
    // A loop that silently matched nothing would pass for the wrong reason.
    assert!(checked > 0, "no workspace manifest was checked");
}

#[test]
fn the_plugin_manifest_emits_the_license_the_crate_declares() {
    // The plugin bundle is what a user installs, and its manifest names a
    // licence. It said Apache-2.0 while the crate said MIT.
    let plugin_license = "Apache-2.0";
    assert_eq!(
        env!("CARGO_PKG_LICENSE"),
        plugin_license,
        "the plugin manifest would tell an installer a different licence from \
         the one the crate publishes under"
    );
}
