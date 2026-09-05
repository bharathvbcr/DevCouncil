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

    // Languages that now extract calls each own a bucket too, so none of them
    // can absorb a Swift or Kotlin name.
    for owned in ["ruby", "php", "lua", "luau", "r", "dart", "scala"] {
        let family = LangFamily::from_lang(owned);
        assert_ne!(
            family,
            LangFamily::Generic,
            "{owned} extracts calls, so it must not sit in the shared bucket"
        );
        assert_ne!(swift, family, "Swift must not match a {owned} candidate");
        assert_ne!(kotlin, family, "Kotlin must not match a {owned} candidate");
    }

    // `Generic` still exists for languages that contribute no call sites.
    //
    // This list was `["cobol", "solidity"]` and the Solidity half went stale the
    // day Solidity gained call extraction — which is the whole failure mode this
    // file exists to catch, reproduced in the test that guards against it. So
    // membership is now *derived* rather than asserted: a language may sit in
    // the shared bucket only if nothing claims it emits calls.
    for quiet in ["cobol", "html", "css", "json", "yaml", "toml", "markdown"] {
        assert!(
            !devmap_extract::langcalls::CALL_EXTRACTION_LANGUAGES.contains(&quiet),
            "{quiet} is listed as a call-extracting language, so it can no longer \
             be used here as an example of one that is not"
        );
        assert_eq!(
            LangFamily::from_lang(quiet),
            LangFamily::Generic,
            "{quiet} extracts no calls, so the shared bucket is where it belongs"
        );
    }

    // And the bucket is inert regardless, which is what makes the staleness
    // above survivable: two languages that share `Generic` cannot resolve into
    // each other even while someone is still noticing that one of them moved.
    assert!(
        !LangFamily::Generic.admits(LangFamily::Generic),
        "the shared bucket must never admit a resolution; that is how a Svelte \
         call reached a Solidity contract method at 0.9"
    );
    assert_eq!(
        LangFamily::from_lang("solidity"),
        LangFamily::Solidity,
        "Solidity extracts calls now and must own a bucket"
    );
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

/// Any language that extracts calls must own its resolution bucket.
///
/// This is the guard for the defect a sibling agent reproduced while adding Lua
/// and R: with both in `LangFamily::Generic`, an R file calling `process(1)`
/// resolved to a Lua `process` in another file and the graph carried
///
/// ```text
/// caller.r::function -> only_lua.lua::process   Calls   0.9
/// ```
///
/// a cross-language edge that cannot exist, at near-full confidence — the SC9
/// class of confidently-wrong answer. `Generic` is safe only for languages that
/// contribute no call sites at all, so the invariant is not "these particular
/// languages have variants" but "extracting calls implies owning a bucket".
/// Written as a derived check rather than a hand-maintained list, because the
/// next language to gain call extraction will be added by someone who has not
/// read this file.
#[test]
fn every_call_extracting_language_owns_its_bucket() {
    for lang in devmap_extract::langcalls::CALL_EXTRACTION_LANGUAGES {
        let family = LangFamily::from_lang(lang);
        assert_ne!(
            family,
            LangFamily::Generic,
            "{lang} extracts calls but shares the generic bucket, so its calls can \
             resolve to a same-named symbol in an unrelated language at high \
             confidence; give it a LangFamily variant"
        );
    }
}

/// A builtin table's worth is its authority, so each entry must trace to the
/// language's own specification — not to a popular library.
///
/// The check that matters is the negative one. `specify` and `describe` reach a
/// Ruby file through RSpec, `it` through several DSLs, and admitting them would
/// say "the language declares this" about something Ruby says nothing about.
/// That is the one claim `UnresolvedClass::Builtin` must never make: a wrong
/// entry exempts a real defect permanently, while a missing one only
/// over-reports.
#[test]
fn no_builtin_table_admits_a_library_dsl_name() {
    for (family, borrowed) in [
        (LangFamily::Ruby, ["specify", "describe", "it", "expect"]),
        (LangFamily::Php, ["dd", "collect", "route", "view"]),
        (LangFamily::Kotlin, ["given", "then", "shouldBe", "verify"]),
        (LangFamily::Swift, ["XCTAssert", "expect", "describe", "it"]),
    ] {
        for name in borrowed {
            assert!(
                !is_builtin(family, name),
                "{name} comes from a library, not from {family:?}'s specification; \
                 classifying it as Builtin exempts a real defect"
            );
        }
    }
}

/// Ruby and PHP answer from their own standard libraries.
#[test]
fn ruby_and_php_read_their_own_standard_libraries() {
    assert!(is_builtin(LangFamily::Ruby, "puts"));
    assert!(is_builtin(LangFamily::Ruby, "block_given?"));
    assert!(is_builtin(LangFamily::Php, "array_map"));
    assert!(is_builtin(LangFamily::Php, "is_array"));

    assert!(!is_builtin(LangFamily::Php, "puts"), "no cross-talk");
    assert!(!is_builtin(LangFamily::Ruby, "array_map"), "no cross-talk");

    // Enumerable/receiver methods must not be here: the builtin rung only sees
    // a bare callee, so an entry could never match and would only mislead.
    for receiver_method in ["each", "map", "select", "reject"] {
        assert!(
            !is_builtin(LangFamily::Ruby, receiver_method),
            "{receiver_method} is called on a receiver and can never reach this rung"
        );
    }
}
