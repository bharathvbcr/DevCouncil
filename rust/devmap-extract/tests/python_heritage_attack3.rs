//! Round-three adversarial probes of the Python heritage extraction: the
//! Python-only emission-order tie-break in `treesitter`, the segment order of a
//! dotted base, and the self-reference skip. A failing test is a BREAK.

use devmap_extract::extract_file;
use devmap_extract::model::{ExtractedReference, ReferenceKind};

fn heritage(path: &str, source: &str) -> Vec<(String, usize, usize)> {
    extract_file(path, source)
        .references
        .iter()
        .filter(|r| matches!(r.kind, ReferenceKind::Heritage | ReferenceKind::HeritageInterface))
        .map(|r| (r.name.clone(), r.span.start_byte, r.span.end_byte))
        .collect()
}

fn names(path: &str, source: &str) -> Vec<String> {
    heritage(path, source).into_iter().map(|(n, _, _)| n).collect()
}

fn key(r: &ExtractedReference) -> (usize, usize, String) {
    (r.span.start_byte, r.span.end_byte, r.name.clone())
}

/// Dotted heritage per non-Python language. The references of every one of
/// them must still be ordered `(start, end, name)`: the base sort, which the
/// Python tie-break must not have touched.
const NON_PYTHON: &[(&str, &str)] = &[
    ("w.ts", "class A extends a.b.C implements p.q.I {}\nclass B extends Zed.mid.base {}\n"),
    ("w.tsx", "class A extends React.Component {}\nclass B extends Zed.mid.base {}\n"),
    ("W.java", "class A extends a.b.C implements p.q.I, z.y.J {}\nclass B extends Zed.mid.base {}\n"),
    ("W.kt", "class A : a.b.C(), p.q.I\nclass B : Zed.mid.base()\n"),
    ("W.cs", "class A : a.b.C, N.M.I {}\nclass B : Zed.mid.base {}\n"),
    ("w.rb", "class A < Zed::mid::Base\nend\nclass B < A::B::C\nend\n"),
    ("W.scala", "class A extends a.b.C with p.q.T\nclass B extends Zed.mid.base\n"),
    ("w.swift", "class A: a.b.C, p.Proto {}\nclass B: Zed.mid.base {}\n"),
    ("w.php", "<?php\nclass A extends \\a\\b\\C implements \\p\\I {}\n"),
    ("w.cpp", "class A : public a::b::C, private z::D {};\n"),
    ("w.dart", "class A extends a.C with M implements I {}\n"),
    ("w.rs", "struct S;\nimpl a::b::T for S {}\n"),
    ("w.go", "package w\ntype A struct {\n    pkg.Base\n    *other.Ptr\n}\n"),
    ("w.sol", "contract A is a.B, C {}\n"),
];

#[test]
fn non_python_references_keep_the_start_end_name_sort() {
    for (path, source) in NON_PYTHON {
        let refs = extract_file(path, source).references;
        let keys: Vec<_> = refs.iter().map(key).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "{path}: references are no longer name-sorted on a tie");
    }
}

#[test]
fn non_python_dotted_heritage_is_still_innermost_first_sorted() {
    // `a.b.C` over one span: the old order is alphabetical, whatever the
    // spelling. Pin the TS and Java shapes the way the base binary emits them.
    let ts = names("w.ts", "class A extends Zed.mid.base {}\n");
    let mut expected = ts.clone();
    expected.sort();
    assert_eq!(ts, expected, "ts: {ts:?}");
    let java = names("W.java", "class A extends Zed.mid.base {}\n");
    let mut expected = java.clone();
    expected.sort();
    assert_eq!(java, expected, "java: {java:?}");
}

#[test]
fn python_non_heritage_references_stay_name_sorted_on_a_tie() {
    let refs = extract_file(
        "w.py",
        "import a\nclass C(a.Base):\n    x = a.Base\n    y = a.Base\n",
    )
    .references;
    let non: Vec<_> = refs
        .iter()
        .filter(|r| r.kind != ReferenceKind::Heritage)
        .map(key)
        .collect();
    let mut sorted = non.clone();
    sorted.sort();
    assert_eq!(non, sorted);
}

