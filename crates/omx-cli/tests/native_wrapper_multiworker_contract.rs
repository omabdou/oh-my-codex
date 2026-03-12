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

fn wait_for_all(paths: &[PathBuf], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if paths.iter().all(|path| path.exists()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let missing = paths
        .iter()
        .filter(|path| !path.exists())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    panic!("timed out waiting for: {missing}");
}

#[test]
fn bin_wrapper_team_prompt_multiworker_bootstraps_distinct_lane_state_without_tmux() {
    let cwd = temp_dir("team-multiworker");
    let capture_dir = cwd.join("captures");
    let worker_path = cwd.join("worker-cli.sh");
    fs::create_dir_all(&capture_dir).expect("create capture dir");
    fs::write(
        &worker_path,
        format!(
            concat!(
                "#!/bin/sh\n",
                "safe_worker=$(printf '%s' \"$OMX_TEAM_WORKER\" | tr '/' '_')\n",
                "printf 'worker=%s\\nstate_root=%s\\n' \"$OMX_TEAM_WORKER\" \"$OMX_TEAM_STATE_ROOT\" > '{}'/\"$safe_worker\".txt\n",
                "trap 'exit 0' TERM INT\n",
                "while true; do sleep 1; done\n"
            ),
            capture_dir.display()
        ),
    )
    .expect("write worker stub");
    make_executable(&worker_path);

    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
    let start = Command::new(wrapper_path())
        .current_dir(&cwd)
        .env("OMX_RUST_BIN", native_binary_path())
        .env("OMX_TEAM_WORKER_CLI", &worker_path)
        .env("OMX_SESSION_ID", "sess-native-team-multi")
        .env("PATH", path)
        .args(["team", "2:executor", "bootstrap", "native", "team"])
        .output()
        .expect("run wrapper team start");

    let start_stdout = String::from_utf8(start.stdout).expect("utf8 stdout");
    let start_stderr = String::from_utf8(start.stderr).expect("utf8 stderr");
    assert!(start.status.success(), "{start_stderr}{start_stdout}");
    assert!(start_stdout.contains("Team started: bootstrap-native-team"));
    assert!(start_stdout.contains("runtime target: prompt-bootstrap-native-team"));
    assert!(start_stdout
        .contains("tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true"));

    let worker_one_capture = capture_dir.join("bootstrap-native-team_worker-1.txt");
    let worker_two_capture = capture_dir.join("bootstrap-native-team_worker-2.txt");
    wait_for_all(
        &[worker_one_capture.clone(), worker_two_capture.clone()],
        Duration::from_secs(3),
    );

    let worker_one_env = fs::read_to_string(&worker_one_capture).expect("read worker 1 capture");
    let worker_two_env = fs::read_to_string(&worker_two_capture).expect("read worker 2 capture");
    assert!(worker_one_env.contains("worker=bootstrap-native-team/worker-1"));
    assert!(worker_one_env.contains("state_root="));
    assert!(worker_two_env.contains("worker=bootstrap-native-team/worker-2"));
    assert!(worker_two_env.contains("state_root="));

    let team_root = cwd.join(".omx/state/team/bootstrap-native-team");
    let config = fs::read_to_string(team_root.join("config.json")).expect("read config");
    let manifest = fs::read_to_string(team_root.join("manifest.v2.json")).expect("read manifest");
    let layout_state =
        fs::read_to_string(team_root.join("layout-state.json")).expect("read layout state");
    let team_mode_state =
        fs::read_to_string(cwd.join(".omx/state/team-state.json")).expect("read team mode");
    let task_one = fs::read_to_string(team_root.join("tasks/task-1.json")).expect("read task 1");
    let task_two = fs::read_to_string(team_root.join("tasks/task-2.json")).expect("read task 2");
    let identity_one = fs::read_to_string(team_root.join("workers/worker-1/identity.json"))
        .expect("read worker 1 identity");
    let identity_two = fs::read_to_string(team_root.join("workers/worker-2/identity.json"))
        .expect("read worker 2 identity");
    let inbox_one =
        fs::read_to_string(team_root.join("workers/worker-1/inbox.md")).expect("read inbox 1");
    let inbox_two =
        fs::read_to_string(team_root.join("workers/worker-2/inbox.md")).expect("read inbox 2");
    let launch_one =
        fs::read_to_string(team_root.join("workers/worker-1/launch.json")).expect("read launch 1");
    let launch_two =
        fs::read_to_string(team_root.join("workers/worker-2/launch.json")).expect("read launch 2");

    assert!(config.contains("\"worker_launch_mode\": \"prompt\""));
    assert!(config.contains("\"worker_count\": 2"));
    assert!(config.contains("\"tmux_session\": null"));
    assert!(config.contains("\"leader_pane_id\": null"));
    assert!(config.contains("\"hud_pane_id\": null"));
    assert!(config.contains("\"resize_hook_name\": null"));
    assert!(config.contains("\"workspace_mode\": \"single\""));
    assert!(config.contains("\"assigned_tasks\":[\"1\"]"));
    assert!(config.contains("\"assigned_tasks\":[\"2\"]"));
    assert!(config.contains("\"worker-1"));
    assert!(config.contains("\"worker-2"));

    assert!(manifest.contains("\"worker_launch_mode\":\"prompt\""));
    assert!(manifest.contains("\"worker_count\": 2"));
    assert!(manifest.contains("\"tmux_session\": null"));
    assert!(manifest.contains("\"worker-1"));
    assert!(manifest.contains("\"worker-2"));

    assert!(layout_state.contains("\"mode\": \"native_equivalent\""));
    assert!(layout_state.contains("\"slots\": 2"));
    assert!(layout_state.contains("\"no_tmux\": true"));
    assert!(team_mode_state.contains("\"layout_mode\": \"native_equivalent\""));
    assert!(team_mode_state.contains("\"agent_count\": 2"));
    assert!(team_mode_state.contains("\"no_tmux\": true"));

    assert!(task_one.contains("\"id\":\"1\""));
    assert!(task_one.contains("\"subject\":\"bootstrap native team [lane 1]\""));
    assert!(task_one.contains("\"owner\":\"worker-1\""));
    assert!(task_two.contains("\"id\":\"2\""));
    assert!(task_two.contains("\"subject\":\"bootstrap native team [lane 2]\""));
    assert!(task_two.contains("\"owner\":\"worker-2\""));

    assert!(identity_one.contains("\"assigned_tasks\":[\"1\"]"));
    assert!(identity_two.contains("\"assigned_tasks\":[\"2\"]"));
    assert!(identity_one.contains(&format!("\"working_dir\":\"{}\"", cwd.display())));
    assert!(identity_two.contains(&format!("\"working_dir\":\"{}\"", cwd.display())));
    assert!(identity_one.contains(&format!(
        "\"team_state_root\":\"{}\"",
        cwd.join(".omx/state").display()
    )));
    assert!(identity_two.contains(&format!(
        "\"team_state_root\":\"{}\"",
        cwd.join(".omx/state").display()
    )));

    assert!(inbox_one.contains("**Task 1**: bootstrap native team [lane 1]"));
    assert!(inbox_two.contains("**Task 2**: bootstrap native team [lane 2]"));
    assert!(inbox_one.contains("Verification:"));
    assert!(inbox_two.contains("Verification:"));
    assert!(inbox_one.contains(&format!(
        "{}/team/bootstrap-native-team/tasks/task-1.json",
        cwd.join(".omx/state").display()
    )));
    assert!(inbox_two.contains(&format!(
        "{}/team/bootstrap-native-team/tasks/task-2.json",
        cwd.join(".omx/state").display()
    )));

    assert!(launch_one.contains(&format!("\"worker_cli\": \"{}\"", worker_path.display())));
    assert!(launch_two.contains(&format!("\"worker_cli\": \"{}\"", worker_path.display())));
    assert!(launch_one.contains("\"task_id\": \"1\""));
    assert!(launch_two.contains("\"task_id\": \"2\""));
    assert!(launch_one.contains("--dangerously-bypass-approvals-and-sandbox"));
    assert!(launch_two.contains("--dangerously-bypass-approvals-and-sandbox"));

    let shutdown = Command::new(wrapper_path())
        .current_dir(&cwd)
        .env("OMX_RUST_BIN", native_binary_path())
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .args(["team", "shutdown", "bootstrap-native-team", "--force"])
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
        "Team shutdown complete: bootstrap-native-team\n"
    );
    assert!(!team_root.exists());
}
