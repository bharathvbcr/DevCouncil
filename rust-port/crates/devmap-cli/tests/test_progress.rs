use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn failures_identify_the_command_store_version_and_failed_stage() {
    let root = temp_root();
    let db = root.join("broken.sqlite");
    fs::write(&db, "not a sqlite database").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["build", "--json", "--progress", "never", "--db"])
        .arg(&db)
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = &payload["diagnostic_context"];
    assert_eq!(context["command"], "build");
    assert_eq!(context["db_path"], db.to_string_lossy().as_ref());
    assert!(context["binary_version"]
        .as_str()
        .unwrap()
        .contains("store schema"));
    assert!(context["stage"].as_str().unwrap().contains("scanning"));
    assert!(context["elapsed_ms"].is_number());
    assert!(context["pid"].as_u64().unwrap() > 0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("DevMap context:"), "{stderr}");
    assert!(!stderr.contains('\u{1b}'));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_build_failures_have_context_without_query_content() {
    let root = temp_root();
    let db = root.join("missing.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["search", "PRIVATE_QUERY_CONTENT", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["diagnostic_context"]["command"], "search");
    assert!(!payload["diagnostic_context"]
        .to_string()
        .contains("PRIVATE_QUERY_CONTENT"));
    assert!(payload["diagnostic_context"]["stage"].is_null());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn verbose_artifact_output_releases_the_writer_before_stdout_backpressure() {
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
    let root = temp_root();
    let (_reader, writer) = std::io::pipe().unwrap();
    let fd = writer.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert_eq!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let bytes = [b'x'; 4096];
    let mut full = false;
    for _ in 0..1024 {
        if unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) } < 0 {
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            full = true;
            break;
        }
    }
    assert!(full);
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) }, 0);
    let db = root.join("index.sqlite");
    let mut child = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args([
            "build",
            "--manifest",
            "--verbose",
            "--progress",
            "never",
            "--db",
        ])
        .arg(&db)
        .arg(&root)
        .arg("--output")
        .arg(root.join("map.json"))
        .arg("--graph-output")
        .arg(root.join("graph.json"))
        .stdout(writer)
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut released = false;
    while Instant::now() < deadline {
        if root.join("graph.json").exists() {
            if let Ok(lock) = devmap_store::Store::lock_writer_at(&db, Duration::ZERO) {
                drop(lock);
                released = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        root.join("graph.json").exists(),
        "artifact work did not finish"
    );
    fs::remove_dir_all(root).unwrap();
    assert!(
        released,
        "verbose artifact output held the writer behind stdout"
    );
}

#[cfg(unix)]
fn open_terminal() -> (fs::File, fs::File) {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    // ptsname uses shared storage. This test binary calls it only here, under
    // this lock. Both opens go through std, which sets CLOEXEC atomically;
    // setting it after openpty leaves a fork window in parallel tests.
    static PTY_NAMES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _names = PTY_NAMES.lock().unwrap();
    let master = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/ptmx")
        .unwrap();
    assert_eq!(unsafe { libc::grantpt(master.as_raw_fd()) }, 0);
    assert_eq!(unsafe { libc::unlockpt(master.as_raw_fd()) }, 0);
    let name = unsafe { libc::ptsname(master.as_raw_fd()) };
    assert!(!name.is_null());
    // SAFETY: ptsname returned a terminated string, borrowed while holding the
    // sole caller's lock and while its master is still open.
    let path = std::ffi::OsStr::from_bytes(unsafe { CStr::from_ptr(name) }.to_bytes());
    let slave = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(path)
        .unwrap();
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &size) },
        0
    );
    (master, slave)
}

#[cfg(unix)]
#[test]
fn terminal_fixture_handles_cannot_leak_into_concurrent_child_builds() {
    use std::os::fd::AsRawFd;
    let (master, slave) = open_terminal();
    for file in [master, slave] {
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "a leaked PTY master makes a disconnected peer look connected"
        );
    }
}