#[test]
fn python_heritage_does_not_interleave_with_same_span_non_heritage() {
    // Heritage rows of one base are contiguous: the resolver folds
    // consecutive rows of one span into one dotted base.
    let refs = extract_file("w.py", "import a\nclass C(a.b.Base):\n    pass\n").references;
    let positions: Vec<usize> = refs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == ReferenceKind::Heritage)
        .map(|(i, _)| i)
        .collect();
    assert!(
        positions.windows(2).all(|w| w[1] == w[0] + 1),
        "heritage rows are split by another reference: {refs:#?}"
    );
}

#[test]
fn python_extraction_is_deterministic_across_runs_and_threads() {
    let src = "import a\nclass C(a.b.D, x.y.E, *m, make(), (z.F), G[int]):\n    pass\nclass K(Zed.mid.base):\n    pass\n";
    let first = format!("{:?}", extract_file("w.py", src).references);
    for _ in 0..50 {
        assert_eq!(first, format!("{:?}", extract_file("w.py", src).references));
    }
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let src = src.to_string();
            std::thread::spawn(move || format!("{:?}", extract_file("w.py", &src).references))
        })
        .collect();
    for h in handles {
        assert_eq!(first, h.join().unwrap());
    }
}

#[test]
fn a_dotted_base_whose_member_is_the_class_name_keeps_every_segment() {
    // `class Model(models.Model)` / `class Function(torch.autograd.Function)`:
    // the member shares the declarer's name. The self-reference skip must not
    // delete the member (or a qualifier segment) of a dotted base, or the
    // resolver reads `torch.autograd` / `models` as the base.
    assert_eq!(names("w.py", "class Model(models.Model):\n    pass\n"), ["Model", "models"]);
    assert_eq!(
        names("w.py", "class Function(torch.autograd.Function):\n    pass\n"),
        ["Function", "torch", "autograd"]
    );
}

#[test]
fn a_dotted_base_with_a_qualifier_named_like_the_class_keeps_every_segment() {
    assert_eq!(
        names("w.py", "class b(a.b.C):\n    pass\n"),
        ["C", "a", "b"]
    );
    assert_eq!(
        names("w.py", "class pkg(pkg.mod.Base):\n    pass\n"),
        ["Base", "pkg", "mod"]
    );
}

#[test]
fn comment_inside_a_dotted_base_is_still_a_dotted_base() {
    let n = names("w.py", "class C(\n    a  # the module\n    .Base\n):\n    pass\n");
    assert_eq!(n, ["Base", "a"], "{n:?}");
}

#[test]
fn backslash_continued_dotted_base_is_still_a_dotted_base() {
    let n = names("w.py", "class C(a \\\n    .Base):\n    pass\n");
    assert_eq!(n, ["Base", "a"], "{n:?}");
}

#[test]
fn opaque_marker_is_never_a_name_in_a_non_python_language() {
    for (path, source) in NON_PYTHON {
        for r in extract_file(path, source).references {
            assert_ne!(r.name, devmap_extract::model::OPAQUE_BASE, "{path}");
        }
    }
}

#[test]
fn a_class_named_in_a_string_base_or_none_is_opaque_not_a_name() {
    for head in ["class C(None):", "class C(\"Base\"):", "class C(lambda: 1):", "class C((a, b)):"] {
        let n = names("w.py", &format!("{head}\n    pass\n"));
        assert_eq!(n, [devmap_extract::model::OPAQUE_BASE], "{head}: {n:?}");
    }
}

#[test]
fn a_subscript_of_a_call_or_index_is_not_a_named_base() {
    // `class C(bases[0])` is not a class named `bases`.
    let n = names("w.py", "class C(bases[0]):\n    pass\n");
    assert_eq!(n, [devmap_extract::model::OPAQUE_BASE], "{n:?}");
}
