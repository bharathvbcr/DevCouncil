//! A common name narrowed by path, language and kind.
//!
//! `record` is defined in two directories and two languages. Each filter has
//! to select exactly the definitions that pass it, and `total` has to count
//! that set rather than the unfiltered namesakes.

use devmap_extract::extract_file;
use devmap_query::{NameQueryFilter, Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-name-filter-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn filter(paths: &[&str], languages: &[&str], kinds: &[&str]) -> NameQueryFilter {
    let owned = |values: &[&str]| values.iter().map(|value| (*value).to_string()).collect::<Vec<_>>();
    NameQueryFilter::new(&owned(paths), &owned(languages), &owned(kinds))
        .unwrap()
        .expect("a non-empty filter")
}

fn names(engine: &StoreQueryEngine, query: &str, narrowing: Option<&NameQueryFilter>) -> (u32, BTreeSet<(String, String)>) {
    let response = engine
        .search_filtered(
            Request {
                query: query.to_string(),
                token_budget: 8_000,
                min_confidence: 0.0,
                max_depth: 1,
            },
            narrowing,
        )
        .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        response.shown + response.hidden,
        response.total,
        "shown + hidden must be the filtered total"
    );
    if narrowing.is_some() {
        let scope = response.scope.as_ref().expect("a filter is echoed");
        let expected = narrowing.unwrap();
        assert_eq!(scope.paths, expected.paths());
        assert_eq!(scope.languages, expected.languages());
        assert_eq!(scope.kinds, expected.kinds());
    } else {
        assert!(response.scope.is_none(), "an unfiltered search echoes no scope");
    }
    let found = response
        .items
        .iter()
        .map(|hit| (hit.file_path.clone(), format!("{}:{}", hit.kind, hit.symbol_name)))
        .collect();
    (response.total, found)
}

#[test]
fn each_filter_selects_exactly_the_namesakes_that_pass_it() {
    let root = scratch("namesakes");
    std::fs::create_dir_all(root.join("left")).unwrap();
    std::fs::create_dir_all(root.join("right")).unwrap();
    let files = [
        ("left/a.py", "def record():\n    return 1\n"),
        ("right/b.py", "def record():\n    return 2\n"),
        ("left/a.go", "package left\n\nfunc record() int { return 1 }\n"),
        (
            "right/b.go",
            "package right\n\ntype record struct { n int }\n\nfunc record() int { return 2 }\n",
        ),
    ];
    let mut extractions = Vec::new();
    for (path, body) in files {
        std::fs::write(root.join(path), body).unwrap();
        extractions.push(extract_file(path, body));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    let engine = StoreQueryEngine::new(&store);

    let (all_total, all) = names(&engine, "record", None);
    let expect_all = BTreeSet::from([
        ("left/a.py".to_string(), "Function:record".to_string()),
        ("right/b.py".to_string(), "Function:record".to_string()),
        ("left/a.go".to_string(), "Function:record".to_string()),
        ("right/b.go".to_string(), "Function:record".to_string()),
        ("right/b.go".to_string(), "Struct:record".to_string()),
    ]);
    assert_eq!(all, expect_all, "unfiltered namesakes");
    assert_eq!(all_total, 5);

    let left = filter(&["left"], &[], &[]);
    let (left_total, left_found) = names(&engine, "record", Some(&left));
    assert_eq!(
        left_found,
        BTreeSet::from([
            ("left/a.py".to_string(), "Function:record".to_string()),
            ("left/a.go".to_string(), "Function:record".to_string()),
        ])
    );
    assert_eq!(left_total, 2);

    let python = filter(&[], &["python"], &[]);
    let (py_total, py_found) = names(&engine, "record", Some(&python));
    assert_eq!(
        py_found,
        BTreeSet::from([
            ("left/a.py".to_string(), "Function:record".to_string()),
            ("right/b.py".to_string(), "Function:record".to_string()),
        ])
    );
    assert_eq!(py_total, 2);

    let structs = filter(&[], &[], &["Struct"]);
    let (struct_total, struct_found) = names(&engine, "record", Some(&structs));
    assert_eq!(
        struct_found,
        BTreeSet::from([("right/b.go".to_string(), "Struct:record".to_string())])
    );
    assert_eq!(struct_total, 1);
    assert_eq!(structs.kinds(), &["Struct".to_string()]);

    let precise = filter(&["left"], &["go"], &["function"]);
    assert_eq!(precise.kinds(), &["Function".to_string()]);
    let (precise_total, precise_found) = names(&engine, "record", Some(&precise));
    assert_eq!(
        precise_found,
        BTreeSet::from([("left/a.go".to_string(), "Function:record".to_string())])
    );
    assert_eq!(precise_total, 1);
    assert_eq!(
        engine
            .search_filtered(
                Request {
                    query: "record".to_string(),
                    token_budget: 8_000,
                    min_confidence: 0.0,
                    max_depth: 1,
                },
                Some(&precise),
            )
            .unwrap()
            .scope
            .unwrap()
            .kinds,
        vec!["Function".to_string()]
    );

    let starved = engine
        .search_filtered(
            Request {
                query: "record".to_string(),
                token_budget: 1,
                min_confidence: 0.0,
                max_depth: 1,
            },
            Some(&left),
        )
        .unwrap();
    assert_eq!(starved.total, 2, "a small budget still counts the filtered set");
    assert_eq!(starved.shown + starved.hidden, starved.total);
    assert!(starved.hidden > 0, "budget 1 cannot show both definitions");

    let explore = engine
        .explore_filtered("record", 5, 8_000, 0.0, 1, Some(&precise))
        .unwrap();
    assert_eq!(explore.definitions.total, 1);
    assert_eq!(explore.definitions.shown + explore.definitions.hidden, 1);
    assert_eq!(explore.definitions.items[0].file_path, "left/a.go");
    let scope = explore.scope.expect("explore echoes the filter");
    assert_eq!(scope.paths, vec!["left".to_string()]);
    assert_eq!(scope.languages, vec!["go".to_string()]);
    assert_eq!(scope.kinds, vec!["Function".to_string()]);

    let unknown = NameQueryFilter::new(&[], &[], &["Widget".to_string()]).unwrap_err();
    assert!(
        unknown.to_string().contains("not a symbol kind"),
        "{unknown}"
    );
    let missing = filter(&["missing"], &[], &[]);
    let refused = engine
        .search_filtered(
            Request {
                query: "record".to_string(),
                token_budget: 8_000,
                min_confidence: 0.0,
                max_depth: 1,
            },
            Some(&missing),
        )
        .unwrap_err();
    assert!(
        refused.to_string().contains("matches no indexed file"),
        "{refused}"
    );
}