#[cfg(unix)]
#[test]
fn an_error_still_returns_json_when_the_progress_pipe_is_full() {
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
    for json in [true, false] {
        let root = temp_root();
        let (_reader, writer) = std::io::pipe().unwrap();
        let fd = writer.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let bytes = [b'x'; 4096];
        let mut full = false;
        for _ in 0..1024 {
            if unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) } < 0 {
                assert_eq!(
                    std::io::Error::last_os_error().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                full = true;
                break;
            }
        }
        assert!(full);
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) }, 0);
        let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
        if json {
            command.arg("--json");
        }
        let mut child = command
            .args(["build", "--progress", "always", "--db"])
            .arg(root.join("src/main.py/impossible.sqlite"))
            .arg(&root)
            .stderr(writer)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let blocked = child.try_wait().unwrap().is_none();
        if blocked {
            child.kill().unwrap();
        }
        let output = child.wait_with_output().unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(!blocked, "a diagnostic blocked the error result");
        assert!(!output.status.success());
        if !json {
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(
                text.contains("Error:") && text.contains("impossible.sqlite"),
                "plain failure disappeared: {text}"
            );
            continue;
        }
        let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(payload["error"].is_string());
        assert_eq!(payload["progress_output"]["incomplete"], true);
        assert_eq!(
            payload["progress_output"]["diagnostics"]["unrendered_total"],
            1
        );
        assert!(payload["timings"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage.get("open").is_none()));
    }
}

#[cfg(unix)]
#[test]
fn live_rows_adapt_after_a_real_terminal_resize() {
    use std::time::Duration;
    let root = temp_root();
    let lock =
        devmap_store::Store::lock_writer_at(&root.join("index.sqlite"), Duration::ZERO).unwrap();
    let (output, terminal) = terminal_build(
        &root,
        &["--progress", "always"],
        "xterm",
        |rendered, fd, _| {
            rendered.recv_timeout(Duration::from_secs(5)).unwrap();
            for width in [24, 120] {
                let size = libc::winsize {
                    ws_row: 24,
                    ws_col: width,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                assert_eq!(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) }, 0);
                std::thread::sleep(Duration::from_millis(240));
            }
            drop(lock);
        },
    );
    assert!(output.status.success(), "{terminal}");
    let rows: Vec<_> = terminal
        .split("\x1b[2K")
        .filter_map(|row| {
            let row = row.split(['\r', '\n', '\x1b']).next().unwrap_or("");
            row.contains("1/5").then_some(row)
        })
        .collect();
    let cells = |row: &&str| {
        row.chars()
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum::<usize>()
    };
    assert!(
        rows.iter().any(|row| cells(row) <= 23),
        "no narrow frame: {terminal}"
    );
    assert!(
        rows.iter().any(|row| cells(row) > 80),
        "no expanded frame: {terminal}"
    );
    assert!(
        !terminal.contains("\x1b[?25l"),
        "progress must never hide the cursor"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn interrupting_an_animated_lock_wait_preserves_terminal_and_store_recovery() {
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let root = temp_root();
        let lock = devmap_store::Store::lock_writer_at(&root.join("index.sqlite"), Duration::ZERO)
            .unwrap();
        let (output, terminal) = terminal_build(
            &root,
            &["--progress", "always"],
            "xterm",
            |rendered, _, pid| {
                rendered.recv_timeout(Duration::from_secs(5)).unwrap();
                assert_eq!(
                    unsafe { libc::kill(i32::try_from(pid).unwrap(), signal) },
                    0
                );
                drop(lock);
            },
        );
        assert_eq!(output.status.signal(), Some(signal));
        assert!(!terminal.contains("complete:"), "{terminal}");
        let rebuilt = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["build", "--json", "--progress", "never", "--db"])
            .arg(root.join("index.sqlite"))
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            rebuilt.status.success(),
            "{}",
            String::from_utf8_lossy(&rebuilt.stderr)
        );
        let connection = rusqlite::Connection::open(root.join("index.sqlite")).unwrap();
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        drop(connection);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn file_counters_distinguish_new_changed_removed_cached_and_skipped_work() {
    let root = temp_root();
    let build = |extra: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["build", "--json", "--progress", "never", "--db"])
            .arg(root.join("index.sqlite"))
            .arg(&root)
            .args(extra)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let cold = build(&[]);
    assert_eq!(cold["file_progress"]["scan"]["completed"], 1);
    assert_eq!(cold["file_progress"]["extraction"]["cache_hits"], 0);
    assert_eq!(cold["file_progress"]["delta"]["added"], 1);
    let warm = build(&[]);
    assert!(warm["file_progress"]["extraction"].is_null());
    assert_eq!(warm["file_progress"]["delta"]["unchanged"], 1);
    fs::write(root.join("src/new.py"), "def added(): return 2\n").unwrap();
    let added = build(&[]);
    assert_eq!(added["file_progress"]["extraction"]["completed"], 2);
    assert_eq!(added["file_progress"]["extraction"]["cache_hits"], 1);
    assert_eq!(added["file_progress"]["delta"]["added"], 1);
    fs::remove_file(root.join("src/main.py")).unwrap();
    fs::write(root.join("src/new.py"), "def changed(): return 3\n").unwrap();
    let changed = build(&[]);
    assert_eq!(changed["file_progress"]["delta"]["changed"], 1);
    assert_eq!(changed["file_progress"]["delta"]["removed"], 1);
    assert_eq!(
        build(&["--full"])["file_progress"]["extraction"]["cache_hits"],
        0
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_build_has_one_compact_summary_and_verbose_retains_details() {
    let root = temp_root();
    for verbose in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
        command
            .args(["build", "--full", "--progress", "never", "--db"])
            .arg(root.join("index.sqlite"))
            .arg(&root);
        if verbose {
            command.arg("--verbose");
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.contains("Symbols extracted:"), verbose, "{stdout}");
        assert!(stdout.contains("generation #"), "{stdout}");
        assert!(stdout.contains("1 file ·"), "{stdout}");
        assert!(!stdout.contains("1 files"), "{stdout}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_closed_progress_pipe_does_not_signal_the_indexing_process() {
    let root = temp_root();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["build", "--json", "--progress", "always", "--db"])
        .arg(root.join("index.sqlite"))
        .arg(&root)
        .stderr(writer)
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "progress failure terminated the index: {:?}",
        output.status
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["files_indexed"], 1);
    assert_eq!(payload["progress_output"]["incomplete"], true);
}

