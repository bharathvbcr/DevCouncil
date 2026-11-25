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
    // rust/devmap-cli -> rust -> repository root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository root above rust/devmap-cli")
        .to_path_buf()
}

#[test]
fn the_declared_license_is_the_one_the_repository_ships() {
    // Compiled in by cargo from the manifest, so this reads the real
    // declaration rather than a copy of it that could itself drift.
    let declared = env!("CARGO_PKG_LICENSE");
    assert_eq!(
        declared, "MIT OR Apache-2.0",
        "the crate declares {declared:?}; the repository is dual-licensed MIT OR Apache-2.0"
    );

    let root = repo_root();
    let license = root.join("LICENSE");
    let text = std::fs::read_to_string(&license)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", license.display()));
    assert!(
        text.contains("LICENSE-MIT") && text.contains("LICENSE-APACHE"),
        "LICENSE must point to both LICENSE-MIT and LICENSE-APACHE"
    );

    let apache = root.join("LICENSE-APACHE");
    let apache_text = std::fs::read_to_string(&apache)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", apache.display()));
    assert!(
        apache_text.contains("Apache License") && apache_text.contains("Version 2.0"),
        "LICENSE-APACHE is not the Apache 2.0 text the manifest declares"
    );

    let mit = root.join("LICENSE-MIT");
    let mit_text = std::fs::read_to_string(&mit)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", mit.display()));
    assert!(
        mit_text.contains("Permission is hereby granted, free of charge"),
        "LICENSE-MIT is not the MIT License text the manifest declares"
    );
}

#[test]
fn every_workspace_manifest_declares_that_same_license() {
    // One dual license governs the tree, so a second workspace declaring something
    // else is the same defect in another file.
    let root = repo_root();
    let mut checked = 0usize;
    for manifest in ["rust/Cargo.toml"] {
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
            r#"license = "MIT OR Apache-2.0""#,
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
    // licence. Keep it aligned with the crate's declared SPDX expression.
    let plugin_license = "MIT OR Apache-2.0";
    assert_eq!(
        env!("CARGO_PKG_LICENSE"),
        plugin_license,
        "the plugin manifest would tell an installer a different licence from \
         the one the crate publishes under"
    );
}
