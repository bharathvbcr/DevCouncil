//! Tests for Lean 4 (`.lean`) extraction and Trusted Computing Base (TCB) boundary auditing.

use devmap_extract::lean::{analyze_lean_tcb, LeanTcbHoleKind, LeanTcbRole};
use devmap_extract::model::{ExtractionEngine, ParseOutcome, ReferenceKind, SymbolKind};
use devmap_extract::{detect_language, extract_file};
use std::path::Path;

#[test]
fn lean_extraction_recovers_namespaces_declarations_heritage_and_references() {
    assert_eq!(detect_language(Path::new("specs/SignalX3DH.lean")), "lean");

    let src = r#"
import Mathlib.Data.ZMod.Basic
/- Outer comment
   /- Nested comment mentioning theorem fake_decl := sorry -/
   still inside comment -/
namespace Crypto.X3DH

structure TranscriptBase where
  sid : Nat

structure InitiatorState extends TranscriptBase where
  ephemeral : Nat

private def deriveKey (st : InitiatorState) : Nat :=
  st.sid

theorem deriveKey_sound (st : InitiatorState) : deriveKey st = st.sid := by
  rfl

end Crypto.X3DH
"#;

    let ext = extract_file("specs/SignalX3DH.lean", src);
    assert_eq!(ext.language, "lean");
    assert!(matches!(ext.engine, ExtractionEngine::RegexFallback { .. }));
    assert!(matches!(ext.parse_outcome, ParseOutcome::Fallback { .. }));
    assert!(!ext.is_parse_failure());

    let qnames: Vec<&str> = ext
        .symbols
        .iter()
        .map(|s| s.qualified_name.as_str())
        .collect();
    assert!(qnames.contains(&"Crypto.X3DH.TranscriptBase"));
    assert!(qnames.contains(&"Crypto.X3DH.InitiatorState"));
    assert!(qnames.contains(&"Crypto.X3DH.deriveKey"));
    assert!(qnames.contains(&"Crypto.X3DH.deriveKey_sound"));
    assert!(
        !qnames.iter().any(|q| q.contains("fake_decl")),
        "declarations inside nested /- ... -/ comments must never be extracted"
    );

    let derive_sym = ext
        .symbols
        .iter()
        .find(|s| s.qualified_name == "Crypto.X3DH.deriveKey")
        .unwrap();
    assert!(
        !derive_sym.is_exported,
        "`private def` must set is_exported=false"
    );
    assert_eq!(derive_sym.kind, SymbolKind::Function);

    let has_heritage = ext.references.iter().any(|r| {
        r.kind == ReferenceKind::Heritage
            && r.name == "TranscriptBase"
            && r.enclosing_symbol.as_deref() == Some("Crypto.X3DH.InitiatorState")
    });
    assert!(
        has_heritage,
        "expected Heritage reference from InitiatorState to TranscriptBase"
    );

    let has_call_ref = ext.references.iter().any(|r| {
        r.kind == ReferenceKind::Call
            && r.name == "deriveKey"
            && r.enclosing_symbol.as_deref() == Some("Crypto.X3DH.deriveKey_sound")
    });
    assert!(
        has_call_ref,
        "expected Call reference to deriveKey inside theorem"
    );
}

#[test]
fn empty_or_comment_only_lean_file_is_not_a_parse_failure() {
    let ext = extract_file("specs/Empty.lean", "-- header comment only\n/- empty -/\n");
    assert_eq!(ext.language, "lean");
    assert!(!ext.is_parse_failure());
    assert!(matches!(ext.parse_outcome, ParseOutcome::Fallback { .. }));
}

