use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

mod support;

struct GitRepo(PathBuf);

impl GitRepo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dcgrep-audit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create fixture root");
        let repo = Self(root);
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "user.name", "Test"]);
        repo
    }

    fn write(&self, rel: &str, contents: &[u8]) {
        let full = self.0.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(full, contents).expect("write fixture file");
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(&self.0)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn search(&self, mut req: Value) -> (bool, Value) {
        req["root"] = json!(self.0);
        let (ok, stdout) = support::run(
            "search",
            req.to_string().as_bytes(),
            Duration::from_secs(30),
        );
        let reply: Value = serde_json::from_slice(&stdout).unwrap_or_else(|err| {
            panic!(
                "invalid json ({err}): {:?}",
                String::from_utf8_lossy(&stdout)
            )
        });
        (ok, reply)
    }

    fn files(&self, mut req: Value) -> (bool, Value) {
        req["root"] = json!(self.0);
        let (ok, stdout) =
            support::run("files", req.to_string().as_bytes(), Duration::from_secs(30));
        let reply: Value = serde_json::from_slice(&stdout).unwrap_or_else(|err| {
            panic!(
                "invalid json ({err}): {:?}",
                String::from_utf8_lossy(&stdout)
            )
        });
        (ok, reply)
    }
}

impl Drop for GitRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn enotdir_and_directory_typechange_on_tracked_paths_are_skipped_cleanly() {
    let repo = GitRepo::new();
    repo.write("a/b.txt", b"needle inside deleted dir\n");
    repo.write("dir_replaced.txt", b"needle inside replaced file\n");
    repo.write("keep.txt", b"needle kept\n");
    repo.git(&["add", "a/b.txt", "dir_replaced.txt", "keep.txt"]);
    repo.git(&["commit", "-q", "-m", "initial"]);

    // Replace tracked directory `a` with regular file `a` (triggers ENOTDIR when checking `a/b.txt`)
    // and replace tracked file `dir_replaced.txt` with a directory.
    fs::remove_dir_all(repo.0.join("a")).expect("remove dir a");
    fs::write(repo.0.join("a"), b"plain file now\n").expect("write regular file a");
    fs::remove_file(repo.0.join("dir_replaced.txt")).expect("remove dir_replaced.txt");
    fs::create_dir_all(repo.0.join("dir_replaced.txt")).expect("mkdir dir_replaced.txt");
    repo.write("dir_replaced.txt/child.txt", b"untracked child\n");

    let (ok, reply) = repo.search(json!({
        "pattern": "needle",
        "include_devcouncil": true,
        "ignore_dot_ignore": true,
        "include_git_tracked": true
    }));
    assert!(ok, "search failed: {reply}");
    assert_eq!(reply["skipped"]["unreadable"], json!(0), "reply: {reply}");
    let paths: Vec<&str> = reply["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["keep.txt"]);
}

#[cfg(unix)]
#[test]
fn chmod_zero_on_tracked_ignored_file_reports_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let repo = GitRepo::new();
    repo.write(".gitignore", b"secret.txt\n");
    repo.write("secret.txt", b"needle hidden tracked\n");
    repo.write("visible.txt", b"needle visible\n");
    repo.git(&["add", ".gitignore", "visible.txt"]);
    repo.git(&["add", "-f", "secret.txt"]);
    repo.git(&["commit", "-q", "-m", "init"]);

    let secret_path = repo.0.join("secret.txt");
    let orig_mode = fs::metadata(&secret_path).unwrap().permissions().mode();
    fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o000)).unwrap();

    // Only assert unreadable if OS enforces permissions (not effective root).
    let read_blocked = fs::read(&secret_path).is_err();
    let (ok, reply) = repo.search(json!({
        "pattern": "needle",
        "include_git_tracked": true
    }));

    // Restore before assertions so tempdir cleanup succeeds even on failure.
    let _ = fs::set_permissions(&secret_path, fs::Permissions::from_mode(orig_mode));

    assert!(ok, "search failed: {reply}");
    if read_blocked {
        assert_eq!(reply["skipped"]["unreadable"], json!(1), "reply: {reply}");
    }
    let paths: Vec<&str> = reply["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["visible.txt"]);
}

