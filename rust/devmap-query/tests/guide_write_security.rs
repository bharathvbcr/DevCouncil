#![cfg(unix)]
use devmap_query::guides::{write_agent_guides, AGENT_GUIDE_MARKER, CURSOR_RULE_REL};
use std::fs;
use std::os::unix::fs::symlink;

#[test]
fn read_only_guides_that_need_no_write_keep_their_disposition() {
    use devmap_query::guides::{agent_guide_text, GuideDisposition};
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("devmap-guide-readonly-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let map = serde_json::json!({});
    let generated = agent_guide_text(&map, "map.json", "graph.json", "store.sqlite") + "\n";
    fs::write(root.join("AGENTS.md"), "handwritten instructions\n").unwrap();
    fs::write(root.join("CLAUDE.md"), &generated).unwrap();
    for name in ["AGENTS.md", "CLAUDE.md"] {
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o444)).unwrap();
    }
    let outcome = write_agent_guides(&root, &map, "map.json", "graph.json", "store.sqlite");
    for name in ["AGENTS.md", "CLAUDE.md"] {
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    assert_eq!(
        fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "handwritten instructions\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
        generated
    );
    fs::remove_dir_all(root).unwrap();
    let outcomes = outcome.expect("unmarked and unchanged guides need no write access");
    assert_eq!(outcomes[0].disposition, GuideDisposition::NotOurs);
    assert_eq!(outcomes[1].disposition, GuideDisposition::Unchanged);
}

#[test]
fn guide_outputs_refuse_links_and_preflight_the_entire_set() {
    for case in ["guide", "cursor_parent", "cursor_leaf", "hardlink"] {
        let root = std::env::temp_dir().join(format!(
            "devmap-guide-boundary-{case}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("repo")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        let victim = root.join("outside/notes");
        let original = format!("{AGENT_GUIDE_MARKER}\noutside sentinel\n");
        fs::write(&victim, &original).unwrap();
        let repo = root.join("repo");
        match case {
            "guide" => symlink(&victim, repo.join("AGENTS.md")).unwrap(),
            "cursor_parent" => symlink(root.join("outside"), repo.join(".cursor")).unwrap(),
            "cursor_leaf" => {
                fs::create_dir_all(repo.join(".cursor/rules")).unwrap();
                symlink(&victim, repo.join(CURSOR_RULE_REL)).unwrap();
            }
            "hardlink" => fs::hard_link(&victim, repo.join("CLAUDE.md")).unwrap(),
            _ => unreachable!(),
        }
        let outcome = write_agent_guides(
            &repo,
            &serde_json::json!({}),
            "map.json",
            "graph.json",
            "store.sqlite",
        );
        assert!(outcome.is_err(), "{case} was accepted");
        assert_eq!(
            fs::read_to_string(&victim).unwrap(),
            original,
            "{case} touched outside contents"
        );
        if case != "guide" {
            assert!(
                !repo.join("AGENTS.md").exists(),
                "{case} wrote a partial guide set before refusing"
            );
        }
        assert!(!root.join("outside/rules").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