#[cfg(unix)]
#[test]
fn a_paused_terminal_cannot_hold_the_build_or_change_parent_descriptor_flags() {
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
    let root = temp_root();
    let (master, slave) = open_terminal();
    let flags = unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(unsafe { libc::tcflow(slave.as_raw_fd(), libc::TCOOFF) }, 0);
    // A pause does not imply a full kernel queue. Saturate it so the fixture
    // proves actual backpressure, rather than successful deferred delivery.
    assert_eq!(
        unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let block = [b'x'; 4096];
    let mut full = false;
    for _ in 0..256 {
        let result = unsafe { libc::write(slave.as_raw_fd(), block.as_ptr().cast(), block.len()) };
        if result < 0 {
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            full = true;
            break;
        }
    }
    assert!(full, "fixture did not saturate the terminal output queue");
    assert_eq!(
        unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_SETFL, flags) },
        0
    );
    // macOS adds an internal F_GETFL bit when the queue saturates. The
    // invariant is what the child inherits after fixture setup, not before it.
    let inherited_flags = unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_GETFL) };
    assert_eq!(inherited_flags & libc::O_NONBLOCK, flags & libc::O_NONBLOCK);
    let mut child = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["build", "--json", "--progress", "always", "--db"])
        .arg(root.join("index.sqlite"))
        .arg(&root)
        .env("TERM", "xterm-256color")
        .stderr(slave.try_clone().unwrap())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let blocked = child.try_wait().unwrap().is_none();
    if blocked {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let after_flags = unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_GETFL) };
    assert_eq!(unsafe { libc::tcflow(slave.as_raw_fd(), libc::TCOON) }, 0);
    drop(slave);
    drop(master);
    fs::remove_dir_all(root).unwrap();
    assert!(
        !blocked,
        "progress held the build behind paused terminal output for two seconds"
    );
    assert!(output.status.success());
    assert_eq!(
        inherited_flags, after_flags,
        "progress modified the parent's open-file description"
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["files_indexed"], 1);
    assert!(
        payload["progress_output"]["incomplete"].as_bool().unwrap(),
        "unrendered output must be disclosed: {payload}"
    );
}

#[cfg(unix)]
#[test]
fn a_disconnected_terminal_does_not_panic_or_destroy_the_json_result() {
    let root = temp_root();
    let (master, slave) = open_terminal();
    drop(master);
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["build", "--json", "--progress", "always", "--db"])
        .arg(root.join("index.sqlite"))
        .arg(&root)
        .env("TERM", "xterm-256color")
        .stderr(slave)
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "a failed display is not a failed index: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["files_indexed"], 1);
    assert_eq!(payload["progress_output"]["incomplete"], true);
}

