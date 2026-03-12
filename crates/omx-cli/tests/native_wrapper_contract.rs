#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn native_binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_omx")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root().join("target/debug/omx"))
}

fn wrapper_path() -> PathBuf {
    repo_root().join("bin/omx")
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("omx-native-wrapper-{label}-{nanos}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn make_executable(path: &Path) {
    let metadata = fs::metadata(path).expect("metadata");
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}

fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {}", path.display());
}

#[test]
fn bin_wrapper_launches_native_omx_without_node_or_tmux_runtime() {
    let cwd = temp_dir("launch");
    let home = cwd.join("home");
    let fake_bin = cwd.join("bin");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&fake_bin).expect("create fake bin");

    let codex_path = fake_bin.join("codex");
    fs::write(&codex_path, "#!/bin/sh\nprintf 'fake-codex:%s\\n' \"$*\"\n")
        .expect("write codex stub");
    make_executable(&codex_path);

    let output = Command::new(wrapper_path())
        .current_dir(&cwd)
        .env("HOME", &home)
        .env("OMX_RUST_BIN", native_binary_path())
        .env("PATH", &fake_bin)
        .arg("--xhigh")
        .arg("--madmax")
        .output()
        .expect("run wrapper launch");

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(output.status.success(), "{stderr}{stdout}");
    assert!(stdout.contains("fake-codex:"));
    assert!(stdout.contains("--dangerously-bypass-approvals-and-sandbox"));
    assert!(stdout.contains("model_reasoning_effort=\"xhigh\""));
    assert!(!stderr.contains("tmux"));
    assert!(!stderr.contains("node"));
}

#[test]
fn bin_wrapper_team_prompt_path_bootstraps_and_shuts_down_without_tmux() {
    let cwd = temp_dir("team");
    let capture_path = cwd.join("worker-env.txt");
    let worker_path = cwd.join("worker-cli.sh");
    fs::write(
        &worker_path,
        format!(
            concat!(
                "#!/bin/sh\n",
                "printf 'worker=%s\\nstate_root=%s\\n' \"$OMX_TEAM_WORKER\" \"$OMX_TEAM_STATE_ROOT\" > '{}'\n",
                "trap 'exit 0' TERM INT\n",
                "while true; do sleep 1; done\n"
            ),
            capture_path.display()
        ),
    )
    .expect("write worker stub");
    make_executable(&worker_path);

    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
    let start = Command::new(wrapper_path())
        .current_dir(&cwd)
        .env("OMX_RUST_BIN", native_binary_path())
        .env("OMX_TEAM_WORKER_CLI", &worker_path)
        .env("OMX_SESSION_ID", "sess-native-wrapper")
        .env("PATH", path)
        .args(["team", "1:executor", "prompt", "wrapper", "proof"])
        .output()
        .expect("run wrapper team start");

    let start_stdout = String::from_utf8(start.stdout).expect("utf8 stdout");
    let start_stderr = String::from_utf8(start.stderr).expect("utf8 stderr");
    assert!(start.status.success(), "{start_stderr}{start_stdout}");
    assert!(start_stdout.contains("Team started: prompt-wrapper-proof"));
    assert!(start_stdout.contains("runtime target: prompt-prompt-wrapper-proof"));
    assert!(
        start_stdout.contains(
            "tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true"
        )
    );

    wait_for_file(&capture_path, Duration::from_secs(3));
    let worker_capture = fs::read_to_string(&capture_path).expect("read worker capture");
    assert!(worker_capture.contains("worker=prompt-wrapper-proof/worker-1"));
    assert!(worker_capture.contains("state_root="));

    let team_root = cwd.join(".omx/state/team/prompt-wrapper-proof");
    let config = fs::read_to_string(team_root.join("config.json")).expect("read config");
    let manifest = fs::read_to_string(team_root.join("manifest.v2.json")).expect("read manifest");
    let layout_state =
        fs::read_to_string(team_root.join("layout-state.json")).expect("read layout state");
    let team_mode_state =
        fs::read_to_string(cwd.join(".omx/state/team-state.json")).expect("read team mode");

    assert!(config.contains("\"worker_launch_mode\": \"prompt\""));
    assert!(config.contains("\"tmux_session\": null"));
    assert!(config.contains("\"hud_pane_id\": null"));
    assert!(config.contains("\"resize_hook_name\": null"));
    assert!(manifest.contains("\"worker_launch_mode\":\"prompt\""));
    assert!(manifest.contains("\"tmux_session\": null"));
    assert!(layout_state.contains("\"no_tmux\": true"));
    assert!(team_mode_state.contains("\"no_tmux\": true"));

    let shutdown = Command::new(wrapper_path())
        .current_dir(&cwd)
        .env("OMX_RUST_BIN", native_binary_path())
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .args(["team", "shutdown", "prompt-wrapper-proof", "--force"])
        .output()
        .expect("run wrapper team shutdown");

    let shutdown_stdout = String::from_utf8(shutdown.stdout).expect("utf8 shutdown stdout");
    let shutdown_stderr = String::from_utf8(shutdown.stderr).expect("utf8 shutdown stderr");
    assert!(
        shutdown.status.success(),
        "{shutdown_stderr}{shutdown_stdout}"
    );
    assert_eq!(
        shutdown_stdout,
        "Team shutdown complete: prompt-wrapper-proof\n"
    );
    assert!(!team_root.exists());
}