#[test]
fn lean_tcb_auditor_accepts_sound_proof_and_rejects_every_tcb_escape() {
    let sound_src = r#"
import Init.Data.BitVec

namespace SymCrypt.Kyber

def compress (x : Nat) : Nat := x

theorem compress_idempotent (x : Nat) : compress (compress x) = compress x := by
  rfl

end SymCrypt.Kyber
"#;
    let sound_report = analyze_lean_tcb(sound_src);
    assert!(
        sound_report.is_sound_tcb(),
        "sound proof must pass TCB audit"
    );
    assert_eq!(sound_report.imports.len(), 1);
    assert_eq!(sound_report.imports[0].module_specifier, "Init.Data.BitVec");
    assert_eq!(sound_report.declarations.len(), 2);
    assert_eq!(sound_report.declarations[1].role, LeanTcbRole::VerifiedSpec);

    // Disproof 1: `sorry` inside proof body (must fail even if string/comment also mentions sorry)
    let sorry_report = analyze_lean_tcb(
        "theorem bogus (n : Nat) : n = n + 1 := by\n  -- not a sorry here\n  sorry\n",
    );
    assert!(!sorry_report.is_sound_tcb());
    assert!(sorry_report.holes.iter().any(
        |h| h.kind == LeanTcbHoleKind::Sorry && h.enclosing_symbol.as_deref() == Some("bogus")
    ));

    // Disproof 2: `native_decide` trusts compiler codegen outside Lean kernel
    let native_decide_report =
        analyze_lean_tcb("theorem fast_check : 2 + 2 = 4 := by native_decide\n");
    assert!(!native_decide_report.is_sound_tcb());
    assert!(native_decide_report
        .holes
        .iter()
        .any(|h| h.kind == LeanTcbHoleKind::NativeDecide));

    // Disproof 3: `axiom` enlarges logical theory
    let axiom_report = analyze_lean_tcb("axiom ddh_hard : True\n");
    assert!(!axiom_report.is_sound_tcb());
    assert_eq!(axiom_report.declarations[0].role, LeanTcbRole::TrustedAxiom);

    // Disproof 4: `@[implemented_by]` / `@[extern]` substitutes unverified runtime execution
    let extern_report = analyze_lean_tcb(
        "@[extern \"c_sha512\"]\ndef sha512 (msg : ByteArray) : ByteArray := msg\n",
    );
    assert!(!extern_report.is_sound_tcb());
    assert_eq!(extern_report.declarations[0].role, LeanTcbRole::ExternHook);

    // Disproof 5: `sorry` inside string literal or comment MUST NOT trigger a false positive
    let literal_only = analyze_lean_tcb(
        "-- sorry admit native_decide\ndef msg : String := \"sorry native_decide\"\n",
    );
    assert!(
        literal_only.is_sound_tcb(),
        "tokens inside comments or string literals must not trigger TCB holes"
    );

    // Disproof 6: unclosed block comment fails closed
    let unclosed = analyze_lean_tcb("theorem t : True := trivial\n/- unclosed comment sorry");
    assert!(!unclosed.is_sound_tcb());
    assert!(unclosed
        .holes
        .iter()
        .any(|h| h.kind == LeanTcbHoleKind::UnclosedLexicalConstruct));

    // Disproof 7: indented `axiom`, `@[extern]`, and `unsafe` inside a `namespace`/`section`
    // (after an earlier benign declaration) MUST be extracted and fail `is_sound_tcb()`.
    let indented_ns_axiom = "\
namespace Signal.DoubleRatchet
  theorem step_ok : True := trivial
  axiom forged_false : False
end Signal.DoubleRatchet
";
    let ns_ax_rep = analyze_lean_tcb(indented_ns_axiom);
    assert!(
        !ns_ax_rep.is_sound_tcb(),
        "indented axiom inside namespace must fail TCB audit"
    );
    assert!(ns_ax_rep.declarations.iter().any(|d| {
        d.qualified_name == "Signal.DoubleRatchet.forged_false"
            && d.role == LeanTcbRole::TrustedAxiom
    }));

    let indented_ns_extern = "\
namespace Signal.DoubleRatchet
  def benign : Nat := 1
  @[scoped extern \"evil_c_shim\"]
  def backdoor : Nat := 1
end Signal.DoubleRatchet
";
    let ns_ext_rep = analyze_lean_tcb(indented_ns_extern);
    assert!(
        !ns_ext_rep.is_sound_tcb(),
        "indented @[extern] inside namespace must fail TCB audit"
    );
    assert!(ns_ext_rep.declarations.iter().any(|d| {
        d.qualified_name == "Signal.DoubleRatchet.backdoor" && d.role == LeanTcbRole::ExternHook
    }));

    let indented_ns_unsafe = "\
section Inner
  theorem ok : True := trivial
  unsafe def unsoundCast : Nat := 0
end Inner
";
    let ns_uns_rep = analyze_lean_tcb(indented_ns_unsafe);
    assert!(!ns_uns_rep.is_sound_tcb());
    assert!(ns_uns_rep
        .holes
        .iter()
        .any(|h| h.kind == LeanTcbHoleKind::UnsafeDecl));

    // Disproof 8: unclosed guillemet identifier `«` must fail closed instead of swallowing
    // subsequent `sorry` / `axiom` tokens to EOF.
    let unclosed_guillemet = "def sneaky «unclosed\ntheorem fake : False := by sorry\n";
    let unclosed_g_rep = analyze_lean_tcb(unclosed_guillemet);
    assert!(
        !unclosed_g_rep.is_sound_tcb(),
        "unclosed « must fail closed with UnclosedLexicalConstruct"
    );
    assert!(unclosed_g_rep
        .holes
        .iter()
        .any(|h| h.kind == LeanTcbHoleKind::UnclosedLexicalConstruct));

    // Disproof 9: guillemet-escaped `«sorry»`, `«admit»`, `«native_decide»`, plus kernel
    // `sorryAx` and `Lean.trustCompiler` must all fail `is_sound_tcb()`.
    for (label, snippet, expected_kind) in [
        (
            "guillemet sorry",
            "theorem p : False := «sorry»\n",
            LeanTcbHoleKind::Sorry,
        ),
        (
            "kernel sorryAx",
            "theorem p : False := sorryAx False false\n",
            LeanTcbHoleKind::Sorry,
        ),
        (
            "guillemet admit",
            "theorem p : False := by «admit»\n",
            LeanTcbHoleKind::Admit,
        ),
        (
            "Lean.trustCompiler",
            "theorem p : True := Lean.trustCompiler True\n",
            LeanTcbHoleKind::NativeDecide,
        ),
        (
            "Lean.ofReduceBool",
            "theorem p : True := Lean.ofReduceBool true true rfl\n",
            LeanTcbHoleKind::NativeDecide,
        ),
        (
            "unsafe example",
            "unsafe example : Nat := 0\n",
            LeanTcbHoleKind::UnsafeDecl,
        ),
        (
            "unclosed string literal",
            "def broken : String := \"unclosed sorry\n",
            LeanTcbHoleKind::UnclosedLexicalConstruct,
        ),
    ] {
        let rep = analyze_lean_tcb(snippet);
        assert!(
            !rep.is_sound_tcb(),
            "{label}: expected TCB failure on {snippet:?}"
        );
        assert!(
            rep.holes.iter().any(|h| h.kind == expected_kind),
            "{label}: expected {expected_kind:?}, got {:?}",
            rep.holes
        );
    }

    // Disproof 10: `opaque`, `@[csimp]`, and standalone `attribute [extern ...]`
    for (label, snippet, expected_role) in [
        (
            "opaque witness",
            "opaque hiddenWitness : Nat\n",
            LeanTcbRole::TrustedAxiom,
        ),
        (
            "@[csimp] runtime rewrite",
            "@[csimp] theorem fast_subst : id = id := rfl\n",
            LeanTcbRole::ExternHook,
        ),
        (
            "standalone attribute [implemented_by]",
            "def specFn (x : Nat) : Nat := x\nattribute [implemented_by fastFn] specFn\n",
            LeanTcbRole::ExternHook,
        ),
        (
            "where-clause local definition with @[extern]",
            "def outer : Nat := localEvil\nwhere\n  @[extern \"c_evil\"] localEvil : Nat := 0\n",
            LeanTcbRole::ExternHook,
        ),
        (
            "qualified @[_root_.extern] hook",
            "@[_root_.extern \"c_evil\"] def rootHook : Nat := 0\n",
            LeanTcbRole::ExternHook,
        ),
    ] {
        let rep = analyze_lean_tcb(snippet);
        assert!(!rep.is_sound_tcb(), "{label}: expected TCB failure");
        assert!(
            rep.declarations.iter().any(|d| d.role == expected_role),
            "{label}: expected {expected_role:?}, got {:?}",
            rep.declarations
        );
    }

    // Disproof 11: Lean 4 raw string literals (`r"\"`, `r#"\"#`) and multi-byte / hex-escaped
    // char literals (`'«'`, `'\x22'`) must NOT desynchronize lexical scanning and hide `sorry`!
    for (label, snippet, expected_kind) in [
        (
            "raw string literal ending with backslash before sorry",
            "def path : String := r#\"C:\\\"#\ntheorem pwn : False := sorry\ndef pad : String := \"\"\n",
            LeanTcbHoleKind::Sorry,
        ),
        (
            "zero-hash raw string ending with backslash before sorry",
            "def path : String := r\"\\\"\ntheorem pwn : False := sorry\ndef pad : String := \"\"\n",
            LeanTcbHoleKind::Sorry,
        ),
        (
            "unclosed raw string literal",
            "def broken : String := r#\"unclosed \"#\" sorry\n",
            LeanTcbHoleKind::UnclosedLexicalConstruct,
        ),
        (
            "multi-byte guillemet char literal before sorry",
            "def openCh : Char := '«'\ntheorem pwn : False := sorry\ndef closeCh : Char := '»'\n",
            LeanTcbHoleKind::Sorry,
        ),
        (
            "hex-escaped quote char literal before sorry",
            "def quoteCh : Char := '\\x22'\ntheorem pwn : False := sorry\ndef quoteCh2 : Char := '\\x22'\n",
            LeanTcbHoleKind::Sorry,
        ),
    ] {
        let rep = analyze_lean_tcb(snippet);
        assert!(
            !rep.is_sound_tcb(),
            "{label}: expected TCB failure on {snippet:?}"
        );
        assert!(
            rep.holes.iter().any(|h| h.kind == expected_kind),
            "{label}: expected {expected_kind:?}, got {:?}",
            rep.holes
        );
    }

    // Clean controls for raw strings (containing `"sorry"`) and multi-byte UTF-8 char literals:
    let clean_raw_and_utf8_chars = "\
def rawSorry : String := r#\"literal \\\" sorry axiom\"#
def greekChar : Char := 'α'
def unicodeEscChar : Char := '\\u03b1'
theorem stillSound : 1 = 1 := rfl
";
    let clean_rep = analyze_lean_tcb(clean_raw_and_utf8_chars);
    assert!(
        clean_rep.is_sound_tcb(),
        "valid raw string + UTF-8 char literals must pass sound TCB, got holes={:?} decls={:?}",
        clean_rep.holes,
        clean_rep.declarations
    );
}

#[test]
fn committed_workspace_lean_specs_satisfy_sound_tcb() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // DevCouncil lives at <Code>/devtools/DevCouncil; sibling specs live at
    // <Code>/devtools/gocap/specs/FeatherweightGoSree.lean and
    // <Code>/apps/Graft/specs/GraftProtocol.lean.
    let code_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("resolve workspace parent above rust/devmap-extract");

    let specs = [
        code_root.join("devtools/gocap/specs/FeatherweightGoSree.lean"),
        code_root.join("apps/Graft/specs/GraftProtocol.lean"),
    ];
    let mut checked = 0usize;
    for spec_path in &specs {
        if !spec_path.exists() {
            continue;
        }
        let src = std::fs::read_to_string(spec_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", spec_path.display()));
        let report = analyze_lean_tcb(&src);
        assert!(
            report.is_sound_tcb(),
            "formal spec {} failed TCB soundness check: unread_bytes={}, holes={:?}, decls={:?}",
            spec_path.display(),
            report.unread_bytes,
            report.holes,
            report
                .declarations
                .iter()
                .filter(|d| matches!(d.role, LeanTcbRole::TrustedAxiom | LeanTcbRole::ExternHook))
                .collect::<Vec<_>>()
        );
        assert!(
            !report.declarations.is_empty(),
            "expected extracted declarations in {}",
            spec_path.display()
        );
        assert!(
            report
                .declarations
                .iter()
                .any(|d| d.role == LeanTcbRole::VerifiedSpec),
            "expected at least one kernel-checked theorem/lemma in {}",
            spec_path.display()
        );
        checked += 1;
    }
    if code_root.join("devtools/gocap").is_dir() && code_root.join("apps/Graft").is_dir() {
        assert_eq!(
            checked, 2,
            "both sibling formal specs (FeatherweightGoSree.lean and GraftProtocol.lean) must be verified"
        );
    }
}

#[test]
fn metaprogramming_escapes_are_convicted_without_false_positives_on_clean_specs() {
    let metaprogramming_attacks: &[(&str, &str)] = &[
        (
            "elab tactic synthesizing sorryAx without spelling sorry",
            "elab \"magic_proof\" : tactic => do pure ()\ntheorem fake : False := by magic_proof\n",
        ),
        (
            "elab_rules term",
            "elab_rules : term | `(boom) => pure ()\n",
        ),
        (
            "macro command",
            "macro \"cheat\" : tactic => `(tactic| trivial)\n",
        ),
        (
            "macro_rules",
            "macro_rules | `(tactic| cheat) => `(tactic| trivial)\n",
        ),
        ("syntax extension", "syntax \"forge_ax\" : command\n"),
        ("notation override", "notation \"⊥!\" => True\n"),
        ("prefix operator", "prefix:100 \"!!\" => id\n"),
        ("infix operator", "infix:50 \" <~> \" => Eq\n"),
        ("infixl operator", "infixl:65 \" +++ \" => Nat.add\n"),
        ("infixr operator", "infixr:65 \" *** \" => Nat.mul\n"),
        ("postfix operator", "postfix:max \"!\" => id\n"),
        ("run_cmd elaboration side effect", "run_cmd pure ()\n"),
        (
            "by_elab inside proof body",
            "theorem sneaky : True := by_elab pure ()\n",
        ),
        ("#eval side-effect command", "#eval (1 + 1)\n"),
        ("#eval! unsafe command", "#eval! IO.println \"hi\"\n"),
        ("spaced # eval command", "# eval (1 + 1)\n"),
        ("initialize IO hook", "initialize pure ()\n"),
        ("builtin_initialize IO hook", "builtin_initialize pure ()\n"),
        (
            "@[macro] attribute",
            "@[macro myStx]\ndef expandMyStx : Nat := 0\n",
        ),
        (
            "@[command_elab] attribute",
            "@[command_elab myCmd]\ndef elabMyCmd : Nat := 0\n",
        ),
        (
            "@[term_elab] attribute",
            "@[scoped term_elab myTerm]\ndef elabMyTerm : Nat := 0\n",
        ),
        (
            "@[tactic] attribute",
            "@[local tactic myTac]\ndef elabMyTac : Nat := 0\n",
        ),
        (
            "@[elab_without_expected_type] attribute",
            "@[elab_without_expected_type]\ndef elabUnchecked : Nat := 0\n",
        ),
        (
            "standalone attribute [tactic]",
            "def myTac : Nat := 0\nattribute [tactic foo] myTac\n",
        ),
        (
            "guillemet-escaped «elab» and @[«macro»]",
            "@[«macro» foo]\ndef «elab_helper» : Nat := «elab»\n",
        ),
    ];

    for (label, snippet) in metaprogramming_attacks {
        let rep = analyze_lean_tcb(snippet);
        assert!(
            !rep.is_sound_tcb(),
            "{label}: metaprogramming escape must fail closed, got holes={:?} decls={:?}",
            rep.holes,
            rep.declarations
        );
        assert!(
            rep.holes
                .iter()
                .any(|h| h.kind == LeanTcbHoleKind::MetaprogrammingEscape),
            "{label}: expected MetaprogrammingEscape hole, got holes={:?}",
            rep.holes
        );
    }

    // Identifiers that merely contain `prefix`/`macro`/`syntax` as substrings (e.g. `isPrefixOf`,
    // `macroStep`, `syntaxTree`) or mention `#eval`/`elab` inside comments/strings must stay sound!
    let clean_substring_names = "\
-- #eval elab macro syntax run_cmd @[tactic]
/- elab_rules macro_rules builtin_initialize -/
namespace CleanSubstrings
def isPrefixOf (x : Nat) : Nat := x
def macroStep (syntaxTree : Nat) : Nat := isPrefixOf syntaxTree
def msg : String := \"#eval elab macro @[command_elab]\"
theorem step_eq (n : Nat) : macroStep n = n := rfl
end CleanSubstrings
";
    let clean_rep = analyze_lean_tcb(clean_substring_names);
    assert!(
        clean_rep.is_sound_tcb(),
        "clean identifiers with prefix/macro/syntax substrings must remain sound, got holes={:?}",
        clean_rep.holes
    );
}

#[test]
fn lean_extraction_preserves_namespace_across_mutual_end_block() {
    let src = "\
namespace FeatherweightGoSree
mutual
  def stepA (n : Nat) : Nat := stepB n
  def stepB (n : Nat) : Nat := n
end
theorem preservation (n : Nat) : stepB n = n := rfl
end FeatherweightGoSree
";
    let ext = extract_file("specs/FeatherweightGoSree.lean", src);
    let pres = ext
        .symbols
        .iter()
        .find(|s| s.name == "preservation")
        .expect("preservation symbol extracted");
    assert_eq!(pres.qualified_name, "FeatherweightGoSree.preservation");
    assert_eq!(
        pres.parent_symbol.as_deref(),
        Some("FeatherweightGoSree")
    );
}
