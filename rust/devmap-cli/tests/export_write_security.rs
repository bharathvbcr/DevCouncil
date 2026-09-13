#![cfg(unix)]
use devmap_extract::subprocess::{run_bounded, Bounds};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn run(root: &Path, db: &Path, args: &[&str]) -> devmap_extract::subprocess::Captured {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
    command
        .current_dir(root)
        .arg("--db")
        .arg(db)
        .args(args)
        .env_remove("DEVMAP_HOME");
    run_bounded(
        &mut command,
        Bounds {
            deadline: Duration::from_secs(20),
            stdout_cap: 2 * 1024 * 1024,
            stderr_cap: 16384,
        },
    )
    .unwrap()
}

#[test]
fn graph_exports_refuse_linked_destinations() {
    for command in ["export", "html"] {
        for case in ["leaf", "parent", "explicit_parent"] {
            let root = std::env::temp_dir().join(format!(
                "devmap-export-boundary-{command}-{case}-{}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("repo")).unwrap();
            fs::create_dir_all(root.join("outside")).unwrap();
            let repo = root.join("repo");
            fs::write(repo.join("main.py"), "def f(): pass\n").unwrap();
            let db = root.join("index.sqlite");
            assert!(run(&repo, &db, &["build", "."]).status.success());
            let victim = root.join("outside/sentinel");
            fs::write(&victim, b"outside sentinel").unwrap();
            let filename = if command == "export" {
                "graph.graphml"
            } else {
                "graph.html"
            };
            match case {
                "leaf" => {
                    fs::create_dir_all(repo.join(".devmap")).unwrap();
                    symlink(&victim, repo.join(".devmap").join(filename)).unwrap();
                }
                "parent" => symlink(root.join("outside"), repo.join(".devmap")).unwrap(),
                "explicit_parent" => symlink(root.join("outside"), root.join("alias")).unwrap(),
                _ => unreachable!(),
            }
            let output = if case == "explicit_parent" {
                run(
                    &repo,
                    &db,
                    &[
                        command,
                        ".",
                        "--out",
                        root.join("alias/sentinel").to_str().unwrap(),
                    ],
                )
            } else {
                run(&repo, &db, &[command, "."])
            };
            assert!(
                !output.status.success(),
                "{command}/{case} accepted an unsafe output"
            );
            assert_eq!(fs::read(&victim).unwrap(), b"outside sentinel");
            assert!(!root.join("outside").join(filename).exists());
            fs::remove_dir_all(root).unwrap();
        }
    }
}
