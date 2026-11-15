#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn helper_waits_for_parent_pipe_then_replaces_and_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let stage = tempfile::Builder::new()
        .prefix(".designer1-update-")
        .tempdir_in(dir.path())
        .unwrap();
    let target = dir.path().join("designer1");
    let next = stage.path().join("designer1");
    let old = b"old executable";
    let new = b"#!/bin/sh\nprintf restarted > marker\n";
    fs::write(&target, old).unwrap();
    fs::write(&next, new).unwrap();
    fs::set_permissions(&next, fs::Permissions::from_mode(0o755)).unwrap();

    let mut helper = Command::new(env!("CARGO_BIN_EXE_designer1"))
        .arg("--internal-apply-update")
        .arg(&target)
        .arg(stage.path())
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    helper.stdin.as_mut().unwrap().flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fs::read(&target).unwrap(), old);
    drop(helper.stdin.take());
    assert!(helper.wait().unwrap().success());
    assert_eq!(fs::read(&target).unwrap(), new);
    assert_eq!(
        fs::read(stage.path().join("previous-executable")).unwrap(),
        old
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !dir.path().join("marker").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read(dir.path().join("marker")).unwrap(), b"restarted");
}
