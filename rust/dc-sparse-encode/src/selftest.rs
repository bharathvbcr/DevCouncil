//! What the script's `--self-test` checked, without a model.
//!
//! The index half is `dc_grep::build_index` and `dc_grep::ranked_search`.
//! Those are the functions the `dcgrep index` and `dcgrep rank` commands
//! call; the binary adds a JSON line and nothing else. A stand-in that
//! reimplemented the schema would pass while the binary refused.

use std::collections::HashMap;
use std::path::Path;

use dc_grep::{IndexRequest, RankedRequest, build_index, ranked_search};

use crate::fileset::walk;
use crate::jsonl::{self, DocLine, Prepared};
use crate::vocab;

pub fn self_test() -> Result<(), String> {
    let mut failures = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok {
            println!("  ok    {name}");
        } else {
            println!("  FAIL  {name}: {detail}");
            failures.push(name.to_string());
        }
    };

    let ordered = [
        "[UNK]", "parse", "json", "server", "route", "handler", "##json", "gamma",
    ];
    let shuffled: HashMap<String, u32> = ordered
        .iter()
        .enumerate()
        .map(|(i, token)| ((*token).to_string(), i as u32))
        .collect();
    let dense = vocab::densify(&shuffled).unwrap_or_default();
    let restored: Vec<&str> = dense.iter().map(String::as_str).collect();
    check(
        "vocabulary() restores id order",
        restored == ordered,
        format!("{restored:?}"),
    );

    let holed = HashMap::from([("a".into(), 0u32), ("c".into(), 2)]);
    let holed = vocab::densify(&holed).unwrap_or_default();
    check(
        "a hole keeps later ids in place",
        holed.len() == 3 && holed[0] == "a" && holed[2] == "c" && holed[1] == "[unused_hole_1]",
        format!("{holed:?}"),
    );

    let specials = ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]"];
    let drop = crate::post::special_drop(&specials.map(str::to_string).to_vec());
    let drop_ok = drop.as_ref().ok().filter(|set| {
        set.len() == 4 && !set.contains(&1) && set.contains(&0) && set.contains(&2)
    });
    check(
        "special ids drop everything but [UNK]",
        drop_ok.is_some(),
        format!("{drop:?}"),
    );

    let room = std::env::temp_dir().join(format!(
        "dc-sparse-encode-walk-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(room.join("src")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(room.join("node_modules")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(room.join(".git")).map_err(|e| e.to_string())?;
    std::fs::write(room.join("src/a.rs"), "parse json\n").map_err(|e| e.to_string())?;
    std::fs::write(room.join("src/b.bin"), "binary-ish\n").map_err(|e| e.to_string())?;
    std::fs::write(room.join("node_modules/c.js"), "skip me\n").map_err(|e| e.to_string())?;
    std::fs::write(room.join(".git/d.py"), "skip me\n").map_err(|e| e.to_string())?;
    let link = room.join("src/link.rs");
    std::os::unix::fs::symlink(room.join("src/a.rs"), &link).map_err(|e| e.to_string())?;
    let found: std::collections::HashSet<String> = walk(&room, 200_000)?
        .iter()
        .map(|path| {
            path.strip_prefix(&room)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    check(
        "walk takes source and skips the rest",
        found.len() == 1 && found.contains("src/a.rs"),
        format!("{found:?}"),
    );
    let capped = walk(&room, 1)?;
    check("walk honours its own ceiling", capped.len() == 1, format!("{}", capped.len()));
    let _ = std::fs::remove_dir_all(&room);

    match end_to_end() {
        Ok(()) => {}
        Err(err) => check("dcgrep accepts the encoding", false, err),
    }

    println!();
    if failures.is_empty() {
        println!("self-test: passed (0 failures)");
        Ok(())
    } else {
        println!(
            "self-test: FAILED ({} failure{})",
            failures.len(),
            if failures.len() == 1 { "" } else { "s" }
        );
        Err(format!("self-test failed: {}", failures.join(", ")))
    }
}

fn end_to_end() -> Result<(), String> {
    let room = std::env::temp_dir().join(format!(
        "dc-sparse-encode-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let repo = room.join("repo");
    std::fs::create_dir_all(repo.join("src")).map_err(|e| e.to_string())?;
    std::fs::write(repo.join("src/alpha.rs"), "fn alpha() {}\n").map_err(|e| e.to_string())?;
    std::fs::write(repo.join("src/beta.rs"), "fn beta() {}\n").map_err(|e| e.to_string())?;
    let out = room.join("sparse.jsonl");

    let ordered = vec![
        "[UNK]".into(),
        "parse".into(),
        "json".into(),
        "server".into(),
        "route".into(),
        "handler".into(),
        "##json".into(),
        "gamma".into(),
    ];
    // What this build's WordPiece produces for the parity texts below, on
    // this vocabulary. Recorded the same way the script recorded them: if
    // the header said something else, the build would refuse.
    let known = [
        ("parse", vec![1u32]),
        ("json", vec![2]),
        ("parseJson", vec![1, 6]),
        ("nothingatall", vec![0]),
    ];
    let vocab = vocab::lookup(&ordered);
    let weights = vec![0.0, 2.0, 3.0, 1.0, 1.0, 1.0, 2.5, 1.0];
    let prepared = Prepared {
        model_id: "self-test",
        tokens: &ordered,
        weights: &weights,
        vocab: &vocab,
        max_positions: 512,
    };
    // The real header uses the full parity list. This self-test, like the
    // script, substitutes a list the fake vocabulary can actually split, or
    // the gate would refuse a header that was never going to match.
    let mut header = jsonl::header(&prepared)?;
    header["parity"] = serde_json::json!(
        known
            .iter()
            .map(|(text, ids)| serde_json::json!({"text": text, "ids": ids}))
            .collect::<Vec<_>>()
    );
    for (text, ids) in &known {
        let got = crate::tokenize::query_ids(text, &vocab);
        if got != *ids {
            return Err(format!(
                "the self-test vocabulary no longer splits {text:?} into {ids:?} (got {got:?}). \
                 The ids are what dcgrep's WordPiece produces; a change here is a change in the tokeniser"
            ));
        }
    }

    let files = walk(&repo, 200_000)?;
    let mut docs = Vec::new();
    for path in &files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let weight = if name == "alpha.rs" { 0.9 } else { 0.1 };
        docs.push(DocLine {
            path: path
                .strip_prefix(&repo)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/"),
            total_terms: 12,
            terms: vec![(2, weight)],
        });
    }
    if docs.len() != 2 {
        return Err(format!("the encoding should hold both documents, found {}", docs.len()));
    }
    jsonl::write_lines(&out, &header, &docs)?;
    println!("  ok    the encoding holds both documents");

    let built = build_index(&IndexRequest {
        root: repo.clone(),
        max_files: 0,
        sparse: Some(out.clone()),
    })?;
    if !built.ok {
        return Err(format!("index build returned ok: false ({built:?})"));
    }
    println!("  ok    dcgrep accepts the encoding this encoder writes");
    if built.lexical_vocabulary != "wordpiece-30522" {
        return Err(format!(
            "lexical_vocabulary is {:?}, want wordpiece-30522",
            built.lexical_vocabulary
        ));
    }
    println!("  ok    it publishes a learned index");
    if built.lexical_model.as_deref() != Some("self-test") {
        return Err(format!("lexical_model is {:?}, want self-test", built.lexical_model));
    }
    println!("  ok    it attributes the weights to this producer");
    if built.lexical_files != 2 || built.lexical_unmatched != 0 {
        return Err(format!(
            "lexical_files={} lexical_unmatched={}",
            built.lexical_files, built.lexical_unmatched
        ));
    }
    println!("  ok    every document was placed");

    let ranked = ranked_search(&RankedRequest {
        query: "json".into(),
        root: repo.clone(),
        path: String::new(),
        max_results: 0,
    })?;
    let paths: Vec<&str> = ranked.files.iter().map(|hit| hit.path.as_str()).collect();
    if paths != ["src/alpha.rs", "src/beta.rs"] {
        return Err(format!("ranking is {paths:?}, want alpha then beta"));
    }
    println!("  ok    the ranking follows the weights this encoder emitted");

    let mut lying = header;
    lying["parity"] = serde_json::json!([{"text": "parse", "ids": [3]}]);
    jsonl::write_lines(&out, &lying, &docs)?;
    match build_index(&IndexRequest {
        root: repo.clone(),
        max_files: 0,
        sparse: Some(out),
    }) {
        Err(err) if err.contains("disagrees") => {
            println!("  ok    a tokenizer disagreement refuses the build");
        }
        Err(err) => return Err(format!("a disagreement refused for the wrong reason: {err}")),
        Ok(_) => return Err("a tokenizer disagreement was accepted".into()),
    }

    let _ = std::fs::remove_dir_all(&room);
    let _ = Path::new(".");
    Ok(())
}
