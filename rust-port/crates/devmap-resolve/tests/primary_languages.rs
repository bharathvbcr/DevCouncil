//! Swift and Kotlin are primary languages for this repository.
//!
//! Both fell to `LangFamily::Generic` until this pass. `Generic` is not a
//! neutral default — `resolver.rs` filters candidates with
//! `*candidate_family == family`, so it is one shared namespace spanning Swift,
//! Kotlin, Ruby, PHP, Lua, R, COBOL and Solidity. Every bare name in any of
//! those languages competed for the same bucket.

use devmap_resolve::builtins::is_builtin;
use devmap_resolve::model::LangFamily;

/// Swift and Kotlin each own a bucket, and neither shares one with the other
/// or with the languages still in `Generic`.
///
/// This is what stops a Swift `run()` resolving to a Ruby `run()` once call
/// extraction lands for both. Written now rather than after, because the moment
/// those calls exist the defect is a wrong edge at full confidence, not a
/// missing one.
#[test]
fn swift_and_kotlin_do_not_share_a_resolution_bucket() {
    let swift = LangFamily::from_lang("swift");
    let kotlin = LangFamily::from_lang("kotlin");

    assert_eq!(swift, LangFamily::Swift);
    assert_eq!(kotlin, LangFamily::Kotlin);
    assert_ne!(
        swift, kotlin,
        "a Swift name must not match a Kotlin candidate"
    );

    for shared in ["ruby", "php", "lua", "r", "cobol", "solidity"] {
        let family = LangFamily::from_lang(shared);
        assert_eq!(
            family,
            LangFamily::Generic,
            "{shared} is expected to still share the generic bucket"
        );
        assert_ne!(swift, family, "Swift must not match a {shared} candidate");
        assert_ne!(kotlin, family, "Kotlin must not match a {shared} candidate");
    }
}

/// Each primary language answers from its own standard library, and only for
/// names that language actually declares.
#[test]
fn each_primary_language_reads_its_own_standard_library() {
    assert!(is_builtin(LangFamily::Swift, "fatalError"));
    assert!(is_builtin(LangFamily::Kotlin, "println"));

    // Cross-talk is the failure the separate buckets exist to prevent.
    assert!(
        !is_builtin(LangFamily::Kotlin, "fatalError"),
        "Kotlin must not claim a Swift builtin"
    );
    assert!(
        !is_builtin(LangFamily::Swift, "println"),
        "Swift must not claim a Kotlin builtin"
    );

    // `print` is genuinely declared by both, so both must answer for it —
    // the tables are per-language, not a partition of one list.
    assert!(is_builtin(LangFamily::Swift, "print"));
    assert!(is_builtin(LangFamily::Kotlin, "print"));

    // Names deliberately withheld under the SC30 rule stay unresolved, because
    // a repository plausibly declares them itself.
    for withheld in ["max", "min", "abs", "swap"] {
        assert!(
            !is_builtin(LangFamily::Swift, withheld),
            "{withheld} is stdlib but plausibly repository-declared; it must stay a possible defect"
        );
    }
    for withheld in ["error", "check", "require", "repeat"] {
        assert!(
            !is_builtin(LangFamily::Kotlin, withheld),
            "{withheld} is stdlib but plausibly repository-declared; it must stay a possible defect"
        );
    }
}