#[test]
fn core_subsections_ignored_while_core_worktree_and_broken_config_worktree_fail_closed() {
    // 1. `[core "sub"]` must not trigger ignorecase or bare.
    let repo = GitRepo::new();
    repo.write(".gitignore", b"SECRET.txt\ntracked_ignored.txt\n");
    repo.write("secret.txt", b"needle in lowercase secret\n");
    repo.write("tracked_ignored.txt", b"needle in tracked ignored\n");
    repo.git(&["add", ".gitignore"]);
    repo.git(&["add", "-f", "tracked_ignored.txt"]);
    repo.git(&["commit", "-q", "-m", "init"]);

    let cfg_path = repo.0.join(".git/config");
    let mut cfg = fs::read_to_string(&cfg_path).unwrap();
    cfg.push_str("\n[core \"sub\"]\n\tignorecase = true\n\tbare = true\n[core.other]\n\tworktree = ../elsewhere\n");
    fs::write(&cfg_path, cfg).unwrap();

    // Set real [core] ignorecase = false so `[core "sub"]` is tested cleanly on macOS too.
    repo.git(&["config", "core.ignorecase", "false"]);

    let (ok, reply) = repo.search(json!({
        "pattern": "needle",
        "include_git_tracked": true
    }));
    assert!(ok, "search failed despite subsection-only keys: {reply}");
    let mut paths: Vec<String> = reply["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap().to_string())
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["secret.txt", "tracked_ignored.txt"]);

    // 2. Real `[core] worktree = ...` fails closed for `include_git_tracked`.
    let repo_wt = GitRepo::new();
    repo_wt.write("a.txt", b"needle\n");
    repo_wt.git(&["add", "a.txt"]);
    repo_wt.git(&["commit", "-q", "-m", "init"]);
    let wt_cfg = repo_wt.0.join(".git/config");
    let mut wt_body = fs::read_to_string(&wt_cfg).unwrap();
    wt_body.push_str("\n[core]\n\tworktree = ../other\n");
    fs::write(&wt_cfg, wt_body).unwrap();

    let (ok_wt, reply_wt) = repo_wt.search(json!({
        "pattern": "needle",
        "include_git_tracked": true
    }));
    assert!(!ok_wt, "expected fail closed for core.worktree: {reply_wt}");

    // 3. Broken symlink `.git/config.worktree` fails closed for `include_git_tracked`.
    #[cfg(unix)]
    {
        let repo_link = GitRepo::new();
        repo_link.write("a.txt", b"needle\n");
        repo_link.git(&["add", "a.txt"]);
        repo_link.git(&["commit", "-q", "-m", "init"]);
        std::os::unix::fs::symlink(
            "/nonexistent/dcgrep-missing-config-worktree",
            repo_link.0.join(".git/config.worktree"),
        )
        .unwrap();

        let (ok_link, reply_link) = repo_link.search(json!({
            "pattern": "needle",
            "include_git_tracked": true
        }));
        assert!(
            !ok_link,
            "expected fail closed for broken config.worktree symlink: {reply_link}"
        );
    }
}

fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn build_v2_git_index(paths: &[&str]) -> Vec<u8> {
    let mut sorted = paths.to_vec();
    sorted.sort_unstable();
    let mut buf = Vec::new();
    buf.extend_from_slice(b"DIRC");
    buf.extend_from_slice(&2u32.to_be_bytes());
    buf.extend_from_slice(&(sorted.len() as u32).to_be_bytes());
    for name in sorted {
        let start = buf.len();
        buf.extend_from_slice(&[0u8; 24]); // ctime/mtime/dev/ino
        buf.extend_from_slice(&0o100644u32.to_be_bytes()); // mode
        buf.extend_from_slice(&[0u8; 12]); // uid/gid/size
        buf.extend_from_slice(&[0x11u8; 20]); // fake object id
        let flags = (name.len().min(0x0fff)) as u16;
        buf.extend_from_slice(&flags.to_be_bytes());
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        while (buf.len() - start) % 8 != 0 {
            buf.push(0);
        }
    }
    let checksum = sha1_digest(&buf);
    buf.extend_from_slice(&checksum);
    buf
}

#[test]
fn rejects_empty_segments_backslashes_and_excludes_hfs_ntfs_dotgit_variants_in_index() {
    let repo = GitRepo::new();
    // Ignore everything so only `.git/index` (`include_git_tracked`) can reach these files.
    repo.write(".gitignore", b"*\n");
    repo.write("keep.txt", b"needle keep\n");
    repo.write("a/b.txt", b"needle double slash\n");
    repo.write("git~1/secret.txt", b"needle shortname\n");
    repo.write(".git\u{200C}/secret.txt", b"needle hfs\n");
    repo.write("sub/.git./secret.txt", b"needle trailing dot\n");
    repo.write(".devcouncil/dcgrep/blob.txt", b"needle dcgrep blob\n");

    for unsafe_entry in ["a//b.txt", "a/c.txt/", "sub\\.git/secret.txt"] {
        let index_bytes = build_v2_git_index(&[unsafe_entry, "keep.txt"]);
        fs::write(repo.0.join(".git/index"), index_bytes).expect("write crafted v2 index");
        let (ok, reply) = repo.search(json!({
            "pattern": "needle",
            "include_devcouncil": true,
            "include_git_tracked": true
        }));
        assert!(
            !ok,
            "expected fail closed for unsafe index path {unsafe_entry:?}: {reply}"
        );
        assert!(
            reply["error"]
                .as_str()
                .is_some_and(|e| e.contains("unsafe path in git index")),
            "unexpected error for {unsafe_entry:?}: {reply}"
        );
    }

    let index_bytes = build_v2_git_index(&[
        ".devcouncil/dcgrep/blob.txt",
        ".git\u{200C}/secret.txt",
        "git~1/secret.txt",
        "keep.txt",
        "sub/.git./secret.txt",
    ]);
    fs::write(repo.0.join(".git/index"), index_bytes).expect("write crafted v2 index");

    let (ok, reply) = repo.search(json!({
        "pattern": "needle",
        "include_devcouncil": true,
        "include_git_tracked": true
    }));
    assert!(ok, "search failed: {reply}");
    let paths: Vec<&str> = reply["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["keep.txt"]);

    // Verify `files` works cleanly on normal unignored files too.
    repo.write(".gitignore", b"");
    repo.write("ok.txt", b"needle ok\n");
    let (ok_files, reply_files) = repo.files(json!({
        "include_devcouncil": true,
        "ignore_dot_ignore": true
    }));
    assert!(ok_files, "files failed: {reply_files}");
    let file_paths: Vec<&str> = reply_files["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(file_paths.contains(&"ok.txt"));
    assert!(file_paths.contains(&"a/b.txt"));
    assert!(!file_paths.contains(&".devcouncil/dcgrep/blob.txt"));
}
