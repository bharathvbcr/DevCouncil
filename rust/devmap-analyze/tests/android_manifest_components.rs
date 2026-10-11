//! A class only an `AndroidManifest.xml` names is live, and says why.
//!
//! The framework instantiates `<activity>`, `<service>`, `<receiver>` and
//! `<provider>` classes from the manifest, so no call in the corpus constructs
//! them. Before the manifest was read, such a class was exempt only when it
//! happened to be public or extended a framework base the extractor knew by
//! name — the exemption said nothing about the manifest, and a class with a
//! base the list did not know was a dead-code candidate.

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_resolve::Resolver;

fn exemption(extractions: &[Extraction], file: &str, symbol: &str) -> Option<String> {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    analysis
        .dead_symbols
        .iter()
        .find(|report| report.file_path == file && report.symbol_name == symbol)
        .and_then(|report| report.exemption_reason.clone())
}

const MANIFEST: &str = r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <application>
        <receiver android:name="com.fx.app.Boot" android:exported="false"/>
        <service android:name=".Sync"/>
    </application>
</manifest>"#;

#[test]
fn a_receiver_the_manifest_declares_is_exempt_with_the_manifest_as_the_reason() {
    let extractions = [
        extract_file("app/src/main/AndroidManifest.xml", MANIFEST),
        // A base class the extractor's framework list does not know, so the
        // only evidence of use is the manifest.
        extract_file(
            "app/src/main/java/com/fx/app/Boot.kt",
            "package com.fx.app\n\nclass Boot : VendorBase()\n",
        ),
        // Declared relative (`.Sync`): matched by suffix inside the module.
        extract_file(
            "app/src/main/java/com/fx/app/Sync.kt",
            "package com.fx.app\n\nclass Sync : VendorBase()\n",
        ),
        // The same name in another module is not the manifest's class.
        extract_file(
            "other/src/main/java/com/fx/app/Boot.kt",
            "package com.fx.app\n\nclass Boot : VendorBase()\n",
        ),
    ];
    let boot = exemption(&extractions, "app/src/main/java/com/fx/app/Boot.kt", "Boot")
        .expect("the manifest names it");
    assert!(
        boot.contains("app/src/main/AndroidManifest.xml") && boot.contains("<receiver>"),
        "the reason must name the manifest and the tag: {boot}"
    );
    let sync = exemption(&extractions, "app/src/main/java/com/fx/app/Sync.kt", "Sync")
        .expect("a relative name matches inside the module");
    assert!(sync.contains("<service>"), "{sync}");
    let elsewhere = exemption(
        &extractions,
        "other/src/main/java/com/fx/app/Boot.kt",
        "Boot",
    );
    assert!(
        !elsewhere.is_some_and(|reason| reason.contains("AndroidManifest")),
        "a manifest governs its own module only"
    );
}

#[test]
fn a_manifest_that_cannot_be_read_is_a_recorded_failure_not_an_empty_one() {
    let broken = "<manifest><application><receiver android:name=\"com.fx.app.Boot\"";
    let extraction = extract_file("app/src/main/AndroidManifest.xml", broken);
    match &extraction.parse_outcome {
        ParseOutcome::Failed { reason } => assert!(
            reason.contains("AndroidManifest.xml could not be read"),
            "{reason}"
        ),
        other => panic!("an unreadable manifest must be a failure, got {other:?}"),
    }
    assert!(
        extraction.wiring.is_empty(),
        "a manifest that could not be read makes no claim: {:?}",
        extraction.wiring
    );
    // And the readable one is a clean, claim-bearing extraction.
    let good = extract_file("app/src/main/AndroidManifest.xml", MANIFEST);
    assert!(!matches!(good.parse_outcome, ParseOutcome::Failed { .. }));
    assert_eq!(good.wiring.len(), 2, "{:?}", good.wiring);
}

#[test]
fn a_bare_component_name_is_relative_to_the_namespace() {
    // `android:name="Sync"`, no leading dot: Android reads it against the
    // namespace exactly as it reads `.Sync`. A class whose name only ends in
    // the same letters is not it.
    let manifest = r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <application><service android:name="Sync"/></application>
</manifest>"#;
    let extractions = [
        extract_file("app/src/main/AndroidManifest.xml", manifest),
        extract_file(
            "app/src/main/java/com/fx/app/Sync.kt",
            "package com.fx.app\n\nclass Sync : VendorBase()\n",
        ),
        extract_file(
            "app/src/main/java/com/fx/app/AutoSync.kt",
            "package com.fx.app\n\nclass AutoSync : VendorBase()\n",
        ),
    ];
    let sync = exemption(&extractions, "app/src/main/java/com/fx/app/Sync.kt", "Sync")
        .expect("a bare name matches inside the module");
    assert!(sync.contains("<service>"), "{sync}");
    let auto = exemption(
        &extractions,
        "app/src/main/java/com/fx/app/AutoSync.kt",
        "AutoSync",
    );
    assert!(
        !auto.is_some_and(|reason| reason.contains("AndroidManifest")),
        "a suffix that is not a whole name segment does not match"
    );
}