/// Exercise the binary with a real terminal on stderr and a separate JSON
/// pipe on stdout. Both the child and terminal drain have bounded waits.
#[cfg(unix)]
fn terminal_build(
    root: &std::path::Path,
    args: &[&str],
    term: &str,
    after_spawn: impl FnOnce(&std::sync::mpsc::Receiver<()>, i32, u32),
) -> (std::process::Output, String) {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    let (mut master, slave) = open_terminal();
    let control = slave.try_clone().unwrap();
    let termios = || {
        let mut value = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(
            unsafe { libc::tcgetattr(control.as_raw_fd(), value.as_mut_ptr()) },
            0
        );
        let value = unsafe { value.assume_init() };
        (
            value.c_iflag,
            value.c_oflag,
            value.c_cflag,
            value.c_lflag,
            value.c_cc,
        )
    };
    let before = termios();
    let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
    command
        .args(["build", "--json", "--db"])
        .arg(root.join("index.sqlite"))
        .arg(root)
        .args(args)
        .env("TERM", term)
        .env("NO_COLOR", "1")
        .env("LC_ALL", "en_US.UTF-8")
        .stderr(slave)
        .stdout(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    drop(command); // Drop the parent's slave before waiting for terminal EOF.
    let (first_frame, rendered) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            assert!(Instant::now() < deadline, "terminal output did not close");
            let mut ready = libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll receives one valid pollfd for the owned master.
            let result = unsafe { libc::poll(&mut ready, 1, 100) };
            if result == 0 {
                continue;
            }
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                panic!("terminal poll failed: {error}");
            }
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if bytes.is_empty() {
                        first_frame.send(()).unwrap();
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                // Linux reports EIO rather than EOF when the slave closes.
                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => panic!("terminal read failed: {e}"),
            }
        }
        String::from_utf8(bytes).unwrap()
    });
    after_spawn(&rendered, control.as_raw_fd(), child.id());
    assert_eq!(termios(), before, "progress changed terminal modes");
    drop(control);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("terminal build exceeded 15 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    (child.wait_with_output().unwrap(), reader.join().unwrap())
}

