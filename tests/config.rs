mod common;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn repo_tmp() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], cwd: Option<&Path>) -> std::process::ExitStatus {
    let exe = env!("CARGO_BIN_EXE_microtel");
    let mut cmd = Command::new(exe);
    cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.status().expect("microtel spawns")
}

fn listening_addr(args: &[&str]) -> String {
    let exe = env!("CARGO_BIN_EXE_microtel");
    let mut child = Command::new(exe)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("microtel spawns");
    let stderr = child.stderr.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut addr = None;
    for line in std::io::BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if let Some(a) = line.split("listening on ").nth(1) {
            addr = Some(a.trim().to_string());
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    addr.expect("server reported its address")
}

fn write_yaml(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("microtel.yaml");
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn yaml_defaults_are_applied() {
    let dir = repo_tmp().join("cfg-yaml");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let cfg = write_yaml(&dir, "listen: 127.0.0.1:19871\n");
    let addr = listening_addr(&[
        "--config",
        cfg.to_str().unwrap(),
        "--storage-dir",
        dir.join("data").to_str().unwrap(),
    ]);
    assert_eq!(addr, "127.0.0.1:19871", "yaml listen wins when no CLI flag");
}

#[test]
fn cli_overrides_yaml() {
    let dir = repo_tmp().join("cfg-override");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let cfg = write_yaml(&dir, "listen: 127.0.0.1:19872\n");
    let addr = listening_addr(&[
        "--config",
        cfg.to_str().unwrap(),
        "--storage-dir",
        data.to_str().unwrap(),
        "--listen",
        "127.0.0.1:0",
    ]);
    assert!(
        addr.starts_with("127.0.0.1:") && !addr.ends_with(":19872"),
        "CLI --listen must beat the yaml value, got {addr}"
    );
}

#[test]
fn built_in_defaults_when_no_config_file() {
    let dir = repo_tmp().join("cfg-default");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let addr = listening_addr(&["--storage-dir", data.to_str().unwrap()]);
    assert_eq!(addr, "127.0.0.1:4318", "documented default port");
}

#[test]
fn malformed_yaml_is_a_startup_error() {
    let dir = repo_tmp().join("cfg-bad");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = write_yaml(&dir, "storage_dir: [unclosed\n  bad yaml {{{\n");
    let status = run(&["--config", cfg.to_str().unwrap()], None);
    assert!(!status.success(), "malformed yaml must fail startup");
}

#[test]
fn missing_config_file_is_an_error_but_implicit_yaml_is_optional() {
    let status = run(&["--config", "/nonexistent/microtel.yaml"], None);
    assert!(!status.success(), "explicit --config must exist");
    // no config anywhere: defaults apply (covered by built_in_defaults_when_no_config_file)
}

#[test]
fn unbindable_listen_is_a_startup_error() {
    let dir = repo_tmp().join("cfg-listen");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let status = run(
        &[
            "--storage-dir",
            data.to_str().unwrap(),
            "--listen",
            "not-an-address",
        ],
        None,
    );
    assert!(
        !status.success(),
        "garbage listen address must fail startup"
    );
}

#[test]
fn storage_dir_on_a_file_is_a_startup_error() {
    let dir = repo_tmp().join("cfg-storage");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    let status = run(
        &[
            "--storage-dir",
            file.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
        ],
        None,
    );
    assert!(
        !status.success(),
        "storage_dir pointing at a file must fail"
    );
}
