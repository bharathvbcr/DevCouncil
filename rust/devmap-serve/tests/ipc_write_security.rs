#![cfg(unix)]
use devmap_serve::protocol::UnixIpcServer;
use std::fs;
use std::os::unix::fs::symlink;

#[tokio::test]
async fn ipc_lock_cannot_write_a_link_target() {
    let root = std::path::PathBuf::from(format!("/tmp/devmap-ipc-boundary-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let victim = root.join("outside-sentinel");
    fs::write(&victim, b"outside sentinel\n").unwrap();
    let socket = root.join("ipc.sock");
    symlink(&victim, root.join("ipc.sock.lock")).unwrap();
    let outcome = UnixIpcServer::bind(&socket);
    assert!(outcome.is_err(), "linked IPC lock was accepted");
    assert_eq!(fs::read(&victim).unwrap(), b"outside sentinel\n");
    assert!(!socket.exists());
    fs::remove_dir_all(root).unwrap();
}