#[cfg(unix)]
#[test]
fn a_real_terminal_animates_during_work_and_stops_before_the_result() {
    use std::time::Duration;
    let root = temp_root();
    let lock =
        devmap_store::Store::lock_writer_at(&root.join("index.sqlite"), Duration::ZERO).unwrap();
    let (output, terminal) = terminal_build(
        &root,
        &["--progress", "always"],
        "xterm-256color",
        |rendered, _, _| {
            // A held writer lock makes a slow stage deterministic without a large
            // repository, a mocked reporter or performance assumptions.
            rendered
                .recv_timeout(Duration::from_secs(5))
                .expect("first live frame");
            std::thread::sleep(Duration::from_millis(350));
            drop(lock);
        },
    );
    assert!(output.status.success(), "{terminal}");
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["files_indexed"], 1);
    assert!(terminal.contains("\r\x1b[2K"), "no live redraw: {terminal}");
    assert!(terminal.contains("╺━━"), "no stage bar: {terminal}");
    let frames: std::collections::BTreeSet<_> = terminal
        .chars()
        .filter(|c| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(*c))
        .collect();
    assert!(frames.len() >= 2, "no heartbeat while waiting: {terminal}");
    assert!(
        !terminal.contains("\x1b[36m"),
        "NO_COLOR must suppress styling"
    );
    let (_, after_completion) = terminal.split_once("complete:").expect("completion");
    assert!(
        !after_completion.contains('\x1b'),
        "redraw after completion: {terminal}"
    );

    let (output, terminal) = terminal_build(
        &root,
        &["--progress", "always"],
        "xterm-256color",
        |_, _, _| {},
    );
    assert!(output.status.success());
    assert!(terminal.contains("up to date"));
    assert!(!terminal.contains("build stopped"));
    for (args, term) in [
        (&["--progress", "always"][..], "dumb"),
        (&["--progress", "never"][..], "xterm"),
        (&[][..], "xterm"),
    ] {
        let (output, terminal) = terminal_build(&root, args, term, |_, _, _| {});
        assert!(output.status.success());
        assert!(!terminal.contains('\x1b'), "{term}: {terminal}");
        if term != "dumb" {
            assert!(terminal.is_empty(), "JSON auto/never: {terminal}");
        }
    }
    let (output, terminal) = terminal_build(
        &root,
        &["--progress", "always", "--affected", "../outside.py"],
        "xterm-256color",
        |_, _, _| {},
    );
    assert!(!output.status.success());
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(payload["error"].is_string());
    assert!(terminal.contains("build stopped"), "{terminal}");
    assert!(!terminal.contains("complete:"), "{terminal}");
    let (_, after_error) = terminal.split_once("Error:").expect("failure diagnostic");
    assert!(
        !after_error.contains('\x1b'),
        "redraw after error: {terminal}"
    );
    let lock =
        devmap_store::Store::lock_writer_at(&root.join("index.sqlite"), Duration::ZERO).unwrap();
    let invalid_output = root.join("src/main.py/map.json");
    let (output, terminal) = terminal_build(
        &root,
        &[
            "--progress",
            "always",
            "--full",
            "--manifest",
            "--force",
            "--output",
            invalid_output.to_str().unwrap(),
        ],
        "xterm-256color",
        |rendered, _, _| {
            rendered.recv_timeout(Duration::from_secs(5)).unwrap();
            drop(lock);
        },
    );
    assert!(!output.status.success(), "{terminal}");
    assert!(
        !terminal.contains("✓ [5/5]"),
        "a failed stage received a success marker: {terminal}"
    );
    assert!(
        terminal.contains("writing consumer artifacts failed"),
        "{terminal}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unchanged_progress_finishes_and_reclaim_is_printed_once() {
    let root = temp_root();
    let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
    command
        .args(["build", "--verbose", "--progress", "always", "--db"])
        .arg(root.join("index.sqlite"))
        .arg(&root);
    assert!(command.output().unwrap().status.success());
    let output = command.output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let combined = format!("{stderr}{}", String::from_utf8_lossy(&output.stdout));
    assert!(combined.contains("still current"), "{combined}");
    assert_eq!(combined.matches("generation #").count(), 1, "{combined}");
    assert_eq!(
        combined.to_lowercase().matches("reclaim:").count(),
        1,
        "{combined}"
    );

    let output = command.arg("--json").output().unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["unchanged"], true);
    assert!(
        payload["timings"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage.get("open").is_none()),
        "{payload}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn auto_progress_and_forced_log_output_are_terminal_escape_free() {
    let root = temp_root();
    for mode in ["auto", "always", "never"] {
        let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["build", "--json", "--full", "--progress", mode, "--db"])
            .arg(root.join("index.sqlite"))
            .arg(&root)
            .output()
            .unwrap();
        assert!(output.status.success());
        let _: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!output.stderr.contains(&b'\x1b'));
        assert!(!output.stderr.contains(&b'\r'));
        if mode != "always" {
            assert!(output.stderr.is_empty());
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_build_does_not_announce_completion() {
    let root = temp_root();
    // A database whose parent is a file fails after the reporter starts.
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["build", "--json", "--progress", "always", "--db"])
        .arg(root.join("src/main.py/index.sqlite"))
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(payload["error"].is_string());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("complete:"), "{stderr}");
    assert!(stderr.contains("build stopped"), "{stderr}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_waits_for_requested_artifacts_on_cold_and_unchanged_builds() {
    let root = temp_root();
    for unchanged in [false, true] {
        let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args([
                "build",
                "--json",
                "--progress",
                "always",
                "--manifest",
                "--force",
                "--db",
            ])
            .arg(root.join("index.sqlite"))
            .arg(&root)
            .arg("--output")
            .arg(root.join("src/main.py/map.json"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(payload["error"].is_string(), "{payload}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            !stderr.contains("complete:") && !stderr.contains("up to date:"),
            "{stderr}"
        );
        assert!(stderr.contains("build stopped"), "{stderr}");
        assert_eq!(stderr.contains("file unchanged"), unchanged, "{stderr}");
        // Persistence succeeded; this is a later failure, not an argument refusal.
        let store = devmap_store::Store::open(root.join("index.sqlite")).unwrap();
        assert_eq!(store.latest_generation_id().unwrap(), Some(1));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_empty_tree_completes_without_inventing_file_progress() {
    let root = temp_root();
    fs::remove_file(root.join("src/main.py")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["build", "--json", "--progress", "always", "--db"])
        .arg(root.join("index.sqlite"))
        .arg(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["files_indexed"], 0);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("complete:"), "{stderr}");
    assert!(
        !stderr.contains("NaN") && !stderr.contains("inf%"),
        "{stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

/// Temp roots must be unique per call, not merely per instant. `SystemTime`
/// resolution on macOS is 1 us, so two tests entering this function in the same
/// microsecond used to receive the *same* directory; whichever finished first
/// deleted the tree out from under its sibling. The pid and the monotonic
/// counter make the name unique within and across processes.
fn temp_root() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-progress-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    fs::write(root.join("src/main.py"), "def main():\n    return 0\n").expect("write fixture");
    root
}

#[test]
fn build_progress_is_bounded_complete_and_keeps_json_stdout_clean() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--progress", "always", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .output()
        .expect("run build with forced progress");

    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout remains one JSON value");
    assert_eq!(payload["files_indexed"], 1);

    let progress = String::from_utf8(output.stderr).expect("progress is UTF-8");
    for expected in ["[1/5]", "[2/5]", "[3/5]", "[4/5]", "[5/5]"] {
        assert!(
            progress.contains(expected),
            "missing {expected}: {progress}"
        );
    }
    assert!(
        progress.contains("complete"),
        "missing completion: {progress}"
    );

    fs::remove_dir_all(root).expect("remove fixture tree");
}

/// `persist:write` carries the split of what it wrote, by relation.
///
/// One number cannot be acted on. On this repository `persist:write` is 0.30 s
/// of a 1.10 s one-file incremental build, and the relations beneath it have
/// nothing in common as fixes: v18 put the edges and the unresolved ledger on
/// validity ranges and left the nodes, the full-text map, the file rows, the
/// dead symbols and the coverage gaps as full per-generation copies. Which of
/// those the 0.30 s is decides whether the next schema rung is worth its
/// migration, and no profiler outside the store can answer it — the node and
/// full-text inserts are one interleaved loop.
///
/// The split is asserted here, on the CLI's own `--json` output, because that
/// is where a reader meets it. Two properties, both about honesty:
/// every relation is named even when it wrote nothing, and the parts never
/// outlast the phase that contains them.
#[test]
fn the_persist_write_phase_reports_what_each_relation_cost() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--progress", "never", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .output()
        .expect("run build");
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON value");

    /// The sub-phase named `wanted`, at any nesting depth.
    fn find<'a>(stages: &'a [serde_json::Value], wanted: &str) -> Option<&'a serde_json::Value> {
        for stage in stages {
            if stage["stage"] == wanted {
                return Some(stage);
            }
            if let Some(nested) = stage["sub"].as_array() {
                if let Some(hit) = find(nested, wanted) {
                    return Some(hit);
                }
            }
        }
        None
    }

    let stages = payload["timings"]["stages"]
        .as_array()
        .unwrap_or_else(|| panic!("a build carries timings: {payload}"));
    let write = find(stages, "persist:write")
        .unwrap_or_else(|| panic!("the write is a timed phase: {payload}"));
    let parts = write["sub"]
        .as_array()
        .unwrap_or_else(|| panic!("persist:write reports no per-relation split: {write}"));

    let named: Vec<&str> = parts
        .iter()
        .map(|part| part["stage"].as_str().unwrap_or("<unnamed>"))
        .collect();
    // Every relation, always — a relation that wrote nothing this build reports
    // zero rather than vanishing, because a missing name and a name worth
    // nothing are the same silence to a reader deciding what to fix.
    assert_eq!(
        named,
        vec![
            "file_rows",
            "nodes",
            "fts",
            "edges",
            "unresolved",
            "digests",
            "gaps",
            "dead",
            "history",
            "commit",
        ],
        "the split names every relation the write touches: {write}"
    );

    let whole = write["seconds"].as_f64().expect("the write has a duration");
    let charged: f64 = parts
        .iter()
        .map(|part| part["seconds"].as_f64().expect("a part has a duration"))
        .sum();
    assert!(
        charged <= whole + 1e-6,
        "the parts of a phase cannot outlast it: {charged}s charged of {whole}s: {write}"
    );

    fs::remove_dir_all(root).expect("remove fixture tree");
}

