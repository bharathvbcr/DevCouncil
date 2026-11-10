//! `CStyle` was one resolution family for C, C++, Objective-C, CUDA, Java and
//! C#. Java and C# are not link-compatible with the C-like group or with each
//! other, so a bare call in one bound to a same-named declaration in another:
//! a C# method `Direct` called `only_c` and bound to `c/lib.c::only_c` at 0.9,
//! with no `DllImport` involved. The families are now three:
//!
//! * `CStyle` (the identifier is kept, so ledger text `family CStyle` and
//!   serialized rows stay byte for byte): c, cpp, objc, cuda (metal rides the
//!   `cpp` grammar key);
//! * `Java`;
//! * `CSharp`.
//!
//! Everything below is written against `LangFamily::from_lang(..)` values, not
//! variant names, so the file compiles on the pre-split tree and fails there on
//! an assertion rather than on a missing variant.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::builtins::{host_global_environment, is_builtin};
use devmap_resolve::model::{ResolutionResult, UnresolvedClass};
use devmap_resolve::{LangFamily, Resolution, Resolver};

fn resolve(all: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = all
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// Calls edges leaving `from_file`, as `(source_symbol, target_file)`.
fn call_targets(result: &ResolutionResult, from_file: &str) -> Vec<(String, String)> {
    result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_file == from_file)
        .map(|e| (e.source_symbol.clone(), e.target_file.clone()))
        .collect()
}

fn calls(result: &ResolutionResult, from_file: &str, to_file: &str) -> bool {
    call_targets(result, from_file)
        .iter()
        .any(|(_, target)| target == to_file)
}

fn no_call_edge_out_of(result: &ResolutionResult, from_file: &str) {
    let out = call_targets(result, from_file);
    assert!(out.is_empty(), "{from_file} must bind nothing: {out:?}");
}

const C_LIB: &str = "int only_c(int x) { return x; }\nint shared_name(int x) { return x; }\n";
const JAVA_SHARED: &str = "class Sh { static int shared_name(int x) { return x; } }\n";
const CS_SHARED: &str = "class Sh2 { static int shared_name(int x) { return x; } }\n";
const JAVA_ONLY: &str = "class J { static int only_java(int x) { return x; } }\n";
const CS_ONLY: &str = "class D { static int only_cs(int x) { return x; } }\n";

// ---- (a) the added edge: a call that was ambiguous only across groups --------

#[test]
fn a_c_call_with_java_and_csharp_namesakes_binds_the_c_one() {
    let result = resolve(&[
        ("c/ns.c", "int c_uses_shared(int v) { return shared_name(v); }\n"),
        ("c/lib.c", C_LIB),
        ("java/Sh.java", JAVA_SHARED),
        ("cs/Sh.cs", CS_SHARED),
    ]);
    assert!(
        calls(&result, "c/ns.c", "c/lib.c"),
        "the C call must reach the one C declaration: {:?}",
        call_targets(&result, "c/ns.c")
    );
    assert!(!calls(&result, "c/ns.c", "java/Sh.java"));
    assert!(!calls(&result, "c/ns.c", "cs/Sh.cs"));
    let ambiguous = result.unresolved.iter().any(|u| {
        u.source_file == "c/ns.c" && matches!(u.resolution, Resolution::AmbiguousGlobal { .. })
    });
    assert!(!ambiguous, "no ambiguity remains inside the C-like group");
}

// ---- (b) Java <-> C# ---------------------------------------------------------

#[test]
fn java_and_csharp_do_not_bind_each_other_by_bare_name() {
    let result = resolve(&[
        ("java/Use.java", "class U { int go() { return only_cs(1); } }\n"),
        ("cs/D.cs", CS_ONLY),
        ("cs/Use.cs", "class V { static int go() { return only_java(1); } }\n"),
        ("java/J.java", JAVA_ONLY),
    ]);
    no_call_edge_out_of(&result, "java/Use.java");
    no_call_edge_out_of(&result, "cs/Use.cs");
}

// ---- (c) Java / C# -> C, and C-like -> Java / C# -----------------------------

#[test]
fn java_and_csharp_bare_calls_do_not_bind_a_c_function() {
    let result = resolve(&[
        ("c/lib.c", C_LIB),
        ("java/Cls.java", "class Cls { void h() { only_c(1); } }\n"),
        ("cs/Native.cs", "class Native { static int Direct() { return only_c(2); } }\n"),
    ]);
    no_call_edge_out_of(&result, "java/Cls.java");
    no_call_edge_out_of(&result, "cs/Native.cs");
}

#[test]
fn a_c_call_does_not_bind_a_java_or_csharp_function() {
    let result = resolve(&[
        ("java/J.java", JAVA_ONLY),
        ("cs/D.cs", CS_ONLY),
        (
            "c/main.c",
            "int a(int v) { return only_java(v); }\nint b(int v) { return only_cs(v); }\n",
        ),
    ]);
    no_call_edge_out_of(&result, "c/main.c");
}

// ---- positive controls: the C-like group stays one family --------------------

#[test]
fn the_c_like_group_still_binds_across_its_languages() {
    let result = resolve(&[
        ("c/lib.c", C_LIB),
        ("cpp/b.cpp", "int cpp_only(int a) { return a; }\n"),
        ("cu/k.cu", "int cu_only(int a) { return a; }\nint cu_calls_c(int a) { return only_c(a); }\n"),
        ("mm/o.mm", "int mm_only(int a) { return a; }\nint mm_calls_c(int a) { return only_c(a); }\n"),
        ("c/user.c", "int u1(int a) { return cpp_only(a); }\nint u2(int a) { return mm_only(a); }\n"),
        ("cpp/c.cpp", "int cpp_calls_cu(int a) { return cu_only(a); }\n"),
    ]);
    let user: Vec<String> = call_targets(&result, "c/user.c")
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert!(user.contains(&"cpp/b.cpp".to_string()), "C -> C++: {user:?}");
    assert!(user.contains(&"mm/o.mm".to_string()), "C -> ObjC++: {user:?}");
    assert!(calls(&result, "cpp/c.cpp", "cu/k.cu"), "C++ -> CUDA");
    assert!(calls(&result, "cu/k.cu", "c/lib.c"), "CUDA -> C");
    assert!(calls(&result, "mm/o.mm", "c/lib.c"), ".mm -> C");
}

