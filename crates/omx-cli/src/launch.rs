use omx_process::SpawnErrorKind;
use omx_process::process_bridge::{CommandSpec, Platform, ProcessBridge, StdioMode};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

const MADMAX_FLAG: &str = "--madmax";
const CODEX_BYPASS_FLAG: &str = "--dangerously-bypass-approvals-and-sandbox";
const HIGH_REASONING_FLAG: &str = "--high";
const XHIGH_REASONING_FLAG: &str = "--xhigh";
const SPARK_FLAG: &str = "--spark";
const MADMAX_SPARK_FLAG: &str = "--madmax-spark";
const CONFIG_FLAG: &str = "-c";
const REASONING_KEY: &str = "model_reasoning_effort";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchExecution {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchError(String);

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LaunchError {}

#[allow(clippy::missing_errors_doc)]
pub fn run_launch(
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
    help_output: &str,
) -> Result<LaunchExecution, LaunchError> {
    run_launch_with_stdio(args, cwd, env, help_output, StdioMode::Inherit)
}

fn run_launch_with_stdio(
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
    help_output: &str,
    stdio_mode: StdioMode,
) -> Result<LaunchExecution, LaunchError> {
    if matches!(
        args.first().map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        return Ok(LaunchExecution {
            stdout: help_output.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
        });
    }

    let bridge = ProcessBridge::new(Platform::detect(), env.clone());
    let mut spec = CommandSpec::new("codex");
    spec.args = normalize_launch_args(args)
        .into_iter()
        .map(OsString::from)
        .collect();
    spec.cwd = Some(cwd.to_path_buf());
    spec.stdio_mode = stdio_mode;

    let result = bridge.run(&spec);
    if let Some(kind) = result.spawn_error_kind {
        let message = match kind {
            SpawnErrorKind::Missing => {
                "[omx] failed to launch codex: executable not found in PATH".to_string()
            }
            SpawnErrorKind::Blocked => "[omx] failed to launch codex: executable is present but blocked in the current environment".to_string(),
            SpawnErrorKind::Error => "[omx] failed to launch codex".to_string(),
        };
        return Ok(LaunchExecution {
            stdout: Vec::new(),
            stderr: format!("{message}\n").into_bytes(),
            exit_code: 1,
        });
    }

    let mut stderr = result.stderr;
    let exit_code = result.status_code.unwrap_or(1);
    if let Some(signal) = result.terminating_signal {
        let signal_message = format!("[omx] codex exited due to signal {signal}\n");
        stderr.extend_from_slice(signal_message.as_bytes());
    }

    Ok(LaunchExecution {
        stdout: result.stdout,
        stderr,
        exit_code,
    })
}

fn normalize_launch_args(args: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut wants_bypass = false;
    let mut has_bypass = false;
    let mut reasoning_mode: Option<&str> = None;
    let mut index = 0_usize;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "-w" | "--worktree" => {
                if args
                    .get(index + 1)
                    .is_some_and(|value| !value.starts_with('-'))
                {
                    index += 1;
                }
            }
            value if value.starts_with("--worktree=") => {}
            MADMAX_FLAG => wants_bypass = true,
            CODEX_BYPASS_FLAG => {
                wants_bypass = true;
                if !has_bypass {
                    normalized.push(arg.to_string());
                    has_bypass = true;
                }
            }
            HIGH_REASONING_FLAG => reasoning_mode = Some("high"),
            XHIGH_REASONING_FLAG => reasoning_mode = Some("xhigh"),
            SPARK_FLAG => {}
            MADMAX_SPARK_FLAG => wants_bypass = true,
            "--notify-temp" | "--discord" | "--slack" | "--telegram" => {}
            "--custom" => {
                if args
                    .get(index + 1)
                    .is_some_and(|value| !value.starts_with('-'))
                {
                    index += 1;
                }
            }
            value if value.starts_with("--custom=") => {}
            _ => normalized.push(arg.to_string()),
        }
        index += 1;
    }

    if wants_bypass && !has_bypass {
        normalized.push(CODEX_BYPASS_FLAG.to_string());
    }
    if let Some(mode) = reasoning_mode {
        normalized.push(CONFIG_FLAG.to_string());
        normalized.push(format!("{REASONING_KEY}=\"{mode}\""));
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::{normalize_launch_args, run_launch_with_stdio};
    use omx_process::process_bridge::StdioMode;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    const HELP: &str = "top-level help\n";

    #[test]
    fn prints_top_level_help_for_help_variants() {
        let cwd = std::env::current_dir().expect("cwd");
        let env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        for args in [
            vec!["--help".to_string()],
            vec!["-h".to_string()],
            vec!["help".to_string()],
        ] {
            let result = run_launch_with_stdio(&args, &cwd, &env, HELP, StdioMode::Capture)
                .expect("launch help");
            assert_eq!(result.stdout, HELP.as_bytes());
            assert!(result.stderr.is_empty());
            assert_eq!(result.exit_code, 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn launches_codex_directly_without_tmux_when_available() {
        let cwd = temp_dir("launch-direct");
        let fake_bin = cwd.join("bin");
        fs::create_dir_all(&fake_bin).expect("create bin");
        let codex_path = fake_bin.join("codex");
        fs::write(
            &codex_path,
            "#!/bin/sh\nprintf 'fake-codex cwd=%s args=%s\\n' \"$PWD\" \"$*\"\n",
        )
        .expect("write codex");
        make_executable(&codex_path);

        let env = env_with_path(&fake_bin);
        let result = run_launch_with_stdio(
            &["--model".to_string(), "gpt-5".to_string()],
            &cwd,
            &env,
            HELP,
            StdioMode::Capture,
        )
        .expect("launch direct");

        let stdout = String::from_utf8(result.stdout).expect("utf8 stdout");
        assert_eq!(result.exit_code, 0);
        assert!(result.stderr.is_empty());
        assert!(stdout.contains("fake-codex"));
        assert!(stdout.contains(&format!("cwd={}", cwd.display())));
        assert!(stdout.contains("args=--model gpt-5"));
    }

    #[test]
    fn reports_missing_codex_executable_in_path() {
        let cwd = std::env::current_dir().expect("cwd");
        let env = BTreeMap::from([(OsString::from("PATH"), OsString::from(""))]);
        let result = run_launch_with_stdio(&[], &cwd, &env, HELP, StdioMode::Capture)
            .expect("missing codex handled");
        let stderr = String::from_utf8(result.stderr).expect("utf8 stderr");
        assert_eq!(result.exit_code, 1);
        assert!(stderr.contains("failed to launch codex"));
        assert!(stderr.contains("executable not found in PATH"));
    }

    #[test]
    fn normalizes_launch_shorthand_flags_to_codex_args() {
        assert_eq!(
            normalize_launch_args(&["--xhigh".to_string(), "--madmax".to_string()]),
            vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-c".to_string(),
                "model_reasoning_effort=\"xhigh\"".to_string()
            ]
        );
    }

    #[test]
    fn strips_worker_only_and_notify_temp_flags_from_leader_args() {
        assert_eq!(
            normalize_launch_args(&[
                "--notify-temp".to_string(),
                "--discord".to_string(),
                "--custom".to_string(),
                "openclaw:ops".to_string(),
                "--spark".to_string(),
                "--model".to_string(),
                "gpt-5".to_string()
            ]),
            vec!["--model".to_string(), "gpt-5".to_string()]
        );
    }

    #[test]
    fn strips_worktree_flags_before_launching_codex() {
        assert_eq!(
            normalize_launch_args(&[
                "--worktree".to_string(),
                "feature/demo".to_string(),
                "--yolo".to_string()
            ]),
            vec!["--yolo".to_string()]
        );
        assert_eq!(
            normalize_launch_args(&[
                "--worktree=feature/demo".to_string(),
                "--model".to_string(),
                "gpt-5".to_string()
            ]),
            vec!["--model".to_string(), "gpt-5".to_string()]
        );
    }

    #[cfg(unix)]
    fn temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("omx-launch-{label}-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[cfg(unix)]
    fn env_with_path(fake_bin: &PathBuf) -> BTreeMap<OsString, OsString> {
        let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        let mut path = fake_bin.as_os_str().to_os_string();
        if let Some(existing) = std::env::var_os("PATH") {
            path.push(OsString::from(":"));
            path.push(existing);
        }
        env.insert(OsString::from("PATH"), path);
        env
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path).expect("metadata");
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }
}