/// Fixture roots must never be shared between concurrently running tests.
/// Against a purely timestamp-keyed root this fails: `SystemTime` advances in
/// 1 us steps here, so threads entering together receive one identical path and
/// the first teardown destroys a live sibling's tree.
#[test]
fn fixture_roots_are_unique_under_concurrent_construction() {
    let roots: Vec<std::path::PathBuf> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| scope.spawn(|| (0..16).map(|_| temp_root()).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("fixture thread must not panic"))
            .collect()
    });

    let distinct: std::collections::BTreeSet<_> = roots.iter().collect();
    assert_eq!(
        distinct.len(),
        roots.len(),
        "temp_root() handed the same directory to two callers"
    );
    for root in roots {
        fs::remove_dir_all(root).expect("remove fixture tree");
    }
}

#[test]
fn progress_never_suppresses_all_progress_output() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--progress", "never", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .output()
        .expect("run build without progress");

    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(root).expect("remove fixture tree");
}

#[test]
fn history_reports_measured_builds_and_deltas_as_json() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    for body in [
        "def main():\n    return 0\n",
        "def main():\n    return 1\n\ndef helper():\n    return 2\n",
    ] {
        fs::write(root.join("src/main.py"), body).expect("update fixture");
        let build = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["--json", "--progress", "never", "--db"])
            .arg(&db)
            .arg("build")
            .arg(&root)
            .output()
            .expect("run history fixture build");
        assert!(
            build.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--db"])
        .arg(&db)
        .args(["history", "--last", "2"])
        .output()
        .expect("query build history");
    assert!(
        output.status.success(),
        "history failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("history stdout is JSON");
    assert_eq!(payload["shown"], 2);
    let history = payload["history"].as_array().expect("history is an array");
    assert_eq!(history.len(), 2);
    assert!(history[0]["build_ms"].is_number());
    assert!(history[0]["delta"]["symbols"].is_number());
    assert!(history[1]["delta"].is_null());

    fs::remove_dir_all(root).expect("remove fixture tree");
}

/// The most frequent build in the system had no phase profile at all.
///
/// A no-source-change build is what a watcher-driven repository does on almost
/// every tick, and `--json` answered it with
/// `{"unchanged":true,"files":…,"generation":…,"reclaim":…}` — no `timings` key
/// of any kind. So the one build shape a profiler most wants to look at was the
/// one it could not see, and "the warm path is fast" was an assertion nobody
/// could check from the tool's own output.
///
/// It is not a free-standing key either: the branch already spends measurable
/// time — it hashes every file in the tree to *prove* nothing changed, and it
/// runs `persist:vacuum` — so the absence was a reporting gap, not an empty
/// truth.
#[test]
fn an_unchanged_build_reports_its_own_timings() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let build = || {
        Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["--json", "--db"])
            .arg(&db)
            .arg("build")
            .arg(&root)
            .output()
            .expect("run build")
    };

    let first = build();
    assert!(first.status.success(), "first build must succeed");

    let second = build();
    assert!(second.status.success(), "second build must succeed");
    let payload: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("--json must emit JSON");
    assert_eq!(
        payload["unchanged"],
        serde_json::Value::Bool(true),
        "the second build must take the unchanged path: {payload}"
    );

    let timings = payload
        .get("timings")
        .unwrap_or_else(|| panic!("an unchanged build must report timings: {payload}"));
    let total = timings["total_seconds"]
        .as_f64()
        .unwrap_or_else(|| panic!("total_seconds must be a number: {timings}"));
    assert!(
        total > 0.0,
        "a build that hashed every file cannot have taken zero time: {timings}"
    );
    let stages = timings["stages"]
        .as_array()
        .unwrap_or_else(|| panic!("stages must be an array: {timings}"));
    assert!(
        !stages.is_empty(),
        "the unchanged path runs discovery and a vacuum decision; both are stages: {timings}"
    );
    // The reclaim decision is the one stage a warm-path profiler is looking
    // for — K5 was a reclaim that reported success without doing anything.
    //
    // Searched through `sub` as well as the top level, because that is where it
    // legitimately lands: `persist:vacuum` is nested under the stage that ran
    // it, exactly as the full build nests its own `persist:*` entries.
    let names_vacuum = |stage: &serde_json::Value| {
        stage["stage"].as_str() == Some("persist:vacuum")
            || stage["sub"].as_array().is_some_and(|subs| {
                subs.iter()
                    .any(|entry| entry["stage"].as_str() == Some("persist:vacuum"))
            })
    };
    assert!(
        stages.iter().any(names_vacuum),
        "the vacuum decision must be a timed stage: {timings}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