#[test]
fn java_and_csharp_still_bind_within_their_own_language() {
    let result = resolve(&[
        ("j/A.java", "class A { int go() { return helper(1); } }\n"),
        ("j/B.java", "class B { static int helper(int x) { return x; } }\n"),
        ("c/A.cs", "class CA { static int go() { return chelper(1); } }\n"),
        ("c/B.cs", "class CB { static int chelper(int x) { return x; } }\n"),
    ]);
    assert!(calls(&result, "j/A.java", "j/B.java"), "Java -> Java");
    assert!(calls(&result, "c/A.cs", "c/B.cs"), "C# -> C#");
}

#[test]
fn kotlin_still_binds_java_through_the_jvm_bridge() {
    let result = resolve(&[
        (
            "jvm/com/fx/App.kt",
            "package com.fx\nimport com.fx.LegacyMath\nfun run(): Int { return LegacyMath.twice(2) }\n",
        ),
        (
            "jvm/com/fx/LegacyMath.java",
            "package com.fx;\npublic class LegacyMath { public static int twice(int x) { return x * 2; } }\n",
        ),
        (
            "jvm/com/fx/Reg.kt",
            "package com.fx\nobject Registry { fun register(n: Int): Int { return n } }\n",
        ),
        (
            "jvm/com/fx/Use.java",
            "package com.fx;\npublic class Use { void go() { Registry.INSTANCE.register(1); } }\n",
        ),
    ]);
    assert!(calls(&result, "jvm/com/fx/App.kt", "jvm/com/fx/LegacyMath.java"), "Kotlin -> Java");
    assert!(calls(&result, "jvm/com/fx/Use.java", "jvm/com/fx/Reg.kt"), "Java -> Kotlin");
}

#[test]
fn an_extern_declaration_in_the_same_file_still_binds_same_file() {
    let result = resolve(&[
        ("c/lib.c", "int add_native(int a) { return a + 1; }\n"),
        (
            "cs/Native.cs",
            "using System.Runtime.InteropServices;\nclass Native {\n  [DllImport(\"lib\")] static extern int add_native(int a);\n  static int Run() { return add_native(1); }\n}\n",
        ),
        (
            "java/Cls.java",
            "class Cls { native void nativeAdd(int x); void go() { nativeAdd(1); } }\n",
        ),
    ]);
    assert_eq!(
        call_targets(&result, "cs/Native.cs"),
        vec![("cs/Native.cs::Native.Run".to_string(), "cs/Native.cs".to_string())]
    );
    assert_eq!(
        call_targets(&result, "java/Cls.java"),
        vec![("java/Cls.java::Cls.go".to_string(), "java/Cls.java".to_string())]
    );
}

// ---- the family table --------------------------------------------------------

#[test]
fn the_family_table_has_three_groups() {
    let c = LangFamily::from_lang("c");
    for lang in ["cpp", "objc", "cuda"] {
        assert_eq!(LangFamily::from_lang(lang), c, "{lang} is C-like");
    }
    let java = LangFamily::from_lang("java");
    let csharp = LangFamily::from_lang("csharp");
    assert_ne!(java, c);
    assert_ne!(csharp, c);
    assert_ne!(java, csharp);
    assert!(!java.admits(csharp) && !csharp.admits(java));
    assert!(!java.admits(c) && !c.admits(java));
    assert!(!csharp.admits(c) && !c.admits(csharp));
    for family in [c, java, csharp] {
        assert!(family.admits(family));
        assert_ne!(family, LangFamily::Generic);
        assert_ne!(family, LangFamily::Kotlin);
        assert_ne!(family, LangFamily::Scala);
    }
}

// ---- admission-only tripwires ------------------------------------------------

/// No builtin table was shared by `CStyle` (its `is_builtin` arm is
/// `return false`), so the split must keep every name a non-builtin for all
/// three families, whatever table the name belongs to elsewhere.
#[test]
fn the_split_families_keep_no_builtins_and_no_host_globals() {
    let families = [
        ("c", LangFamily::from_lang("c")),
        ("cpp", LangFamily::from_lang("cpp")),
        ("java", LangFamily::from_lang("java")),
        ("csharp", LangFamily::from_lang("csharp")),
    ];
    for (lang, family) in families {
        for name in [
            "len", "print", "println", "append", "Some", "echo", "printf", "malloc", "String",
            "Console", "System", "require", "range", "map", "min", "max", "panic", "assert",
        ] {
            assert!(!is_builtin(family, name), "{lang}: {name} must not be a builtin");
        }
        assert_eq!(host_global_environment(family, "setTimeout"), None, "{lang}");
        assert_eq!(host_global_environment(family, "require"), None, "{lang}");
    }
}

/// The java.lang rung is gated on the `.java` extension, not on the family; the
/// split must not move it.
#[test]
fn a_csharp_string_is_still_never_java_lang() {
    let result = resolve(&[(
        "n/Person.cs",
        "namespace N { public class Person { public String Name; } }\n",
    )]);
    assert!(result.unresolved.iter().all(|u| !matches!(
        &u.class,
        UnresolvedClass::External { module } if module.starts_with("java.")
    )));
}
