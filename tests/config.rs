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

fn listening_addrs(args: &[&str], want_ui: bool) -> (String, Option<String>) {
    let exe = env!("CARGO_BIN_EXE_microtel");
    let mut child = Command::new(exe)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("microtel spawns");
    let stderr = child.stderr.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ingest = None;
    let mut ui = None;
    for line in std::io::BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if let Some(a) = line.split("ui listening on ").nth(1) {
            ui = Some(a.trim().to_string());
        } else if let Some(a) = line.split("listening on ").nth(1) {
            ingest = Some(a.trim().to_string());
        }
        if ingest.is_some() && (ui.is_some() || !want_ui) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    (ingest.expect("server reported its address"), ui)
}

fn listening_addr(args: &[&str]) -> String {
    listening_addrs(args, false).0
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

#[test]
fn default_ui_port_and_healthcheck_target() {
    let dir = repo_tmp().join("cfg-ui-default");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let exe = env!("CARGO_BIN_EXE_microtel");
    let mut child = Command::new(exe)
        .args([
            "--storage-dir",
            data.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("microtel spawns");
    let mut ui = None;
    for line in std::io::BufReader::new(child.stderr.take().unwrap()).lines() {
        let Ok(line) = line else { break };
        if let Some(a) = line.split("ui listening on ").nth(1) {
            ui = Some(a.trim().to_string());
            break;
        }
    }
    assert_eq!(
        ui.as_deref(),
        Some("127.0.0.1:8096"),
        "documented default UI port"
    );
    let status = run(&["healthcheck"], None);
    assert!(
        status.success(),
        "bare healthcheck must probe the default UI port"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cli_ui_listen_beats_yaml() {
    let dir = repo_tmp().join("cfg-ui-flag");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let cfg = write_yaml(
        &dir,
        "listen: 127.0.0.1:19871\nui:\n  listen: 127.0.0.1:19881\n",
    );
    let (ingest, ui) = listening_addrs(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "--storage-dir",
            data.to_str().unwrap(),
            "--ui-listen",
            "127.0.0.1:19882",
        ],
        true,
    );
    assert_eq!(ingest, "127.0.0.1:19871", "yaml listen must stay in force");
    assert_eq!(
        ui.as_deref(),
        Some("127.0.0.1:19882"),
        "CLI --ui-listen must beat the yaml value"
    );
}

#[test]
fn no_ui_flag_disables_the_ui_endpoint() {
    let dir = repo_tmp().join("cfg-no-ui");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let cfg = write_yaml(&dir, "ui:\n  listen: 127.0.0.1:19883\n");
    let (_ingest, ui) = listening_addrs(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "--storage-dir",
            data.to_str().unwrap(),
            "--no-ui",
        ],
        false,
    );
    assert_eq!(ui, None, "--no-ui must not bind the UI endpoint");
    // both binds happen before any log line, so the port state is already final
    assert!(
        std::net::TcpStream::connect("127.0.0.1:19883").is_err(),
        "UI port must stay closed under --no-ui"
    );
}

#[test]
fn unbindable_ui_listen_is_a_startup_error() {
    let dir = repo_tmp().join("cfg-ui-listen");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let status = run(
        &[
            "--storage-dir",
            data.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
            "--ui-listen",
            "not-an-address",
        ],
        None,
    );
    assert!(
        !status.success(),
        "garbage ui listen address must fail startup"
    );
}

#[test]
fn endpoints_do_not_overlap() {
    let dir = repo_tmp().join("cfg-endpoints");
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let (mut child, ingest, ui) = common::spawn_server(&data, None);
    let status_of = |url: String| match ureq::get(&url).call() {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("request {url} failed: {e}"),
    };
    assert_eq!(status_of(format!("http://{ingest}/health")), 404);
    assert_eq!(status_of(format!("http://{ingest}/stats")), 404);
    assert_eq!(status_of(format!("http://{ingest}/")), 404);
    assert_eq!(
        status_of(format!("http://{ingest}/api/metrics/series")),
        404
    );
    assert_eq!(status_of(format!("http://{ui}/health")), 200);
    assert_eq!(status_of(format!("http://{ui}/stats")), 200);
    // OTLP aimed at the UI port must not be swallowed by the SPA fallback
    let code = match ureq::post(&format!("http://{ui}/v1/traces")).send_string("") {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("post failed: {e}"),
    };
    assert_eq!(code, 405);
    child.kill().expect("kill server");
    child.wait().expect("wait for server");
}
