use crate::session_state::{
    extract_json_string_field, read_current_session_id, resolve_state_root,
};
use crate::team_layout::{
    HudModeOverride, deactivate_team_mode_state, sync_prompt_layout_from_state,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::fs::File;
use std::io::{self, Write as IoWrite};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamExecution {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamError(String);

impl TeamError {
    fn runtime(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for TeamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TeamError {}

const TEAM_HELP: &str = concat!(
    "Usage: omx team [ralph] [N:agent-type] \"<task description>\"\n",
    "       omx team status <team-name>\n",
    "       omx team await <team-name> [--timeout-ms <ms>] [--after-event-id <id>] [--json]\n",
    "       omx team resume <team-name>\n",
    "       omx team shutdown <team-name> [--force] [--ralph]\n",
    "       omx team api <operation> [--input <json>] [--json]\n",
    "       omx team api --help\n",
    "\n",
    "Examples:\n",
    "  omx team 3:executor \"fix failing tests\"\n",
    "  omx team status my-team\n",
    "  omx team api send-message --input '{\"team_name\":\"my-team\",\"from_worker\":\"worker-1\",\"to_worker\":\"leader-fixed\",\"body\":\"ACK\"}' --json\n",
);

const TEAM_API_HELP: &str = concat!(
    "Usage: omx team api <operation> [--input <json>] [--json]\n",
    "       omx team api <operation> --help\n",
    "\n",
    "Supported operations:\n",
    "  send-message\n",
    "  broadcast\n",
    "  mailbox-list\n",
    "  mailbox-mark-delivered\n",
    "  mailbox-mark-notified\n",
    "  create-task\n",
    "  read-task\n",
    "  list-tasks\n",
    "  update-task\n",
    "  claim-task\n",
    "  transition-task-status\n",
    "  release-task-claim\n",
    "  read-config\n",
    "  read-manifest\n",
    "  read-worker-status\n",
    "  read-worker-heartbeat\n",
    "  update-worker-heartbeat\n",
    "  write-worker-inbox\n",
    "  write-worker-identity\n",
    "  append-event\n",
    "  read-events\n",
    "  await-event\n",
    "  read-idle-state\n",
    "  read-stall-state\n",
    "  get-summary\n",
    "  cleanup\n",
    "  write-shutdown-request\n",
    "  read-shutdown-ack\n",
    "  read-monitor-snapshot\n",
    "  write-monitor-snapshot\n",
    "  read-task-approval\n",
    "  write-task-approval\n",
    "\n",
    "Examples:\n",
    "  omx team api list-tasks --input '{\"team_name\":\"my-team\"}' --json\n",
    "  omx team api claim-task --input '{\"team_name\":\"my-team\",\"task_id\":\"1\",\"worker\":\"worker-1\",\"expected_version\":1}' --json\n",
);

const DEFAULT_TEAM_WORKER_COUNT: usize = 3;
const DEFAULT_TEAM_MAX_WORKERS: usize = 20;
const DEFAULT_TEAM_DISPATCH_ACK_TIMEOUT_MS: u64 = 3000;

struct ParsedTeamStartArgs {
    linked_ralph: bool,
    worker_count: usize,
    agent_type: String,
    task: String,
    team_name: String,
}

struct SpawnedPromptWorker {
    pid: u64,
    worker_cli: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeLayoutEvidence {
    runtime_target: String,
    worker_launch_mode: String,
    display_mode: String,
    spawn_strategy: String,
    reflow_strategy: String,
    hud_strategy: String,
    tmux_required: bool,
    tmux_session: Option<String>,
    hud_pane_id: Option<String>,
    resize_hook_name: Option<String>,
    resize_hook_target: Option<String>,
    no_tmux_proof: bool,
}

#[allow(clippy::missing_errors_doc)]
pub fn run_team(
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<TeamExecution, TeamError> {
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h" | "help") {
        return Ok(stdout_only(TEAM_HELP));
    }

    if args[0] == "api" {
        if args.len() == 1 || matches!(args[1].as_str(), "--help" | "-h" | "help") {
            return Ok(stdout_only(TEAM_API_HELP));
        }
        return run_team_api(&args[1..], cwd);
    }

    if args[0] == "status" {
        let Some(team_name) = args.get(1) else {
            return Err(TeamError::runtime("Usage: omx team status <team-name>"));
        };
        return run_team_status(team_name, cwd, env);
    }

    if args[0] == "await" {
        return run_team_await(&args[1..], cwd);
    }

    if args[0] == "resume" {
        return run_team_resume(&args[1..], cwd, env);
    }

    if args[0] == "shutdown" {
        return run_team_shutdown(&args[1..], cwd, env);
    }

    run_team_start(args, cwd, env)
}

fn stdout_only(text: &str) -> TeamExecution {
    TeamExecution {
        stdout: text.as_bytes().to_vec(),
        stderr: Vec::new(),
        exit_code: 0,
    }
}

fn execution(stdout: String, stderr: String, exit_code: i32) -> TeamExecution {
    TeamExecution {
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
        exit_code,
    }
}

fn run_team_start(
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<TeamExecution, TeamError> {
    let parsed = parse_team_start_args(args)?;
    let team_root = team_root(cwd, &parsed.team_name);
    if team_root.exists() {
        return Err(TeamError::runtime(format!(
            "Team state already exists for {}",
            parsed.team_name
        )));
    }

    initialize_team_state(&team_root, &parsed, cwd, env)?;
    let summary = summarize_tasks(&team_root.join("tasks"))?;
    let layout = sync_prompt_layout_if_available(
        &team_root,
        cwd,
        "spawn",
        HudModeOverride::Inline,
        Some(env),
    );
    let runtime = read_runtime_layout_evidence_for_team_root(&team_root)?;

    let mut stdout = String::new();
    let _ = writeln!(stdout, "Team started: {}", parsed.team_name);
    let _ = writeln!(stdout, "runtime target: prompt-{}", parsed.team_name);
    let _ = writeln!(stdout, "workers: {}", parsed.worker_count);
    let _ = writeln!(stdout, "agent_type: {}", parsed.agent_type);
    let _ = writeln!(stdout, "{}", render_runtime_layout_line(&runtime));
    let _ = writeln!(stdout, "{}", render_runtime_tmux_line(&runtime));
    if let Some(layout) = layout.as_ref() {
        let _ = writeln!(stdout, "{}", layout.summary_line());
        let _ = writeln!(stdout, "{}", layout.proof_line());
    }
    let _ = writeln!(stdout, "linked_ralph={}", parsed.linked_ralph);
    let _ = writeln!(
        stdout,
        "tasks: total={} pending={} blocked={} in_progress={} completed={} failed={}",
        summary.total,
        summary.pending,
        summary.blocked,
        summary.in_progress,
        summary.completed,
        summary.failed
    );
    Ok(stdout_only(&stdout))
}

fn parse_team_start_args(args: &[String]) -> Result<ParsedTeamStartArgs, TeamError> {
    let mut tokens = args.to_vec();
    let mut linked_ralph = false;
    let mut worker_count = DEFAULT_TEAM_WORKER_COUNT;
    let mut agent_type = String::from("executor");

    if tokens
        .first()
        .is_some_and(|value| value.eq_ignore_ascii_case("ralph"))
    {
        linked_ralph = true;
        tokens.remove(0);
    }

    if let Some(first) = tokens.first().cloned() {
        if let Some((count, role)) = parse_worker_count_token(&first)? {
            worker_count = count;
            if let Some(role) = role {
                agent_type = role;
            }
            tokens.remove(0);
        }
    }

    let task = tokens.join(" ").trim().to_string();
    if task.is_empty() {
        return Err(TeamError::runtime(
            "Usage: omx team [ralph] [N:agent-type] \"<task description>\"",
        ));
    }

    Ok(ParsedTeamStartArgs {
        linked_ralph,
        worker_count,
        agent_type,
        team_name: sanitize_team_name(&task),
        task,
    })
}

fn parse_worker_count_token(token: &str) -> Result<Option<(usize, Option<String>)>, TeamError> {
    let mut parts = token.splitn(2, ':');
    let count_part = parts.next().unwrap_or_default();
    if !count_part.chars().all(|ch| ch.is_ascii_digit()) || count_part.is_empty() {
        return Ok(None);
    }
    let count = count_part.parse::<usize>().map_err(|_| {
        TeamError::runtime(format!(
            "Invalid worker count \"{count_part}\". Expected 1-{DEFAULT_TEAM_MAX_WORKERS}."
        ))
    })?;
    if count == 0 || count > DEFAULT_TEAM_MAX_WORKERS {
        return Err(TeamError::runtime(format!(
            "Invalid worker count \"{count_part}\". Expected 1-{DEFAULT_TEAM_MAX_WORKERS}."
        )));
    }
    let role = parts.next().map(|value| value.trim().to_ascii_lowercase());
    Ok(Some((count, role.filter(|value| !value.is_empty()))))
}

fn sanitize_team_name(task: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in task.chars() {
        let normalized = ch.to_ascii_lowercase();
        if normalized.is_ascii_alphanumeric() {
            out.push(normalized);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let mut team_name = if trimmed.is_empty() {
        "team".to_string()
    } else {
        trimmed.to_string()
    };
    if team_name.len() > 30 {
        team_name.truncate(30);
        team_name = team_name.trim_matches('-').to_string();
    }
    if team_name.is_empty() {
        "team".to_string()
    } else {
        team_name
    }
}

fn resolve_prompt_worker_cli(env: &BTreeMap<OsString, OsString>) -> String {
    env.get(&OsString::from("OMX_TEAM_WORKER_CLI"))
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
        .unwrap_or_else(|| "codex".to_string())
}

fn split_worker_launch_args(raw: Option<&OsString>) -> Vec<String> {
    raw.map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default()
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect()
}

fn spawn_prompt_worker_process(
    team_root: &Path,
    team_name: &str,
    worker_name: &str,
    worker_index: usize,
    task: &str,
    task_id: &str,
    cwd: &Path,
    team_state_root: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<SpawnedPromptWorker, TeamError> {
    let worker_cli = resolve_prompt_worker_cli(env);
    let worker_dir = team_root.join("workers").join(worker_name);
    let stdout_log = worker_dir.join("stdout.log");
    let stderr_log = worker_dir.join("stderr.log");
    let launch_meta = worker_dir.join("launch.json");
    let mut args =
        split_worker_launch_args(env.get(&OsString::from("OMX_TEAM_WORKER_LAUNCH_ARGS")));
    if worker_cli == "codex"
        && !args
            .iter()
            .any(|value| value == "--dangerously-bypass-approvals-and-sandbox")
    {
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    }
    if worker_cli == "codex" {
        args.push(build_prompt_worker_bootstrap_prompt(
            team_name,
            worker_name,
            task_id,
            task,
            team_root,
        ));
    } else if args.is_empty() {
        args.push(task.to_string());
    }
    write_atomic_text(
        &launch_meta,
        &format!(
            "{{\n  \"worker_cli\": \"{}\",\n  \"args\": {},\n  \"task_id\": \"{}\"\n}}\n",
            escape_json_string(&worker_cli),
            format_string_array_json(&args),
            escape_json_string(task_id),
        ),
    )?;

    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("setsid");
        command.arg(&worker_cli);
        if worker_cli == "codex" {
            command.arg("exec");
        }
        command.args(&args);
        command
    };
    #[cfg(not(unix))]
    let mut command = {
        let mut command = Command::new(&worker_cli);
        if worker_cli == "codex" {
            command.arg("exec");
        }
        command.args(&args);
        command
    };
    command.current_dir(cwd);
    command.stdin(Stdio::null());
    command.stdout(Stdio::from(File::create(&stdout_log).map_err(|error| {
        TeamError::runtime(format!(
            "failed to create {}: {error}",
            stdout_log.display()
        ))
    })?));
    command.stderr(Stdio::from(File::create(&stderr_log).map_err(|error| {
        TeamError::runtime(format!(
            "failed to create {}: {error}",
            stderr_log.display()
        ))
    })?));
    command.envs(env.iter());
    command.env("OMX_TEAM_WORKER", format!("{team_name}/{worker_name}"));
    command.env("OMX_TEAM_STATE_ROOT", team_state_root.display().to_string());
    command.env("OMX_TEAM_LEADER_CWD", cwd.display().to_string());
    command.env("OMX_TEAM_WORKER_INDEX", worker_index.to_string());
    command.env("OMX_TEAM_TASK", task);
    command.env("OMX_TEAM_WORKER_CLI", &worker_cli);
    let pid = command
        .spawn()
        .map_err(|error| {
            TeamError::runtime(format!(
                "failed to spawn prompt worker {} via {}: {}",
                worker_name, worker_cli, error
            ))
        })?
        .id() as u64;
    write_atomic_text(
        &team_root
            .join("workers")
            .join(worker_name)
            .join("status.json"),
        "{\n  \"state\": \"idle\",\n  \"updated_at\": \"bootstrap\"\n}\n",
    )?;
    Ok(SpawnedPromptWorker { pid, worker_cli })
}

fn build_prompt_worker_inbox(
    worker_name: &str,
    team_name: &str,
    role: &str,
    task_id: &str,
    subject: &str,
    description: &str,
    team_state_root: &Path,
    leader_cwd: &Path,
) -> String {
    format!(
        concat!(
            "# Worker Assignment: {worker_name}\n\n",
            "**Team:** {team_name}\n",
            "**Role:** {role}\n",
            "**Worker Name:** {worker_name}\n\n",
            "## Your Assigned Tasks\n\n",
            "- **Task {task_id}**: {subject}\n",
            "  Description: {description}\n",
            "  Status: pending\n",
            "  Role: {role}\n\n",
            "## Instructions\n\n",
            "1. Load and follow the worker skill from the first existing path:\n",
            "   - `${{CODEX_HOME:-~/.codex}}/skills/worker/SKILL.md`\n",
            "   - `~/.agents/skills/worker/SKILL.md`\n",
            "   - `{leader_cwd}/.agents/skills/worker/SKILL.md`\n",
            "   - `{leader_cwd}/skills/worker/SKILL.md` (repo fallback)\n",
            "2. Send startup ACK to the lead mailbox BEFORE any task work:\n\n",
            "   `omx team api send-message --input \"{{\\\"team_name\\\":\\\"{team_name}\\\",\\\"from_worker\\\":\\\"{worker_name}\\\",\\\"to_worker\\\":\\\"leader-fixed\\\",\\\"body\\\":\\\"ACK: {worker_name} initialized\\\"}}\" --json`\n\n",
            "3. Resolve canonical team state root in this order: `OMX_TEAM_STATE_ROOT` env -> worker identity `team_state_root` -> config/manifest `team_state_root` -> local cwd fallback.\n",
            "4. Read the task file at `{team_state_root}/team/{team_name}/tasks/task-{task_id}.json`\n",
            "5. Request a claim via `omx team api claim-task --json`\n",
            "6. Complete the work described in the task\n",
            "7. Complete/fail it via `omx team api transition-task-status --json` from `\"in_progress\"` to `\"completed\"` or `\"failed\"`\n",
            "8. Use `omx team api release-task-claim --json` only for rollback to `pending`\n",
            "9. Write `{{\"state\":\"idle\",\"updated_at\":\"<current ISO timestamp>\"}}` to `{team_state_root}/team/{team_name}/workers/{worker_name}/status.json`\n\n",
            "## Verification Requirements\n\n",
            "When marking completion, include structured verification evidence in your task result:\n",
            "- `Verification:`\n",
            "- One or more PASS/FAIL checks with command/output references\n"
        ),
        worker_name = worker_name,
        team_name = team_name,
        role = role,
        task_id = task_id,
        subject = subject,
        description = description,
        team_state_root = team_state_root.display(),
        leader_cwd = leader_cwd.display(),
    )
}

fn build_prompt_worker_bootstrap_prompt(
    team_name: &str,
    worker_name: &str,
    task_id: &str,
    task: &str,
    team_root: &Path,
) -> String {
    let inbox_path = team_root.join("workers").join(worker_name).join("inbox.md");
    let task_path = team_root.join("tasks").join(format!("task-{task_id}.json"));
    format!(
        concat!(
            "You are {worker_name} in OMX team {team_name}. ",
            "Read inbox at {inbox_path}. ",
            "Send ACK with: omx team api send-message --input '{{\"team_name\":\"{team_name}\",\"from_worker\":\"{worker_name}\",\"to_worker\":\"leader-fixed\",\"body\":\"ACK: {worker_name} initialized\"}}' --json. ",
            "Then claim task {task_id} with: omx team api claim-task --input '{{\"team_name\":\"{team_name}\",\"task_id\":\"{task_id}\",\"worker\":\"{worker_name}\"}}' --json. ",
            "Read task file {task_path}. ",
            "Execute the assigned work: {task}. ",
            "When done, transition the task with omx team api transition-task-status and include verification evidence."
        ),
        worker_name = worker_name,
        team_name = team_name,
        inbox_path = inbox_path.display(),
        task_id = task_id,
        task_path = task_path.display(),
        task = task,
    )
}

fn terminate_worker_pid(pid: u64) -> Result<(), TeamError> {
    if pid == 0 {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| {
                TeamError::runtime(format!("failed to send SIGTERM to {pid}: {error}"))
            })?;
        if !status.success() && status.code() != Some(1) {
            return Err(TeamError::runtime(format!(
                "failed to terminate worker pid {pid}: exit={:?}",
                status.code()
            )));
        }
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            let probe = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|error| TeamError::runtime(format!("failed to probe {pid}: {error}")))?;
            if !probe.success() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        let kill_status = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| {
                TeamError::runtime(format!("failed to send SIGKILL to {pid}: {error}"))
            })?;
        if kill_status.success() || kill_status.code() == Some(1) {
            return Ok(());
        }
        return Err(TeamError::runtime(format!(
            "failed to force terminate worker pid {pid}: exit={:?}",
            kill_status.code()
        )));
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .map_err(|error| TeamError::runtime(format!("failed to terminate {pid}: {error}")))?;
        if status.success() {
            return Ok(());
        }
        return Err(TeamError::runtime(format!(
            "failed to terminate worker pid {pid}: exit={:?}",
            status.code()
        )));
    }
    #[allow(unreachable_code)]
    Ok(())
}

fn build_workers_json(
    worker_names: &[String],
    agent_type: &str,
    worker_task_ids: &BTreeMap<String, String>,
    working_dir: &str,
    team_state_root: &str,
    spawned_workers: &BTreeMap<String, SpawnedPromptWorker>,
) -> String {
    format!(
        "[{}]",
        worker_names
            .iter()
            .enumerate()
            .map(|(index, worker_name)| {
                let assigned_tasks = worker_task_ids
                    .get(worker_name)
                    .map(|task_id| vec![task_id.clone()])
                    .unwrap_or_default();
                let pid = spawned_workers.get(worker_name).map(|worker| worker.pid);
                let worker_cli = spawned_workers.get(worker_name).map(|worker| worker.worker_cli.as_str());
                let mut raw = format!(
                    "{{\"name\":\"{}\",\"index\":{},\"role\":\"{}\",\"assigned_tasks\":{},\"working_dir\":\"{}\",\"team_state_root\":\"{}\"}}",
                    escape_json_string(worker_name),
                    index + 1,
                    escape_json_string(agent_type),
                    format_string_array_json(&assigned_tasks),
                    escape_json_string(working_dir),
                    escape_json_string(team_state_root),
                );
                if let Some(pid) = pid {
                    raw = upsert_json_number_field(&raw, "pid", pid);
                }
                if let Some(worker_cli) = worker_cli {
                    raw = upsert_json_string_field(&raw, "worker_cli", worker_cli);
                }
                raw
            })
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn initialize_team_state(
    team_root: &Path,
    parsed: &ParsedTeamStartArgs,
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<(), TeamError> {
    fs::create_dir_all(team_root.join("tasks")).map_err(|error| {
        TeamError::runtime(format!("failed to create {}: {error}", team_root.display()))
    })?;
    for dir in ["mailbox", "dispatch", "approvals", "events", "claims"] {
        fs::create_dir_all(team_root.join(dir)).map_err(|error| {
            TeamError::runtime(format!(
                "failed to create {}: {error}",
                team_root.join(dir).display()
            ))
        })?;
    }

    let runtime_session_id = format!("prompt-{}", parsed.team_name);
    let created_at = iso_timestamp();
    let worker_names = (1..=parsed.worker_count)
        .map(|index| format!("worker-{index}"))
        .collect::<Vec<_>>();
    let mut worker_task_ids = BTreeMap::<String, String>::new();
    let mut spawned_workers = BTreeMap::<String, SpawnedPromptWorker>::new();
    let state_root = resolve_state_root(cwd, env);
    let working_dir = cwd.display().to_string();
    let team_state_root = state_root.display().to_string();
    let workers_json = build_workers_json(
        &worker_names,
        &parsed.agent_type,
        &worker_task_ids,
        &working_dir,
        &team_state_root,
        &spawned_workers,
    );

    let approval_mode = env
        .get(&OsString::from("CODEX_APPROVAL_MODE"))
        .or_else(|| env.get(&OsString::from("OMX_APPROVAL_MODE")))
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "never".to_string());
    let sandbox_mode = env
        .get(&OsString::from("CODEX_SANDBOX_MODE"))
        .or_else(|| env.get(&OsString::from("OMX_SANDBOX_MODE")))
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "danger-full-access".to_string());
    let network_access = env
        .get(&OsString::from("CODEX_NETWORK_ACCESS"))
        .or_else(|| env.get(&OsString::from("OMX_NETWORK_ACCESS")))
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(true);

    let leader_session_id = read_current_session_id(&state_root)
        .or_else(|| {
            env.get(&OsString::from("OMX_SESSION_ID"))
                .map(|value| value.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "unknown-session".to_string());

    let config_json = format!(
        concat!(
            "{{\n",
            "  \"name\": \"{}\",\n",
            "  \"task\": \"{}\",\n",
            "  \"agent_type\": \"{}\",\n",
            "  \"worker_launch_mode\": \"prompt\",\n",
            "  \"worker_count\": {},\n",
            "  \"max_workers\": {},\n",
            "  \"workers\": {},\n",
            "  \"created_at\": \"{}\",\n",
            "  \"runtime_session_id\": \"{}\",\n",
            "  \"tmux_session\": null,\n",
            "  \"next_task_id\": 1,\n",
            "  \"leader_cwd\": \"{}\",\n",
            "  \"team_state_root\": \"{}\",\n",
            "  \"workspace_mode\": \"single\",\n",
            "  \"leader_pane_id\": null,\n",
            "  \"hud_pane_id\": null,\n",
            "  \"resize_hook_name\": null,\n",
            "  \"resize_hook_target\": null,\n",
            "  \"next_worker_index\": {}\n",
            "}}\n"
        ),
        escape_json_string(&parsed.team_name),
        escape_json_string(&parsed.task),
        escape_json_string(&parsed.agent_type),
        parsed.worker_count,
        DEFAULT_TEAM_MAX_WORKERS,
        workers_json,
        escape_json_string(&created_at),
        escape_json_string(&runtime_session_id),
        escape_json_string(&working_dir),
        escape_json_string(&team_state_root),
        parsed.worker_count + 1,
    );
    write_atomic_text(&team_root.join("config.json"), &config_json)?;

    let manifest_json = format!(
        concat!(
            "{{\n",
            "  \"schema_version\": 2,\n",
            "  \"name\": \"{}\",\n",
            "  \"task\": \"{}\",\n",
            "  \"leader\": {{\"session_id\":\"{}\",\"worker_id\":\"leader-fixed\",\"role\":\"leader\"}},\n",
            "  \"policy\": {{\"display_mode\":\"auto\",\"worker_launch_mode\":\"prompt\",\"dispatch_mode\":\"hook_preferred_with_fallback\",\"dispatch_ack_timeout_ms\":{},\"delegation_only\":false,\"plan_approval_required\":false,\"nested_teams_allowed\":false,\"one_team_per_leader_session\":true,\"cleanup_requires_all_workers_inactive\":true}},\n",
            "  \"permissions_snapshot\": {{\"approval_mode\":\"{}\",\"sandbox_mode\":\"{}\",\"network_access\":{}}},\n",
            "  \"runtime_session_id\": \"{}\",\n",
            "  \"tmux_session\": null,\n",
            "  \"worker_count\": {},\n",
            "  \"workers\": {},\n",
            "  \"next_task_id\": 1,\n",
            "  \"created_at\": \"{}\",\n",
            "  \"leader_cwd\": \"{}\",\n",
            "  \"team_state_root\": \"{}\",\n",
            "  \"workspace_mode\": \"single\",\n",
            "  \"leader_pane_id\": null,\n",
            "  \"hud_pane_id\": null,\n",
            "  \"resize_hook_name\": null,\n",
            "  \"resize_hook_target\": null,\n",
            "  \"next_worker_index\": {}\n",
            "}}\n"
        ),
        escape_json_string(&parsed.team_name),
        escape_json_string(&parsed.task),
        escape_json_string(&leader_session_id),
        DEFAULT_TEAM_DISPATCH_ACK_TIMEOUT_MS,
        escape_json_string(&approval_mode),
        escape_json_string(&sandbox_mode),
        if network_access { "true" } else { "false" },
        escape_json_string(&runtime_session_id),
        parsed.worker_count,
        workers_json,
        escape_json_string(&created_at),
        escape_json_string(&working_dir),
        escape_json_string(&team_state_root),
        parsed.worker_count + 1,
    );
    write_atomic_text(&team_root.join("manifest.v2.json"), &manifest_json)?;
    write_atomic_text(
        &team_root.join("phase.json"),
        "{\n  \"current_phase\": \"team-exec\"\n}\n",
    )?;
    write_atomic_text(&team_root.join("dispatch/requests.json"), "[]\n")?;

    for (index, worker_name) in worker_names.iter().enumerate() {
        fs::create_dir_all(team_root.join("workers").join(worker_name)).map_err(|error| {
            TeamError::runtime(format!(
                "failed to create {}: {error}",
                team_root.join("workers").join(worker_name).display()
            ))
        })?;
        let lane_subject = if parsed.worker_count == 1 {
            parsed.task.clone()
        } else {
            format!("{} [lane {}]", parsed.task, index + 1)
        };
        let created_task = create_team_task(
            team_root,
            Some(worker_name.clone()),
            &lane_subject,
            &parsed.task,
            None,
            Some(true),
        )?;
        let task_id = extract_json_string_field(&created_task, "id")
            .ok_or_else(|| TeamError::runtime("failed to extract created task id"))?;
        worker_task_ids.insert(worker_name.clone(), task_id);
        let inbox = build_prompt_worker_inbox(
            worker_name,
            &parsed.team_name,
            &parsed.agent_type,
            worker_task_ids
                .get(worker_name)
                .map(String::as_str)
                .ok_or_else(|| {
                    TeamError::runtime(format!("missing task id for {}", worker_name))
                })?,
            &lane_subject,
            &parsed.task,
            &state_root,
            cwd,
        );
        write_worker_inbox(team_root, worker_name, &inbox)?;
    }

    for (index, worker_name) in worker_names.iter().enumerate() {
        match spawn_prompt_worker_process(
            team_root,
            &parsed.team_name,
            worker_name,
            index + 1,
            &parsed.task,
            worker_task_ids
                .get(worker_name)
                .map(String::as_str)
                .ok_or_else(|| {
                    TeamError::runtime(format!("missing task id for {}", worker_name))
                })?,
            cwd,
            &state_root,
            env,
        ) {
            Ok(spawned) => {
                spawned_workers.insert(worker_name.clone(), spawned);
            }
            Err(error) => {
                for worker in spawned_workers.values() {
                    let _ = terminate_worker_pid(worker.pid);
                }
                let _ = remove_team_state(team_root);
                return Err(error);
            }
        }
    }

    let workers_json = build_workers_json(
        &worker_names,
        &parsed.agent_type,
        &worker_task_ids,
        &working_dir,
        &team_state_root,
        &spawned_workers,
    );
    let config_json = fs::read_to_string(team_root.join("config.json")).map_err(|error| {
        TeamError::runtime(format!("failed to read config for worker update: {error}"))
    })?;
    let updated_config_json = upsert_json_raw_field(&config_json, "workers", &workers_json);
    write_atomic_text(
        &team_root.join("config.json"),
        &(updated_config_json + "\n"),
    )?;

    let manifest_json =
        fs::read_to_string(team_root.join("manifest.v2.json")).map_err(|error| {
            TeamError::runtime(format!(
                "failed to read manifest for worker update: {error}"
            ))
        })?;
    let updated_manifest_json = upsert_json_raw_field(&manifest_json, "workers", &workers_json);
    write_atomic_text(
        &team_root.join("manifest.v2.json"),
        &(updated_manifest_json + "\n"),
    )?;

    for (index, worker_name) in worker_names.iter().enumerate() {
        let spawned = spawned_workers.get(worker_name);
        write_worker_identity(
            team_root,
            worker_name,
            (index + 1) as u64,
            &parsed.agent_type,
            worker_task_ids
                .get(worker_name)
                .map(|task_id| vec![task_id.clone()])
                .unwrap_or_default(),
            spawned.map(|worker| worker.pid),
            None,
            Some(working_dir.clone()),
            None,
            None,
            None,
            Some(team_state_root.clone()),
        )?;
    }

    if parsed.linked_ralph {
        append_team_event(
            team_root,
            &parsed.team_name,
            "team_leader_nudge",
            "leader-fixed",
            None,
            None,
            Some("linked_ralph_bootstrap".to_string()),
            None,
            None,
            None,
            Some(parsed.worker_count as u64),
            Some("linked_ralph".to_string()),
        )?;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeamApiOperation {
    SendMessage,
    Broadcast,
    MailboxMarkDelivered,
    MailboxMarkNotified,
    CreateTask,
    ReadConfig,
    ReadManifest,
    ReadWorkerStatus,
    ReadWorkerHeartbeat,
    UpdateWorkerHeartbeat,
    WriteWorkerInbox,
    WriteWorkerIdentity,
    AppendEvent,
    ReadTask,
    ListTasks,
    UpdateTask,
    ClaimTask,
    TransitionTaskStatus,
    ReleaseTaskClaim,
    MailboxList,
    Cleanup,
    WriteShutdownRequest,
    ReadMonitorSnapshot,
    WriteMonitorSnapshot,
    ReadShutdownAck,
    ReadTaskApproval,
    WriteTaskApproval,
    ReadEvents,
    AwaitEvent,
    ReadIdleState,
    ReadStallState,
    GetSummary,
}

impl TeamApiOperation {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "send-message" => Some(Self::SendMessage),
            "broadcast" => Some(Self::Broadcast),
            "mailbox-mark-delivered" => Some(Self::MailboxMarkDelivered),
            "mailbox-mark-notified" => Some(Self::MailboxMarkNotified),
            "create-task" => Some(Self::CreateTask),
            "read-config" => Some(Self::ReadConfig),
            "read-manifest" => Some(Self::ReadManifest),
            "read-worker-status" => Some(Self::ReadWorkerStatus),
            "read-worker-heartbeat" => Some(Self::ReadWorkerHeartbeat),
            "update-worker-heartbeat" => Some(Self::UpdateWorkerHeartbeat),
            "write-worker-inbox" => Some(Self::WriteWorkerInbox),
            "write-worker-identity" => Some(Self::WriteWorkerIdentity),
            "append-event" => Some(Self::AppendEvent),
            "read-task" => Some(Self::ReadTask),
            "list-tasks" => Some(Self::ListTasks),
            "update-task" => Some(Self::UpdateTask),
            "claim-task" => Some(Self::ClaimTask),
            "transition-task-status" => Some(Self::TransitionTaskStatus),
            "release-task-claim" => Some(Self::ReleaseTaskClaim),
            "mailbox-list" => Some(Self::MailboxList),
            "cleanup" => Some(Self::Cleanup),
            "write-shutdown-request" => Some(Self::WriteShutdownRequest),
            "read-monitor-snapshot" => Some(Self::ReadMonitorSnapshot),
            "write-monitor-snapshot" => Some(Self::WriteMonitorSnapshot),
            "read-shutdown-ack" => Some(Self::ReadShutdownAck),
            "read-task-approval" => Some(Self::ReadTaskApproval),
            "write-task-approval" => Some(Self::WriteTaskApproval),
            "read-events" => Some(Self::ReadEvents),
            "await-event" => Some(Self::AwaitEvent),
            "read-idle-state" => Some(Self::ReadIdleState),
            "read-stall-state" => Some(Self::ReadStallState),
            "get-summary" => Some(Self::GetSummary),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::SendMessage => "send-message",
            Self::Broadcast => "broadcast",
            Self::MailboxMarkDelivered => "mailbox-mark-delivered",
            Self::MailboxMarkNotified => "mailbox-mark-notified",
            Self::CreateTask => "create-task",
            Self::ReadConfig => "read-config",
            Self::ReadManifest => "read-manifest",
            Self::ReadWorkerStatus => "read-worker-status",
            Self::ReadWorkerHeartbeat => "read-worker-heartbeat",
            Self::UpdateWorkerHeartbeat => "update-worker-heartbeat",
            Self::WriteWorkerInbox => "write-worker-inbox",
            Self::WriteWorkerIdentity => "write-worker-identity",
            Self::AppendEvent => "append-event",
            Self::ReadTask => "read-task",
            Self::ListTasks => "list-tasks",
            Self::UpdateTask => "update-task",
            Self::ClaimTask => "claim-task",
            Self::TransitionTaskStatus => "transition-task-status",
            Self::ReleaseTaskClaim => "release-task-claim",
            Self::MailboxList => "mailbox-list",
            Self::Cleanup => "cleanup",
            Self::WriteShutdownRequest => "write-shutdown-request",
            Self::ReadMonitorSnapshot => "read-monitor-snapshot",
            Self::WriteMonitorSnapshot => "write-monitor-snapshot",
            Self::ReadShutdownAck => "read-shutdown-ack",
            Self::ReadTaskApproval => "read-task-approval",
            Self::WriteTaskApproval => "write-task-approval",
            Self::ReadEvents => "read-events",
            Self::AwaitEvent => "await-event",
            Self::ReadIdleState => "read-idle-state",
            Self::ReadStallState => "read-stall-state",
            Self::GetSummary => "get-summary",
        }
    }
}

struct ParsedTeamApiArgs {
    operation: TeamApiOperation,
    input: String,
    json: bool,
}

fn run_team_api(args: &[String], cwd: &Path) -> Result<TeamExecution, TeamError> {
    let Some(operation_raw) = args.first() else {
        return Ok(stdout_only(TEAM_API_HELP));
    };
    let Some(operation) = TeamApiOperation::parse(operation_raw) else {
        return Err(TeamError::runtime(format!(
            "Command \"team api {}\" is recognized but not yet implemented in the native Rust CLI.",
            operation_raw
        )));
    };

    let trailing = &args[1..];
    if trailing
        .iter()
        .any(|value| matches!(value.as_str(), "--help" | "-h" | "help"))
    {
        return Ok(stdout_only(&build_team_api_operation_help(operation)));
    }

    let wants_json = trailing.iter().any(|value| value == "--json");
    match parse_team_api_args(operation, trailing) {
        Ok(parsed) => Ok(execute_team_api(parsed, cwd)),
        Err(error) if wants_json => Ok(json_error_execution(
            "omx team api",
            "unknown",
            "invalid_input",
            &error,
        )),
        Err(error) => Err(TeamError::runtime(error)),
    }
}

fn run_team_await(args: &[String], cwd: &Path) -> Result<TeamExecution, TeamError> {
    let Some(team_name) = args.first() else {
        return Err(TeamError::runtime(
            "Usage: omx team await <team-name> [--timeout-ms <ms>] [--after-event-id <id>] [--json]",
        ));
    };

    let mut wants_json = false;
    let mut timeout_ms = 30_000_u64;
    let mut after_event_id = String::new();
    let mut index = 1_usize;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => wants_json = true,
            "--timeout-ms" => {
                let Some(raw_value) = args.get(index + 1) else {
                    return Err(TeamError::runtime("Missing value after --timeout-ms"));
                };
                timeout_ms = raw_value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| TeamError::runtime("timeout-ms must be a positive integer"))?;
                index += 1;
            }
            "--after-event-id" => {
                let Some(raw_value) = args.get(index + 1) else {
                    return Err(TeamError::runtime("Missing value after --after-event-id"));
                };
                after_event_id = raw_value.clone();
                index += 1;
            }
            token => {
                return Err(TeamError::runtime(format!(
                    "Unknown argument for \"omx team await\": {token}"
                )));
            }
        }
        index += 1;
    }

    let team_root = team_root(cwd, team_name);
    if !team_root.exists() {
        if wants_json {
            return Ok(stdout_only(&format!(
                "{{\"team_name\":\"{}\",\"status\":\"missing\",\"cursor\":\"{}\",\"event\":null}}\n",
                escape_json_string(team_name),
                escape_json_string(&after_event_id)
            )));
        }
        return Ok(stdout_only(&format!(
            "No team state found for {team_name}\n"
        )));
    }

    let initial_cursor = if after_event_id.is_empty() {
        latest_event_id(cwd, team_name)?
    } else {
        after_event_id
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let events = read_team_events(
            cwd,
            team_name,
            EventQuery {
                after_event_id: if initial_cursor.is_empty() {
                    None
                } else {
                    Some(initial_cursor.as_str())
                },
                wakeable_only: true,
                event_type: None,
                worker: None,
                task_id: None,
            },
        )?;
        if let Some(event) = events.first() {
            if wants_json {
                return Ok(stdout_only(&format!(
                    "{{\"team_name\":\"{}\",\"status\":\"event\",\"cursor\":\"{}\",\"event\":{}}}\n",
                    escape_json_string(team_name),
                    escape_json_string(&event.event_id),
                    event.raw
                )));
            }

            let mut context = vec![
                format!("team={team_name}"),
                format!("event={}", event.event_type),
                format!("worker={}", event.worker),
            ];
            if let Some(state) = &event.state {
                context.push(format!("state={state}"));
            }
            if let Some(prev_state) = &event.prev_state {
                context.push(format!("prev={prev_state}"));
            }
            if let Some(task_id) = &event.task_id {
                context.push(format!("task={task_id}"));
            }
            context.push(format!("cursor={}", event.event_id));
            return Ok(stdout_only(&format!("{}\n", context.join(" "))));
        }

        if Instant::now() >= deadline {
            if wants_json {
                return Ok(stdout_only(&format!(
                    "{{\"team_name\":\"{}\",\"status\":\"timeout\",\"cursor\":\"{}\",\"event\":null}}\n",
                    escape_json_string(team_name),
                    escape_json_string(&initial_cursor)
                )));
            }
            return Ok(stdout_only(&format!(
                "No new event for {team_name} before timeout ({timeout_ms}ms).\n"
            )));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn run_team_resume(
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<TeamExecution, TeamError> {
    let Some(team_name) = args.first() else {
        return Err(TeamError::runtime("Usage: omx team resume <team-name>"));
    };
    if args.len() > 1 {
        return Err(TeamError::runtime(format!(
            "Unknown argument for \"omx team resume\": {}",
            args[1]
        )));
    }

    let team_root = team_root(cwd, team_name);
    if !team_root.exists()
        || (!team_root.join("config.json").exists() && !team_root.join("manifest.v2.json").exists())
    {
        return Ok(stdout_only(&format!(
            "No resumable team found for {team_name}\n"
        )));
    }

    let phase = read_team_phase(&team_root).unwrap_or_else(|| "unknown".to_string());
    let summary = summarize_tasks(&team_root.join("tasks"))?;
    let workers = collect_team_worker_names(&team_root)?;
    let runtime = read_runtime_layout_evidence_for_team_root(&team_root)?;
    let layout = sync_prompt_layout_if_available(
        &team_root,
        cwd,
        "resume",
        HudModeOverride::Preserve,
        Some(env),
    );
    let mut stdout = String::new();
    let _ = writeln!(stdout, "team={} resumed phase={}", team_name, phase);
    let _ = writeln!(stdout, "runtime target: {}", runtime.runtime_target);
    let _ = writeln!(stdout, "{}", render_runtime_layout_line(&runtime));
    let _ = writeln!(stdout, "{}", render_runtime_tmux_line(&runtime));
    if let Some(layout) = layout.as_ref() {
        let _ = writeln!(stdout, "{}", layout.summary_line());
        let _ = writeln!(stdout, "{}", layout.proof_line());
    }
    let _ = writeln!(stdout, "workers={}", workers.len());
    let _ = writeln!(
        stdout,
        "tasks: total={} pending={} blocked={} in_progress={} completed={} failed={}",
        summary.total,
        summary.pending,
        summary.blocked,
        summary.in_progress,
        summary.completed,
        summary.failed
    );
    Ok(stdout_only(&stdout))
}

fn run_team_shutdown(
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<TeamExecution, TeamError> {
    let Some(team_name) = args.first() else {
        return Err(TeamError::runtime(
            "Usage: omx team shutdown <team-name> [--force] [--ralph]",
        ));
    };

    let mut force = false;
    let mut ralph = false;
    for token in &args[1..] {
        match token.as_str() {
            "--force" => force = true,
            "--ralph" => ralph = true,
            unknown => {
                return Err(TeamError::runtime(format!(
                    "Unknown argument for \"omx team shutdown\": {unknown}"
                )));
            }
        }
    }

    let team_root = team_root(cwd, team_name);
    if !team_root.exists() {
        let _ = deactivate_team_mode_state(&resolve_state_root(cwd, env), team_name, "complete");
        return Ok(stdout_only(&format!(
            "Team shutdown complete: {team_name}\n"
        )));
    }

    let tasks = summarize_tasks(&team_root.join("tasks"))?;
    let has_active_work = tasks.pending > 0 || tasks.blocked > 0 || tasks.in_progress > 0;
    let gate_allowed = tasks.pending == 0
        && tasks.blocked == 0
        && tasks.in_progress == 0
        && (tasks.failed == 0 || (ralph && !has_active_work));
    if !force && !gate_allowed {
        return Err(TeamError::runtime(format!(
            "shutdown_gate_blocked:pending={},blocked={},in_progress={},failed={}",
            tasks.pending, tasks.blocked, tasks.in_progress, tasks.failed
        )));
    }

    let workers = collect_team_worker_names(&team_root)?;
    let requested_by = if ralph { "ralph" } else { "leader-fixed" };
    for worker in workers {
        write_shutdown_request_file(&team_root, &worker, requested_by)?;
    }
    if read_optional_json(team_root.join("config.json"))?
        .as_deref()
        .and_then(|raw| extract_json_string_field(raw, "worker_launch_mode"))
        .as_deref()
        == Some("prompt")
    {
        let config_raw = read_optional_json(team_root.join("config.json"))?.unwrap_or_default();
        for pid in collect_worker_pids_from_config(&config_raw) {
            let _ = terminate_worker_pid(pid);
        }
    }
    let _ = deactivate_team_mode_state(&resolve_state_root(cwd, env), team_name, "complete");
    remove_team_state(&team_root)?;
    Ok(stdout_only(&format!(
        "Team shutdown complete: {team_name}\n"
    )))
}

fn execute_team_api(parsed: ParsedTeamApiArgs, cwd: &Path) -> TeamExecution {
    let command = format!("omx team api {}", parsed.operation.as_str());
    match execute_team_api_inner(parsed.operation, &parsed.input, cwd) {
        Ok(data) => {
            if parsed.json {
                json_success_execution(&command, parsed.operation.as_str(), &data)
            } else {
                execution(
                    format!("ok operation={}\n{}\n", parsed.operation.as_str(), data),
                    String::new(),
                    0,
                )
            }
        }
        Err(error) => {
            if parsed.json {
                json_error_execution(
                    &command,
                    parsed.operation.as_str(),
                    "runtime_error",
                    &error.to_string(),
                )
            } else {
                execution(String::new(), format!("{error}\n"), 1)
            }
        }
    }
}

fn execute_team_api_inner(
    operation: TeamApiOperation,
    input: &str,
    cwd: &Path,
) -> Result<String, TeamError> {
    let team_name = required_input_string(input, "team_name")?;
    let team_root = team_root(cwd, &team_name);
    match operation {
        TeamApiOperation::SendMessage => {
            let from_worker = required_input_string(input, "from_worker")?;
            let to_worker = required_input_string(input, "to_worker")?;
            let body = required_input_string(input, "body")?;
            let message =
                send_team_message(&team_root, &team_name, &from_worker, &to_worker, &body)?;
            Ok(format!("{{\"message\":{message}}}"))
        }
        TeamApiOperation::Broadcast => {
            let from_worker = required_input_string(input, "from_worker")?;
            let body = required_input_string(input, "body")?;
            let workers = collect_team_worker_names(&team_root)?;
            let messages = workers
                .into_iter()
                .filter(|worker| worker != &from_worker)
                .map(|worker| {
                    send_team_message(&team_root, &team_name, &from_worker, &worker, &body)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!(
                "{{\"count\":{},\"messages\":[{}]}}",
                messages.len(),
                messages.join(",")
            ))
        }
        TeamApiOperation::MailboxMarkDelivered => {
            let worker = required_input_string(input, "worker")?;
            let message_id = required_input_string(input, "message_id")?;
            let updated = mark_mailbox_message(&team_root, &worker, &message_id, "delivered_at")?;
            let (dispatch_request_id, dispatch_updated) =
                mark_latest_mailbox_dispatch_delivered(&team_root, &worker, &message_id)?;
            Ok(format!(
                concat!(
                    "{{",
                    "\"worker\":\"{}\",",
                    "\"message_id\":\"{}\",",
                    "\"updated\":{},",
                    "\"dispatch_request_id\":{},",
                    "\"dispatch_updated\":{}",
                    "}}"
                ),
                escape_json_string(&worker),
                escape_json_string(&message_id),
                updated,
                dispatch_request_id
                    .map(|value| format!("\"{}\"", escape_json_string(&value)))
                    .unwrap_or_else(|| "null".to_string()),
                dispatch_updated,
            ))
        }
        TeamApiOperation::MailboxMarkNotified => {
            let worker = required_input_string(input, "worker")?;
            let message_id = required_input_string(input, "message_id")?;
            let notified = mark_mailbox_message(&team_root, &worker, &message_id, "notified_at")?;
            Ok(format!(
                "{{\"worker\":\"{}\",\"message_id\":\"{}\",\"notified\":{}}}",
                escape_json_string(&worker),
                escape_json_string(&message_id),
                notified
            ))
        }
        TeamApiOperation::CreateTask => {
            let subject = required_input_string(input, "subject")?;
            let description = required_input_string(input, "description")?;
            let task = create_team_task(
                &team_root,
                optional_input_string(input, "owner"),
                &subject,
                &description,
                optional_input_string_array(input, "blocked_by"),
                parse_optional_bool_field(input, "requires_code_change")?,
            )?;
            Ok(format!("{{\"task\":{task}}}"))
        }
        TeamApiOperation::ReadConfig => {
            let config = read_optional_json(team_root.join("config.json"))?;
            Ok(format!(
                "{{\"config\":{}}}",
                config.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::ReadManifest => {
            let manifest = read_optional_json(team_root.join("manifest.v2.json"))?;
            Ok(format!(
                "{{\"manifest\":{}}}",
                manifest.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::ReadWorkerStatus => {
            let worker = required_input_string(input, "worker")?;
            let status =
                read_optional_json(team_root.join("workers").join(&worker).join("status.json"))?;
            Ok(format!(
                "{{\"worker\":\"{}\",\"status\":{}}}",
                escape_json_string(&worker),
                status.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::ReadWorkerHeartbeat => {
            let worker = required_input_string(input, "worker")?;
            let heartbeat = read_optional_json(
                team_root
                    .join("workers")
                    .join(&worker)
                    .join("heartbeat.json"),
            )?;
            Ok(format!(
                "{{\"worker\":\"{}\",\"heartbeat\":{}}}",
                escape_json_string(&worker),
                heartbeat.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::UpdateWorkerHeartbeat => {
            let worker = required_input_string(input, "worker")?;
            let pid = required_input_u64(input, "pid")?;
            let turn_count = required_input_u64(input, "turn_count")?;
            let alive = required_input_bool(input, "alive")?;
            write_worker_heartbeat(&team_root, &worker, pid, turn_count, alive)?;
            Ok(format!(
                "{{\"worker\":\"{}\"}}",
                escape_json_string(&worker)
            ))
        }
        TeamApiOperation::WriteWorkerInbox => {
            let worker = required_input_string(input, "worker")?;
            let content = required_input_string(input, "content")?;
            write_worker_inbox(&team_root, &worker, &content)?;
            Ok(format!(
                "{{\"worker\":\"{}\"}}",
                escape_json_string(&worker)
            ))
        }
        TeamApiOperation::WriteWorkerIdentity => {
            let worker = required_input_string(input, "worker")?;
            let index = required_input_u64(input, "index")?;
            let role = required_input_string(input, "role")?;
            write_worker_identity(
                &team_root,
                &worker,
                index,
                &role,
                optional_input_string_array(input, "assigned_tasks").unwrap_or_default(),
                optional_input_u64(input, "pid"),
                optional_input_string(input, "pane_id"),
                optional_input_string(input, "working_dir"),
                optional_input_string(input, "worktree_path"),
                optional_input_string(input, "worktree_branch"),
                parse_optional_bool_field(input, "worktree_detached")?,
                optional_input_string(input, "team_state_root"),
            )?;
            Ok(format!(
                "{{\"worker\":\"{}\"}}",
                escape_json_string(&worker)
            ))
        }
        TeamApiOperation::AppendEvent => {
            let event_type = required_input_string(input, "type")?;
            let worker = required_input_string(input, "worker")?;
            if !is_supported_team_event_type(&event_type) {
                return Err(TeamError::runtime(format!(
                    "type must be one of: {}",
                    SUPPORTED_TEAM_EVENT_TYPES.join(", ")
                )));
            }
            let event = append_team_event(
                &team_root,
                &team_name,
                &event_type,
                &worker,
                optional_input_string(input, "task_id"),
                optional_input_string(input, "message_id"),
                optional_input_string(input, "reason"),
                optional_input_string(input, "state"),
                optional_input_string(input, "prev_state"),
                optional_input_string(input, "to_worker"),
                optional_input_u64(input, "worker_count"),
                optional_input_string(input, "source_type"),
            )?;
            Ok(format!("{{\"event\":{event}}}"))
        }
        TeamApiOperation::ReadTask => {
            let task_id = required_input_string(input, "task_id")?;
            let task =
                read_optional_json(team_root.join("tasks").join(format!("task-{task_id}.json")))?;
            Ok(format!(
                "{{\"task\":{}}}",
                task.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::ListTasks => {
            let tasks = read_json_array_from_dir(&team_root.join("tasks"))?;
            Ok(format!(
                "{{\"tasks\":{},\"count\":{}}}",
                tasks,
                count_top_level_items(&tasks)
            ))
        }
        TeamApiOperation::UpdateTask => {
            let task_id = required_input_string(input, "task_id")?;
            if extract_json_value(input, "status").is_some()
                || extract_json_value(input, "owner").is_some()
                || extract_json_value(input, "result").is_some()
                || extract_json_value(input, "error").is_some()
            {
                return Err(TeamError::runtime(
                    "team_update_task cannot mutate lifecycle fields: status, owner, result, error",
                ));
            }
            let task = update_task_metadata(
                &team_root,
                &task_id,
                optional_input_string(input, "subject"),
                optional_input_string(input, "description"),
                optional_input_string_array(input, "blocked_by"),
                parse_optional_bool_field(input, "requires_code_change")?,
            )?;
            Ok(format!(
                "{{\"task\":{}}}",
                task.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::ClaimTask => {
            let task_id = required_input_string(input, "task_id")?;
            let worker = required_input_string(input, "worker")?;
            let expected_version = parse_optional_positive_u64(input, "expected_version")?;
            let result = claim_team_task(&team_root, &task_id, &worker, expected_version)?;
            Ok(result)
        }
        TeamApiOperation::TransitionTaskStatus => {
            let task_id = required_input_string(input, "task_id")?;
            let from = required_input_string(input, "from")?;
            let to = required_input_string(input, "to")?;
            let claim_token = required_input_string(input, "claim_token")?;
            let result = transition_team_task_status(
                &team_root,
                &team_name,
                &task_id,
                &from,
                &to,
                &claim_token,
            )?;
            Ok(result)
        }
        TeamApiOperation::ReleaseTaskClaim => {
            let task_id = required_input_string(input, "task_id")?;
            let claim_token = required_input_string(input, "claim_token")?;
            let worker = required_input_string(input, "worker")?;
            let result = release_team_task_claim(&team_root, &task_id, &claim_token, &worker)?;
            Ok(result)
        }
        TeamApiOperation::MailboxList => {
            let worker = required_input_string(input, "worker")?;
            let mailbox =
                read_optional_json(team_root.join("mailbox").join(format!("{worker}.json")))?;
            let messages = mailbox
                .as_deref()
                .and_then(|raw| extract_json_value(raw, "messages"))
                .unwrap_or_else(|| "[]".to_string());
            Ok(format!(
                "{{\"worker\":\"{}\",\"count\":{},\"mailbox\":{},\"messages\":{}}}",
                escape_json_string(&worker),
                count_top_level_items(&messages),
                mailbox.clone().unwrap_or_else(|| "null".to_string()),
                messages,
            ))
        }
        TeamApiOperation::Cleanup => {
            remove_team_state(&team_root)?;
            Ok(format!(
                "{{\"team_name\":\"{}\"}}",
                escape_json_string(&team_name)
            ))
        }
        TeamApiOperation::WriteShutdownRequest => {
            let worker = required_input_string(input, "worker")?;
            let requested_by = required_input_string(input, "requested_by")?;
            write_shutdown_request_file(&team_root, &worker, &requested_by)?;
            Ok(format!(
                "{{\"worker\":\"{}\"}}",
                escape_json_string(&worker)
            ))
        }
        TeamApiOperation::ReadMonitorSnapshot => {
            let snapshot = read_optional_json(team_root.join("monitor-snapshot.json"))?;
            Ok(format!(
                "{{\"snapshot\":{}}}",
                snapshot.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::WriteMonitorSnapshot => {
            let snapshot = required_input_object(input, "snapshot")?;
            write_monitor_snapshot_file(&team_root, &snapshot)?;
            Ok("{}".to_string())
        }
        TeamApiOperation::ReadShutdownAck => {
            let worker = required_input_string(input, "worker")?;
            let ack = read_shutdown_ack_json(
                &team_root,
                &worker,
                optional_input_string(input, "min_updated_at").as_deref(),
            )?;
            Ok(format!(
                "{{\"worker\":\"{}\",\"ack\":{}}}",
                escape_json_string(&worker),
                ack.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::ReadTaskApproval => {
            let task_id = required_input_string(input, "task_id")?;
            let approval = read_optional_json(
                team_root
                    .join("approvals")
                    .join(format!("task-{task_id}.json")),
            )?;
            Ok(format!(
                "{{\"approval\":{}}}",
                approval.unwrap_or_else(|| "null".to_string())
            ))
        }
        TeamApiOperation::WriteTaskApproval => {
            let task_id = required_input_string(input, "task_id")?;
            let status = required_input_string(input, "status")?;
            let reviewer = required_input_string(input, "reviewer")?;
            let decision_reason = required_input_string(input, "decision_reason")?;
            let required = parse_optional_bool_field(input, "required")?.unwrap_or(true);
            if !matches!(status.as_str(), "pending" | "approved" | "rejected") {
                return Err(TeamError::runtime(
                    "status must be one of: pending, approved, rejected",
                ));
            }
            write_task_approval_file(
                &team_root,
                &team_name,
                &task_id,
                required,
                &status,
                &reviewer,
                &decision_reason,
            )?;
            Ok(format!(
                "{{\"task_id\":\"{}\",\"status\":\"{}\"}}",
                escape_json_string(&task_id),
                escape_json_string(&status),
            ))
        }
        TeamApiOperation::ReadEvents => {
            let after_event_id = optional_input_string(input, "after_event_id");
            let events = read_team_events(
                cwd,
                &team_name,
                EventQuery {
                    after_event_id: after_event_id.as_deref(),
                    wakeable_only: parse_optional_bool_field(input, "wakeable_only")?
                        .unwrap_or(false),
                    event_type: optional_input_string(input, "type").as_deref(),
                    worker: optional_input_string(input, "worker").as_deref(),
                    task_id: optional_input_string(input, "task_id").as_deref(),
                },
            )?;
            let cursor = events
                .last()
                .map(|event| event.event_id.clone())
                .unwrap_or_else(|| after_event_id.unwrap_or_default());
            Ok(format!(
                "{{\"count\":{},\"cursor\":\"{}\",\"events\":[{}]}}",
                events.len(),
                escape_json_string(&cursor),
                events
                    .iter()
                    .map(|event| event.raw.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ))
        }
        TeamApiOperation::AwaitEvent => {
            let timeout_ms = optional_input_u64(input, "timeout_ms").unwrap_or(30_000);
            let wakeable_only = parse_optional_bool_field(input, "wakeable_only")?.unwrap_or(false);
            let after_event_id = optional_input_string(input, "after_event_id");
            let baseline = after_event_id
                .unwrap_or_else(|| latest_event_id(cwd, &team_name).unwrap_or_default());
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));

            loop {
                let events = read_team_events(
                    cwd,
                    &team_name,
                    EventQuery {
                        after_event_id: if baseline.is_empty() {
                            None
                        } else {
                            Some(baseline.as_str())
                        },
                        wakeable_only,
                        event_type: optional_input_string(input, "type").as_deref(),
                        worker: optional_input_string(input, "worker").as_deref(),
                        task_id: optional_input_string(input, "task_id").as_deref(),
                    },
                )?;
                if let Some(event) = events.first() {
                    return Ok(format!(
                        "{{\"status\":\"event\",\"cursor\":\"{}\",\"event\":{}}}",
                        escape_json_string(&event.event_id),
                        event.raw
                    ));
                }
                if Instant::now() >= deadline {
                    return Ok(format!(
                        "{{\"status\":\"timeout\",\"cursor\":\"{}\",\"event\":null}}",
                        escape_json_string(&baseline)
                    ));
                }
                thread::sleep(Duration::from_millis(
                    optional_input_u64(input, "poll_ms").unwrap_or(100),
                ));
            }
        }
        TeamApiOperation::GetSummary => {
            let summary = read_team_summary(&team_root)?;
            Ok(format!("{{\"summary\":{summary}}}"))
        }
        TeamApiOperation::ReadIdleState => {
            let summary = read_team_summary(&team_root)?;
            let snapshot = read_optional_json(team_root.join("monitor-snapshot.json"))?
                .unwrap_or_else(|| "null".to_string());
            let recent_events = read_recent_events_json(cwd, &team_name, 50)?;
            Ok(build_idle_state_json(
                &team_name,
                &summary,
                &snapshot,
                &recent_events,
            ))
        }
        TeamApiOperation::ReadStallState => {
            let summary = read_team_summary(&team_root)?;
            let snapshot = read_optional_json(team_root.join("monitor-snapshot.json"))?
                .unwrap_or_else(|| "null".to_string());
            let recent_events = read_recent_events_json(cwd, &team_name, 50)?;
            Ok(build_stall_state_json(
                &team_name,
                &summary,
                &snapshot,
                &recent_events,
            ))
        }
    }
}

fn build_team_api_operation_help(operation: TeamApiOperation) -> String {
    match operation {
        TeamApiOperation::SendMessage => operation_help(
            "send-message",
            &["team_name", "from_worker", "to_worker", "body"],
            &[],
        ),
        TeamApiOperation::Broadcast => {
            operation_help("broadcast", &["team_name", "from_worker", "body"], &[])
        }
        TeamApiOperation::MailboxMarkDelivered => operation_help(
            "mailbox-mark-delivered",
            &["team_name", "worker", "message_id"],
            &[],
        ),
        TeamApiOperation::MailboxMarkNotified => operation_help(
            "mailbox-mark-notified",
            &["team_name", "worker", "message_id"],
            &[],
        ),
        TeamApiOperation::CreateTask => operation_help(
            "create-task",
            &["team_name", "subject", "description"],
            &["owner", "blocked_by", "requires_code_change"],
        ),
        TeamApiOperation::ReadConfig => operation_help("read-config", &["team_name"], &[]),
        TeamApiOperation::ReadManifest => operation_help("read-manifest", &["team_name"], &[]),
        TeamApiOperation::ReadWorkerStatus => {
            operation_help("read-worker-status", &["team_name", "worker"], &[])
        }
        TeamApiOperation::ReadWorkerHeartbeat => {
            operation_help("read-worker-heartbeat", &["team_name", "worker"], &[])
        }
        TeamApiOperation::UpdateWorkerHeartbeat => operation_help(
            "update-worker-heartbeat",
            &["team_name", "worker", "pid", "turn_count", "alive"],
            &[],
        ),
        TeamApiOperation::WriteWorkerInbox => operation_help(
            "write-worker-inbox",
            &["team_name", "worker", "content"],
            &[],
        ),
        TeamApiOperation::WriteWorkerIdentity => operation_help(
            "write-worker-identity",
            &["team_name", "worker", "index", "role"],
            &[
                "assigned_tasks",
                "pid",
                "pane_id",
                "working_dir",
                "worktree_path",
                "worktree_branch",
                "worktree_detached",
                "team_state_root",
            ],
        ),
        TeamApiOperation::AppendEvent => operation_help(
            "append-event",
            &["team_name", "type", "worker"],
            &[
                "task_id",
                "message_id",
                "reason",
                "state",
                "prev_state",
                "to_worker",
                "worker_count",
                "source_type",
            ],
        ),
        TeamApiOperation::ReadTask => operation_help("read-task", &["team_name", "task_id"], &[]),
        TeamApiOperation::ListTasks => operation_help("list-tasks", &["team_name"], &[]),
        TeamApiOperation::UpdateTask => operation_help(
            "update-task",
            &["team_name", "task_id"],
            &[
                "subject",
                "description",
                "blocked_by",
                "requires_code_change",
            ],
        ),
        TeamApiOperation::ClaimTask => operation_help(
            "claim-task",
            &["team_name", "task_id", "worker"],
            &["expected_version"],
        ),
        TeamApiOperation::TransitionTaskStatus => operation_help(
            "transition-task-status",
            &["team_name", "task_id", "from", "to", "claim_token"],
            &[],
        ),
        TeamApiOperation::ReleaseTaskClaim => operation_help(
            "release-task-claim",
            &["team_name", "task_id", "claim_token", "worker"],
            &[],
        ),
        TeamApiOperation::MailboxList => {
            operation_help("mailbox-list", &["team_name", "worker"], &[])
        }
        TeamApiOperation::Cleanup => operation_help("cleanup", &["team_name"], &[]),
        TeamApiOperation::WriteShutdownRequest => operation_help(
            "write-shutdown-request",
            &["team_name", "worker", "requested_by"],
            &[],
        ),
        TeamApiOperation::ReadMonitorSnapshot => {
            operation_help("read-monitor-snapshot", &["team_name"], &[])
        }
        TeamApiOperation::WriteMonitorSnapshot => {
            operation_help("write-monitor-snapshot", &["team_name", "snapshot"], &[])
        }
        TeamApiOperation::ReadShutdownAck => operation_help(
            "read-shutdown-ack",
            &["team_name", "worker"],
            &["min_updated_at"],
        ),
        TeamApiOperation::ReadTaskApproval => {
            operation_help("read-task-approval", &["team_name", "task_id"], &[])
        }
        TeamApiOperation::WriteTaskApproval => operation_help(
            "write-task-approval",
            &[
                "team_name",
                "task_id",
                "status",
                "reviewer",
                "decision_reason",
            ],
            &["required"],
        ),
        TeamApiOperation::ReadEvents => operation_help(
            "read-events",
            &["team_name"],
            &[
                "after_event_id",
                "wakeable_only",
                "type",
                "worker",
                "task_id",
            ],
        ),
        TeamApiOperation::AwaitEvent => operation_help(
            "await-event",
            &["team_name"],
            &[
                "after_event_id",
                "timeout_ms",
                "poll_ms",
                "wakeable_only",
                "type",
                "worker",
                "task_id",
            ],
        ),
        TeamApiOperation::ReadIdleState => operation_help("read-idle-state", &["team_name"], &[]),
        TeamApiOperation::ReadStallState => operation_help("read-stall-state", &["team_name"], &[]),
        TeamApiOperation::GetSummary => operation_help("get-summary", &["team_name"], &[]),
    }
}

fn operation_help(operation: &str, required: &[&str], optional: &[&str]) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "Usage: omx team api {operation} --input <json> [--json]\n"
    );
    let _ = writeln!(text, "Required input fields:");
    for field in required {
        let _ = writeln!(text, "  - {field}");
    }
    if !optional.is_empty() {
        let _ = writeln!(text, "\nOptional input fields:");
        for field in optional {
            let _ = writeln!(text, "  - {field}");
        }
    }
    let mut sample = String::from("{");
    for (index, field) in required.iter().enumerate() {
        if index > 0 {
            sample.push(',');
        }
        let value = sample_value_for_field(field);
        let _ = write!(sample, "\"{field}\":{value}");
    }
    sample.push('}');
    let _ = writeln!(
        text,
        "\nExample:\n  omx team api {operation} --input '{}' --json",
        sample
    );
    text
}

fn sample_value_for_field(field: &str) -> &'static str {
    match field {
        "team_name" => "\"my-team\"",
        "from_worker" => "\"worker-1\"",
        "to_worker" => "\"leader-fixed\"",
        "worker" => "\"worker-1\"",
        "task_id" => "\"1\"",
        "subject" => "\"Demo task\"",
        "description" => "\"Created through CLI interop\"",
        "body" => "\"ACK\"",
        "message_id" => "\"msg-123\"",
        "content" => "\"# Inbox update\\nProceed with task 2.\"",
        "index" => "1",
        "role" => "\"executor\"",
        "assigned_tasks" => "[\"1\",\"2\"]",
        "pid" => "12345",
        "turn_count" => "12",
        "alive" => "true",
        "requested_by" => "\"leader-fixed\"",
        "status" => "\"approved\"",
        "reviewer" => "\"leader-fixed\"",
        "decision_reason" => "\"approved in demo\"",
        "required" => "true",
        "from" => "\"in_progress\"",
        "to" => "\"completed\"",
        "claim_token" => "\"claim-token\"",
        "expected_version" => "1",
        "type" => "\"task_completed\"",
        "snapshot" => "{\"taskStatusById\":{\"1\":\"completed\"}}",
        _ => "\"value\"",
    }
}

fn parse_team_api_args(
    operation: TeamApiOperation,
    args: &[String],
) -> Result<ParsedTeamApiArgs, String> {
    let mut input = "{}".to_string();
    let mut json = false;
    let mut index = 0_usize;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "--input" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("Missing value after --input".to_string());
                };
                ensure_json_object(value)?;
                input = value.clone();
                index += 1;
            }
            token if token.starts_with("--input=") => {
                let value = &token["--input=".len()..];
                ensure_json_object(value)?;
                input = value.to_string();
            }
            token => return Err(format!("Unknown argument for \"omx team api\": {token}")),
        }
        index += 1;
    }
    Ok(ParsedTeamApiArgs {
        operation,
        input,
        json,
    })
}

fn ensure_json_object(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        Ok(())
    } else {
        Err("Invalid --input JSON: input must be a JSON object".to_string())
    }
}

fn run_team_status(
    team_name: &str,
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<TeamExecution, TeamError> {
    let team_root = cwd.join(".omx").join("state").join("team").join(team_name);
    if !team_root.exists() {
        return Ok(stdout_only(&format!(
            "No team state found for {team_name}\n"
        )));
    }

    let manifest_path = team_root.join("manifest.v2.json");
    let config_path = team_root.join("config.json");
    let config_raw = fs::read_to_string(&config_path).ok();
    let manifest_raw = fs::read_to_string(&manifest_path)
        .or_else(|_| {
            config_raw.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "team config and manifest are missing",
                )
            })
        })
        .map_err(|error| TeamError::runtime(format!("failed to read team config: {error}")))?;
    let snapshot_raw =
        fs::read_to_string(team_root.join("monitor-snapshot.json")).unwrap_or_default();
    let phase_raw = fs::read_to_string(team_root.join("phase.json")).unwrap_or_default();
    let runtime = read_runtime_layout_evidence(
        config_raw.as_deref().unwrap_or(&manifest_raw),
        Some(&manifest_raw),
    );

    let resolved_team_name =
        extract_json_string_field(&manifest_raw, "name").unwrap_or_else(|| team_name.to_string());
    let phase = extract_json_string_field(&phase_raw, "current_phase")
        .unwrap_or_else(|| "unknown".to_string());
    let workers_total = count_worker_names(&manifest_raw);
    let dead_workers = count_false_entries(&snapshot_raw, "workerAliveByName");
    let non_reporting_workers = count_string_entries(&snapshot_raw, "workerStateByName", "unknown");
    let tasks = summarize_tasks(&team_root.join("tasks"))?;
    let layout = sync_prompt_layout_if_available(
        &team_root,
        cwd,
        "status-refresh",
        HudModeOverride::Preserve,
        Some(env),
    );

    let mut stdout = String::new();
    let _ = writeln!(stdout, "team={resolved_team_name} phase={phase}");
    let _ = writeln!(stdout, "runtime target: {}", runtime.runtime_target);
    let _ = writeln!(stdout, "{}", render_runtime_layout_line(&runtime));
    let _ = writeln!(stdout, "{}", render_runtime_tmux_line(&runtime));
    if let Some(layout) = layout.as_ref() {
        let _ = writeln!(stdout, "{}", layout.summary_line());
        let _ = writeln!(stdout, "{}", layout.proof_line());
    }
    let _ = writeln!(
        stdout,
        "workers: total={workers_total} dead={dead_workers} non_reporting={non_reporting_workers}"
    );
    let _ = writeln!(
        stdout,
        "tasks: total={} pending={} blocked={} in_progress={} completed={} failed={}",
        tasks.total, tasks.pending, tasks.blocked, tasks.in_progress, tasks.completed, tasks.failed
    );

    Ok(stdout_only(&stdout))
}

#[derive(Default)]
struct TaskSummary {
    total: usize,
    pending: usize,
    blocked: usize,
    in_progress: usize,
    completed: usize,
    failed: usize,
}

fn summarize_tasks(tasks_dir: &Path) -> Result<TaskSummary, TeamError> {
    let mut summary = TaskSummary::default();
    let entries = match fs::read_dir(tasks_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(summary),
        Err(error) => {
            return Err(TeamError::runtime(format!(
                "failed to read {}: {error}",
                tasks_dir.display()
            )));
        }
    };

    for entry in entries {
        let entry = entry
            .map_err(|error| TeamError::runtime(format!("failed to enumerate tasks: {error}")))?;
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let raw = fs::read_to_string(entry.path()).map_err(|error| {
            TeamError::runtime(format!(
                "failed to read {}: {error}",
                entry.path().display()
            ))
        })?;
        summary.total += 1;
        match extract_json_string_field(&raw, "status").as_deref() {
            Some("pending") => summary.pending += 1,
            Some("blocked") => summary.blocked += 1,
            Some("in_progress") => summary.in_progress += 1,
            Some("completed") => summary.completed += 1,
            Some("failed") => summary.failed += 1,
            _ => {}
        }
    }

    Ok(summary)
}

fn count_worker_names(raw: &str) -> usize {
    split_top_level_json_array_items(
        &extract_json_value(raw, "workers").unwrap_or_else(|| "[]".to_string()),
    )
    .iter()
    .filter(|item| {
        extract_json_string_field(item, "name").is_some_and(|name| name.starts_with("worker-"))
    })
    .count()
}

fn count_false_entries(raw: &str, key: &str) -> usize {
    count_map_entries(raw, key, "false")
}

fn count_string_entries(raw: &str, key: &str, value: &str) -> usize {
    count_map_entries(raw, key, &format!("\"{value}\""))
}

fn count_map_entries(raw: &str, key: &str, expected_value: &str) -> usize {
    let Some(section) = extract_object_contents(raw, key) else {
        return 0;
    };
    section
        .lines()
        .filter(|line| line.contains(expected_value))
        .count()
}

fn extract_object_contents<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    let start = format!("\"{key}\": {{");
    let start_idx = raw.find(&start)? + start.len();
    let rest = raw.get(start_idx..)?;
    let mut depth = 1usize;
    for (idx, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return rest.get(..idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn team_root(cwd: &Path, team_name: &str) -> std::path::PathBuf {
    cwd.join(".omx").join("state").join("team").join(team_name)
}

fn read_optional_json(path: impl AsRef<Path>) -> Result<Option<String>, TeamError> {
    match fs::read_to_string(path.as_ref()) {
        Ok(raw) => Ok(Some(raw.trim().to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(TeamError::runtime(format!(
            "failed to read {}: {error}",
            path.as_ref().display()
        ))),
    }
}

fn read_json_array_from_dir(path: &Path) -> Result<String, TeamError> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("[]".to_string()),
        Err(error) => {
            return Err(TeamError::runtime(format!(
                "failed to read {}: {error}",
                path.display()
            )));
        }
    };

    let mut items = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            TeamError::runtime(format!("failed to enumerate {}: {error}", path.display()))
        })?;
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let raw = fs::read_to_string(entry.path()).map_err(|error| {
            TeamError::runtime(format!(
                "failed to read {}: {error}",
                entry.path().display()
            ))
        })?;
        items.push(raw.trim().to_string());
    }
    items.sort();
    Ok(format!("[{}]", items.join(",")))
}

fn count_top_level_items(raw: &str) -> usize {
    let trimmed = raw.trim();
    if trimmed == "[]" {
        return 0;
    }
    split_top_level_json_array_items(trimmed).len()
}

const SUPPORTED_TEAM_EVENT_TYPES: &[&str] = &[
    "task_completed",
    "task_failed",
    "worker_state_changed",
    "worker_idle",
    "worker_stopped",
    "message_received",
    "leader_notification_deferred",
    "all_workers_idle",
    "shutdown_ack",
    "shutdown_gate",
    "shutdown_gate_forced",
    "ralph_cleanup_policy",
    "ralph_cleanup_summary",
    "approval_decision",
    "team_leader_nudge",
];

fn is_supported_team_event_type(value: &str) -> bool {
    SUPPORTED_TEAM_EVENT_TYPES.contains(&value)
}

fn required_input_string(input: &str, field: &str) -> Result<String, TeamError> {
    extract_json_string_field(input, field)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| TeamError::runtime(format!("{field} is required")))
}

fn optional_input_string(input: &str, field: &str) -> Option<String> {
    extract_json_string_field(input, field).filter(|value| !value.trim().is_empty())
}

fn optional_input_bool(input: &str, field: &str) -> Option<bool> {
    let raw = extract_json_value(input, field)?;
    match raw.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn optional_input_u64(input: &str, field: &str) -> Option<u64> {
    extract_json_value(input, field)?.trim().parse::<u64>().ok()
}

fn required_input_u64(input: &str, field: &str) -> Result<u64, TeamError> {
    optional_input_u64(input, field)
        .ok_or_else(|| TeamError::runtime(format!("{field} must be a positive integer")))
}

fn required_input_bool(input: &str, field: &str) -> Result<bool, TeamError> {
    optional_input_bool(input, field)
        .ok_or_else(|| TeamError::runtime(format!("{field} must be a boolean")))
}

fn parse_optional_bool_field(input: &str, field: &str) -> Result<Option<bool>, TeamError> {
    match extract_json_value(input, field) {
        Some(_) => optional_input_bool(input, field)
            .map(Some)
            .ok_or_else(|| TeamError::runtime(format!("{field} must be a boolean when provided"))),
        None => Ok(None),
    }
}

fn parse_optional_positive_u64(input: &str, field: &str) -> Result<Option<u64>, TeamError> {
    match extract_json_value(input, field) {
        Some(raw) => raw
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .map(Some)
            .ok_or_else(|| {
                TeamError::runtime(format!("{field} must be a positive integer when provided"))
            }),
        None => Ok(None),
    }
}

fn optional_input_string_array(input: &str, field: &str) -> Option<Vec<String>> {
    let raw = extract_json_value(input, field)?;
    parse_json_string_array(&raw)
}

fn required_input_object(input: &str, field: &str) -> Result<String, TeamError> {
    let raw = extract_json_value(input, field)
        .ok_or_else(|| TeamError::runtime(format!("{field} is required")))?;
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        Ok(trimmed.to_string())
    } else {
        Err(TeamError::runtime(format!("{field} must be a JSON object")))
    }
}

fn parse_json_string_array(raw: &str) -> Option<Vec<String>> {
    let trimmed = raw.trim();
    if trimmed == "[]" {
        return Some(Vec::new());
    }
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    let mut items = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in inner.chars() {
        if in_string {
            if escaped {
                current.push(ch);
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => {
                    in_string = false;
                    items.push(current.clone());
                    current.clear();
                }
                _ => current.push(ch),
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        if !ch.is_ascii_whitespace() && ch != ',' {
            return None;
        }
    }
    if in_string || escaped {
        return None;
    }
    Some(items)
}

fn extract_json_value(raw: &str, key: &str) -> Option<String> {
    let key = format!("\"{key}\"");
    let key_start = raw.find(&key)? + key.len();
    let after_key = raw.get(key_start..)?;
    let colon_idx = after_key.find(':')?;
    let value_start = key_start + colon_idx + 1;
    let bytes = raw.as_bytes();
    let mut idx = value_start;
    while bytes.get(idx).is_some_and(u8::is_ascii_whitespace) {
        idx += 1;
    }
    let start = idx;
    match bytes.get(idx)? {
        b'"' => {
            idx += 1;
            let mut escaped = false;
            while let Some(byte) = bytes.get(idx) {
                if escaped {
                    escaped = false;
                } else if *byte == b'\\' {
                    escaped = true;
                } else if *byte == b'"' {
                    return raw.get(start..=idx).map(str::to_string);
                }
                idx += 1;
            }
            None
        }
        b'{' | b'[' => {
            let open = *bytes.get(idx)? as char;
            let close = if open == '{' { '}' } else { ']' };
            let mut depth = 0_i32;
            let mut in_string = false;
            let mut escaped = false;
            for (offset, ch) in raw[idx..].char_indices() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                    continue;
                }
                if ch == '"' {
                    in_string = true;
                    continue;
                }
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return raw
                            .get(start..idx + offset + ch.len_utf8())
                            .map(str::to_string);
                    }
                }
            }
            None
        }
        _ => {
            let end = raw[idx..]
                .find([',', '}', '\n', '\r'])
                .map_or(raw.len(), |offset| idx + offset);
            raw.get(start..end).map(|value| value.trim().to_string())
        }
    }
}

#[derive(Clone)]
struct TaskClaimRecord {
    owner: String,
    token: String,
    leased_until: String,
}

#[derive(Clone)]
struct TaskRecord {
    id: String,
    subject: String,
    description: String,
    status: String,
    owner: Option<String>,
    blocked_by: Vec<String>,
    requires_code_change: Option<bool>,
    result: Option<String>,
    error: Option<String>,
    version: u64,
    claim: Option<TaskClaimRecord>,
    created_at: String,
    completed_at: Option<String>,
}

fn find_json_value_range(raw: &str, key: &str) -> Option<(usize, usize)> {
    let key = format!("\"{key}\"");
    let key_start = raw.find(&key)? + key.len();
    let after_key = raw.get(key_start..)?;
    let colon_idx = after_key.find(':')?;
    let value_start = key_start + colon_idx + 1;
    let bytes = raw.as_bytes();
    let mut idx = value_start;
    while bytes.get(idx).is_some_and(u8::is_ascii_whitespace) {
        idx += 1;
    }
    let start = idx;
    match bytes.get(idx)? {
        b'"' => {
            idx += 1;
            let mut escaped = false;
            while let Some(byte) = bytes.get(idx) {
                if escaped {
                    escaped = false;
                } else if *byte == b'\\' {
                    escaped = true;
                } else if *byte == b'"' {
                    return Some((start, idx + 1));
                }
                idx += 1;
            }
            None
        }
        b'{' | b'[' => {
            let open = *bytes.get(idx)? as char;
            let close = if open == '{' { '}' } else { ']' };
            let mut depth = 0_i32;
            let mut in_string = false;
            let mut escaped = false;
            for (offset, ch) in raw[idx..].char_indices() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                    continue;
                }
                if ch == '"' {
                    in_string = true;
                    continue;
                }
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((start, idx + offset + ch.len_utf8()));
                    }
                }
            }
            None
        }
        _ => {
            let end = raw[idx..]
                .find([',', '}', '\n', '\r'])
                .map_or(raw.len(), |offset| idx + offset);
            Some((start, end))
        }
    }
}

fn upsert_json_raw_field(raw: &str, key: &str, rendered_value: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return format!("{{\"{key}\":{rendered_value}}}");
    }
    if let Some((value_start, value_end)) = find_json_value_range(trimmed, key) {
        let mut updated = String::with_capacity(trimmed.len() + rendered_value.len());
        updated.push_str(&trimmed[..value_start]);
        updated.push_str(rendered_value);
        updated.push_str(&trimmed[value_end..]);
        return updated;
    }
    if let Some(insert_idx) = trimmed.rfind('}') {
        let prefix = trimmed[..insert_idx].trim_end();
        let needs_comma = !prefix.ends_with('{');
        let mut updated = prefix.to_string();
        if needs_comma {
            updated.push(',');
        }
        updated.push_str(&format!("\"{key}\":{rendered_value}"));
        updated.push('}');
        return updated;
    }
    raw.to_string()
}

fn upsert_json_string_field(raw: &str, key: &str, value: &str) -> String {
    upsert_json_raw_field(raw, key, &format!("\"{}\"", escape_json_string(value)))
}

fn upsert_json_bool_field(raw: &str, key: &str, value: bool) -> String {
    upsert_json_raw_field(raw, key, if value { "true" } else { "false" })
}

fn upsert_json_number_field(raw: &str, key: &str, value: u64) -> String {
    upsert_json_raw_field(raw, key, &value.to_string())
}

fn format_string_array_json(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("\"{}\"", escape_json_string(value)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn read_team_phase(team_root: &Path) -> Option<String> {
    let raw = fs::read_to_string(team_root.join("phase.json")).ok()?;
    extract_json_string_field(&raw, "current_phase")
}

fn collect_team_worker_names(team_root: &Path) -> Result<Vec<String>, TeamError> {
    let manifest = read_optional_json(team_root.join("manifest.v2.json"))?;
    let config = read_optional_json(team_root.join("config.json"))?;
    Ok(manifest
        .or(config)
        .as_deref()
        .map(count_worker_names_from_config)
        .unwrap_or_default())
}

fn count_worker_names_from_config(raw: &str) -> Vec<String> {
    collect_worker_names(&extract_json_value(raw, "workers").unwrap_or_else(|| "[]".to_string()))
}

fn collect_worker_pids_from_config(raw: &str) -> Vec<u64> {
    split_top_level_json_array_items(
        &extract_json_value(raw, "workers").unwrap_or_else(|| "[]".to_string()),
    )
    .iter()
    .filter_map(|item| extract_json_value(item, "pid"))
    .filter_map(|value| value.trim().parse::<u64>().ok())
    .collect()
}

fn send_team_message(
    team_root: &Path,
    team_name: &str,
    from_worker: &str,
    to_worker: &str,
    body: &str,
) -> Result<String, TeamError> {
    let created_at = iso_timestamp();
    let message_id = generate_message_id();
    let mailbox_message = format!(
        concat!(
            "{{",
            "\"message_id\":\"{}\",",
            "\"from_worker\":\"{}\",",
            "\"to_worker\":\"{}\",",
            "\"body\":\"{}\",",
            "\"created_at\":\"{}\"",
            "}}"
        ),
        escape_json_string(&message_id),
        escape_json_string(from_worker),
        escape_json_string(to_worker),
        escape_json_string(body),
        escape_json_string(&created_at),
    );
    append_mailbox_message(team_root, to_worker, &mailbox_message)?;
    let event = format!(
        concat!(
            "{{",
            "\"event_id\":\"{}\",",
            "\"team\":\"{}\",",
            "\"type\":\"message_received\",",
            "\"worker\":\"{}\",",
            "\"message_id\":\"{}\",",
            "\"created_at\":\"{}\"",
            "}}"
        ),
        escape_json_string(&generate_event_id()),
        escape_json_string(team_name),
        escape_json_string(to_worker),
        escape_json_string(&message_id),
        escape_json_string(&created_at),
    );
    append_team_event_record(team_root, &event)?;
    Ok(mailbox_message)
}

fn mark_mailbox_message(
    team_root: &Path,
    worker_name: &str,
    message_id: &str,
    field: &str,
) -> Result<bool, TeamError> {
    let mailbox_path = team_root
        .join("mailbox")
        .join(format!("{worker_name}.json"));
    let raw = match fs::read_to_string(&mailbox_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(TeamError::runtime(format!(
                "failed to read {}: {error}",
                mailbox_path.display()
            )));
        }
    };
    let messages_raw = extract_json_value(&raw, "messages").unwrap_or_else(|| "[]".to_string());
    let mut found = false;
    let updated_messages = split_top_level_json_array_items(&messages_raw)
        .into_iter()
        .map(|message| {
            if extract_json_string_field(&message, "message_id").as_deref() == Some(message_id) {
                found = true;
                upsert_json_string_field(&message, field, &iso_timestamp())
            } else {
                message
            }
        })
        .collect::<Vec<_>>();
    if !found {
        return Ok(false);
    }
    let updated = format!(
        "{{\n  \"worker\": \"{}\",\n  \"messages\": [{}]\n}}\n",
        escape_json_string(worker_name),
        updated_messages.join(",")
    );
    write_atomic_text(&mailbox_path, &updated)?;
    Ok(true)
}

fn mark_latest_mailbox_dispatch_delivered(
    team_root: &Path,
    worker_name: &str,
    message_id: &str,
) -> Result<(Option<String>, bool), TeamError> {
    let path = team_root.join("dispatch").join("requests.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((None, false)),
        Err(error) => {
            return Err(TeamError::runtime(format!(
                "failed to read {}: {error}",
                path.display()
            )));
        }
    };
    let mut items = split_top_level_json_array_items(&raw);
    let mut matched_id = None;
    let mut updated = false;
    for item in items.iter_mut().rev() {
        if extract_json_string_field(item, "kind").as_deref() != Some("mailbox")
            || extract_json_string_field(item, "to_worker").as_deref() != Some(worker_name)
            || extract_json_string_field(item, "message_id").as_deref() != Some(message_id)
        {
            continue;
        }
        matched_id = extract_json_string_field(item, "request_id");
        let now = iso_timestamp();
        if extract_json_string_field(item, "status").as_deref() == Some("pending") {
            *item = upsert_json_string_field(item, "status", "notified");
            *item = upsert_json_string_field(item, "notified_at", &now);
        }
        *item = upsert_json_string_field(item, "status", "delivered");
        *item = upsert_json_string_field(item, "updated_at", &now);
        *item = upsert_json_string_field(item, "delivered_at", &now);
        updated = true;
        break;
    }
    if updated {
        write_atomic_text(&path, &format!("[{}]\n", items.join(",")))?;
    }
    Ok((matched_id, updated))
}

fn create_team_task(
    team_root: &Path,
    owner: Option<String>,
    subject: &str,
    description: &str,
    blocked_by: Option<Vec<String>>,
    requires_code_change: Option<bool>,
) -> Result<String, TeamError> {
    let next_id = read_next_task_id(team_root)?;
    let created_at = iso_timestamp();
    let task = TaskRecord {
        id: next_id.to_string(),
        subject: subject.to_string(),
        description: description.to_string(),
        status: "pending".to_string(),
        owner,
        blocked_by: blocked_by.unwrap_or_default(),
        requires_code_change,
        result: None,
        error: None,
        version: 1,
        claim: None,
        created_at,
        completed_at: None,
    };
    write_task_record(team_root, &task)?;
    update_next_task_id_files(team_root, next_id + 1)?;
    Ok(task_record_to_json(&task))
}

fn update_task_metadata(
    team_root: &Path,
    task_id: &str,
    subject: Option<String>,
    description: Option<String>,
    blocked_by: Option<Vec<String>>,
    requires_code_change: Option<bool>,
) -> Result<Option<String>, TeamError> {
    let Some(mut task) = read_task_record(team_root, task_id)? else {
        return Ok(None);
    };
    if let Some(subject) = subject {
        task.subject = subject;
    }
    if let Some(description) = description {
        task.description = description;
    }
    if let Some(blocked_by) = blocked_by {
        task.blocked_by = blocked_by;
    }
    if let Some(requires_code_change) = requires_code_change {
        task.requires_code_change = Some(requires_code_change);
    }
    task.version += 1;
    write_task_record(team_root, &task)?;
    Ok(Some(task_record_to_json(&task)))
}

fn claim_team_task(
    team_root: &Path,
    task_id: &str,
    worker: &str,
    expected_version: Option<u64>,
) -> Result<String, TeamError> {
    let workers = collect_team_worker_names(team_root)?;
    if !workers.iter().any(|candidate| candidate == worker) {
        return Ok("{\"ok\":false,\"error\":\"worker_not_found\"}".to_string());
    }
    let Some(mut task) = read_task_record(team_root, task_id)? else {
        return Ok("{\"ok\":false,\"error\":\"task_not_found\"}".to_string());
    };
    if task.blocked_by.iter().any(|dep| {
        read_task_record(team_root, dep)
            .ok()
            .flatten()
            .map(|dep_task| dep_task.status != "completed")
            .unwrap_or(true)
    }) {
        return Ok(format!(
            "{{\"ok\":false,\"error\":\"blocked_dependency\",\"dependencies\":{}}}",
            format_string_array_json(&task.blocked_by)
        ));
    }
    if expected_version.is_some_and(|version| version != task.version) {
        return Ok("{\"ok\":false,\"error\":\"claim_conflict\"}".to_string());
    }
    if matches!(task.status.as_str(), "completed" | "failed") {
        return Ok("{\"ok\":false,\"error\":\"already_terminal\"}".to_string());
    }
    if task.status == "in_progress" && task.claim.is_some() {
        return Ok("{\"ok\":false,\"error\":\"claim_conflict\"}".to_string());
    }
    let claim_token = generate_claim_token();
    task.status = "in_progress".to_string();
    task.owner = Some(worker.to_string());
    task.claim = Some(TaskClaimRecord {
        owner: worker.to_string(),
        token: claim_token.clone(),
        leased_until: iso_timestamp(),
    });
    task.version += 1;
    write_task_record(team_root, &task)?;
    Ok(format!(
        "{{\"ok\":true,\"task\":{},\"claimToken\":\"{}\"}}",
        task_record_to_json(&task),
        escape_json_string(&claim_token)
    ))
}

fn transition_team_task_status(
    team_root: &Path,
    team_name: &str,
    task_id: &str,
    from: &str,
    to: &str,
    claim_token: &str,
) -> Result<String, TeamError> {
    if !matches!(
        (from, to),
        ("in_progress", "completed") | ("in_progress", "failed")
    ) {
        return Ok("{\"ok\":false,\"error\":\"invalid_transition\"}".to_string());
    }
    let Some(mut task) = read_task_record(team_root, task_id)? else {
        return Ok("{\"ok\":false,\"error\":\"task_not_found\"}".to_string());
    };
    if task.status != from {
        return Ok("{\"ok\":false,\"error\":\"invalid_transition\"}".to_string());
    }
    if matches!(task.status.as_str(), "completed" | "failed") {
        return Ok("{\"ok\":false,\"error\":\"already_terminal\"}".to_string());
    }
    let claim = match &task.claim {
        Some(claim) if claim.token == claim_token => claim.clone(),
        _ => return Ok("{\"ok\":false,\"error\":\"claim_conflict\"}".to_string()),
    };
    task.status = to.to_string();
    task.claim = None;
    task.completed_at = Some(iso_timestamp());
    task.version += 1;
    write_task_record(team_root, &task)?;
    let event_type = if to == "completed" {
        "task_completed"
    } else {
        "task_failed"
    };
    let reason = if to == "failed" {
        Some(
            task.error
                .clone()
                .unwrap_or_else(|| "task_failed".to_string()),
        )
    } else {
        None
    };
    let _ = append_team_event(
        team_root,
        team_name,
        event_type,
        &claim.owner,
        Some(task.id.clone()),
        None,
        reason,
        None,
        None,
        None,
        None,
        None,
    )?;
    if to == "completed" {
        update_completed_task_snapshot(team_root, task_id)?;
    }
    Ok(format!(
        "{{\"ok\":true,\"task\":{}}}",
        task_record_to_json(&task)
    ))
}

fn update_completed_task_snapshot(team_root: &Path, task_id: &str) -> Result<(), TeamError> {
    let path = team_root.join("monitor-snapshot.json");
    let raw = read_optional_json(&path)?.unwrap_or_else(|| {
        "{\"taskStatusById\":{},\"workerAliveByName\":{},\"workerStateByName\":{},\"workerTurnCountByName\":{},\"workerTaskIdByName\":{},\"mailboxNotifiedByMessageId\":{},\"completedEventTaskIds\":{}}".to_string()
    });
    let completed =
        extract_json_value(&raw, "completedEventTaskIds").unwrap_or_else(|| "{}".to_string());
    let updated_completed = upsert_json_bool_field(&completed, task_id, true);
    write_atomic_text(
        &path,
        &(upsert_json_raw_field(&raw, "completedEventTaskIds", &updated_completed) + "\n"),
    )
}

fn release_team_task_claim(
    team_root: &Path,
    task_id: &str,
    claim_token: &str,
    _worker: &str,
) -> Result<String, TeamError> {
    let Some(mut task) = read_task_record(team_root, task_id)? else {
        return Ok("{\"ok\":false,\"error\":\"task_not_found\"}".to_string());
    };
    if matches!(task.status.as_str(), "completed" | "failed") {
        return Ok("{\"ok\":false,\"error\":\"already_terminal\"}".to_string());
    }
    if task.status == "pending" && task.claim.is_none() && task.owner.is_none() {
        return Ok(format!(
            "{{\"ok\":true,\"task\":{}}}",
            task_record_to_json(&task)
        ));
    }
    let claim = match &task.claim {
        Some(claim) if claim.token == claim_token => claim,
        _ => return Ok("{\"ok\":false,\"error\":\"claim_conflict\"}".to_string()),
    };
    if claim.owner != task.owner.clone().unwrap_or_default() {
        return Ok("{\"ok\":false,\"error\":\"claim_conflict\"}".to_string());
    }
    task.status = "pending".to_string();
    task.owner = None;
    task.claim = None;
    task.version += 1;
    write_task_record(team_root, &task)?;
    Ok(format!(
        "{{\"ok\":true,\"task\":{}}}",
        task_record_to_json(&task)
    ))
}

fn write_worker_heartbeat(
    team_root: &Path,
    worker: &str,
    pid: u64,
    turn_count: u64,
    alive: bool,
) -> Result<(), TeamError> {
    let path = team_root
        .join("workers")
        .join(worker)
        .join("heartbeat.json");
    write_atomic_text(
        &path,
        &format!(
            concat!(
                "{{\n",
                "  \"pid\": {},\n",
                "  \"last_turn_at\": \"{}\",\n",
                "  \"turn_count\": {},\n",
                "  \"alive\": {}\n",
                "}}\n"
            ),
            pid,
            escape_json_string(&iso_timestamp()),
            turn_count,
            alive,
        ),
    )
}

fn write_worker_inbox(team_root: &Path, worker: &str, content: &str) -> Result<(), TeamError> {
    write_atomic_text(
        &team_root.join("workers").join(worker).join("inbox.md"),
        content,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_worker_identity(
    team_root: &Path,
    worker: &str,
    index: u64,
    role: &str,
    assigned_tasks: Vec<String>,
    pid: Option<u64>,
    pane_id: Option<String>,
    working_dir: Option<String>,
    worktree_path: Option<String>,
    worktree_branch: Option<String>,
    worktree_detached: Option<bool>,
    team_state_root: Option<String>,
) -> Result<(), TeamError> {
    let mut raw = format!(
        concat!(
            "{{",
            "\"name\":\"{}\",",
            "\"index\":{},",
            "\"role\":\"{}\",",
            "\"assigned_tasks\":{}",
            "}}"
        ),
        escape_json_string(worker),
        index,
        escape_json_string(role),
        format_string_array_json(&assigned_tasks),
    );
    if let Some(pid) = pid {
        raw = upsert_json_number_field(&raw, "pid", pid);
    }
    if let Some(pane_id) = pane_id {
        raw = upsert_json_string_field(&raw, "pane_id", &pane_id);
    }
    if let Some(working_dir) = working_dir {
        raw = upsert_json_string_field(&raw, "working_dir", &working_dir);
    }
    if let Some(worktree_path) = worktree_path {
        raw = upsert_json_string_field(&raw, "worktree_path", &worktree_path);
    }
    if let Some(worktree_branch) = worktree_branch {
        raw = upsert_json_string_field(&raw, "worktree_branch", &worktree_branch);
    }
    if let Some(worktree_detached) = worktree_detached {
        raw = upsert_json_bool_field(&raw, "worktree_detached", worktree_detached);
    }
    if let Some(team_state_root) = team_state_root {
        raw = upsert_json_string_field(&raw, "team_state_root", &team_state_root);
    }
    write_atomic_text(
        &team_root.join("workers").join(worker).join("identity.json"),
        &(raw + "\n"),
    )
}

#[allow(clippy::too_many_arguments)]
fn append_team_event(
    team_root: &Path,
    team_name: &str,
    event_type: &str,
    worker: &str,
    task_id: Option<String>,
    message_id: Option<String>,
    reason: Option<String>,
    state: Option<String>,
    prev_state: Option<String>,
    to_worker: Option<String>,
    worker_count: Option<u64>,
    source_type: Option<String>,
) -> Result<String, TeamError> {
    let mut event = format!(
        concat!(
            "{{",
            "\"event_id\":\"{}\",",
            "\"team\":\"{}\",",
            "\"type\":\"{}\",",
            "\"worker\":\"{}\",",
            "\"created_at\":\"{}\"",
            "}}"
        ),
        escape_json_string(&generate_event_id()),
        escape_json_string(team_name),
        escape_json_string(event_type),
        escape_json_string(worker),
        escape_json_string(&iso_timestamp()),
    );
    if let Some(task_id) = task_id {
        event = upsert_json_string_field(&event, "task_id", &task_id);
    }
    if let Some(message_id) = message_id {
        event = upsert_json_string_field(&event, "message_id", &message_id);
    }
    if let Some(reason) = reason {
        event = upsert_json_string_field(&event, "reason", &reason);
    }
    if let Some(state) = state {
        event = upsert_json_string_field(&event, "state", &state);
    }
    if let Some(prev_state) = prev_state {
        event = upsert_json_string_field(&event, "prev_state", &prev_state);
    }
    if let Some(to_worker) = to_worker {
        event = upsert_json_string_field(&event, "to_worker", &to_worker);
    }
    if let Some(worker_count) = worker_count {
        event = upsert_json_number_field(&event, "worker_count", worker_count);
    }
    if let Some(source_type) = source_type {
        event = upsert_json_string_field(&event, "source_type", &source_type);
    }
    append_team_event_record(team_root, &event)?;
    Ok(event)
}

fn write_shutdown_request_file(
    team_root: &Path,
    worker: &str,
    requested_by: &str,
) -> Result<(), TeamError> {
    write_atomic_text(
        &team_root
            .join("workers")
            .join(worker)
            .join("shutdown-request.json"),
        &format!(
            "{{\"requested_at\":\"{}\",\"requested_by\":\"{}\"}}\n",
            escape_json_string(&iso_timestamp()),
            escape_json_string(requested_by),
        ),
    )
}

fn write_monitor_snapshot_file(team_root: &Path, snapshot: &str) -> Result<(), TeamError> {
    write_atomic_text(
        &team_root.join("monitor-snapshot.json"),
        &(snapshot.trim().to_string() + "\n"),
    )
}

fn read_shutdown_ack_json(
    team_root: &Path,
    worker: &str,
    min_updated_at: Option<&str>,
) -> Result<Option<String>, TeamError> {
    let ack = read_optional_json(
        team_root
            .join("workers")
            .join(worker)
            .join("shutdown-ack.json"),
    )?;
    let Some(ack_raw) = ack else {
        return Ok(None);
    };
    if let Some(min_updated_at) = min_updated_at {
        let updated_at = extract_json_string_field(&ack_raw, "updated_at").unwrap_or_default();
        if updated_at.is_empty() || updated_at.as_str() < min_updated_at {
            return Ok(None);
        }
    }
    Ok(Some(ack_raw))
}

fn write_task_approval_file(
    team_root: &Path,
    team_name: &str,
    task_id: &str,
    required: bool,
    status: &str,
    reviewer: &str,
    decision_reason: &str,
) -> Result<(), TeamError> {
    let approval = format!(
        concat!(
            "{{",
            "\"task_id\":\"{}\",",
            "\"required\":{},",
            "\"status\":\"{}\",",
            "\"reviewer\":\"{}\",",
            "\"decision_reason\":\"{}\",",
            "\"decided_at\":\"{}\"",
            "}}"
        ),
        escape_json_string(task_id),
        required,
        escape_json_string(status),
        escape_json_string(reviewer),
        escape_json_string(decision_reason),
        escape_json_string(&iso_timestamp()),
    );
    write_atomic_text(
        &team_root
            .join("approvals")
            .join(format!("task-{task_id}.json")),
        &(approval.clone() + "\n"),
    )?;
    append_team_event(
        team_root,
        team_name,
        "approval_decision",
        reviewer,
        Some(task_id.to_string()),
        None,
        Some(format!("{status}:{decision_reason}")),
        None,
        None,
        None,
        None,
        None,
    )?;
    Ok(())
}

fn remove_team_state(team_root: &Path) -> Result<(), TeamError> {
    match fs::remove_dir_all(team_root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(TeamError::runtime(format!(
            "failed to remove {}: {error}",
            team_root.display()
        ))),
    }
}

fn read_next_task_id(team_root: &Path) -> Result<u64, TeamError> {
    let config = read_optional_json(team_root.join("config.json"))?;
    if let Some(raw) = config
        .as_deref()
        .and_then(|raw| extract_json_value(raw, "next_task_id"))
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
    {
        return Ok(raw);
    }
    let manifest = read_optional_json(team_root.join("manifest.v2.json"))?;
    if let Some(raw) = manifest
        .as_deref()
        .and_then(|raw| extract_json_value(raw, "next_task_id"))
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
    {
        return Ok(raw);
    }
    let tasks_dir = team_root.join("tasks");
    let entries = match fs::read_dir(&tasks_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(1),
        Err(error) => {
            return Err(TeamError::runtime(format!(
                "failed to read {}: {error}",
                tasks_dir.display()
            )));
        }
    };
    let mut max_id = 0_u64;
    for entry in entries {
        let entry = entry
            .map_err(|error| TeamError::runtime(format!("failed to enumerate tasks: {error}")))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id) = name
            .strip_prefix("task-")
            .and_then(|value| value.strip_suffix(".json"))
            .and_then(|value| value.parse::<u64>().ok())
        {
            max_id = max_id.max(id);
        }
    }
    Ok(max_id + 1)
}

fn update_next_task_id_files(team_root: &Path, next_task_id: u64) -> Result<(), TeamError> {
    for file_name in ["config.json", "manifest.v2.json"] {
        let path = team_root.join(file_name);
        if let Some(raw) = read_optional_json(&path)? {
            let updated = upsert_json_number_field(&raw, "next_task_id", next_task_id);
            write_atomic_text(&path, &(updated + "\n"))?;
        }
    }
    Ok(())
}

fn read_task_record(team_root: &Path, task_id: &str) -> Result<Option<TaskRecord>, TeamError> {
    let path = team_root.join("tasks").join(format!("task-{task_id}.json"));
    let Some(raw) = read_optional_json(&path)? else {
        return Ok(None);
    };
    let claim = extract_json_value(&raw, "claim").and_then(|claim_raw| {
        Some(TaskClaimRecord {
            owner: extract_json_string_field(&claim_raw, "owner")?,
            token: extract_json_string_field(&claim_raw, "token")?,
            leased_until: extract_json_string_field(&claim_raw, "leased_until")?,
        })
    });
    Ok(Some(TaskRecord {
        id: extract_json_string_field(&raw, "id").unwrap_or_else(|| task_id.to_string()),
        subject: extract_json_string_field(&raw, "subject").unwrap_or_default(),
        description: extract_json_string_field(&raw, "description").unwrap_or_default(),
        status: extract_json_string_field(&raw, "status").unwrap_or_else(|| "pending".to_string()),
        owner: extract_json_string_field(&raw, "owner"),
        blocked_by: extract_json_value(&raw, "depends_on")
            .and_then(|value| parse_json_string_array(&value))
            .or_else(|| {
                extract_json_value(&raw, "blocked_by")
                    .and_then(|value| parse_json_string_array(&value))
            })
            .unwrap_or_default(),
        requires_code_change: extract_json_value(&raw, "requires_code_change").and_then(|value| {
            match value.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }
        }),
        result: extract_json_string_field(&raw, "result"),
        error: extract_json_string_field(&raw, "error"),
        version: extract_json_value(&raw, "version")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(1),
        claim,
        created_at: extract_json_string_field(&raw, "created_at").unwrap_or_else(iso_timestamp),
        completed_at: extract_json_string_field(&raw, "completed_at"),
    }))
}

fn task_record_to_json(task: &TaskRecord) -> String {
    let mut raw = format!(
        concat!(
            "{{",
            "\"id\":\"{}\",",
            "\"subject\":\"{}\",",
            "\"description\":\"{}\",",
            "\"status\":\"{}\",",
            "\"version\":{},",
            "\"created_at\":\"{}\"",
            "}}"
        ),
        escape_json_string(&task.id),
        escape_json_string(&task.subject),
        escape_json_string(&task.description),
        escape_json_string(&task.status),
        task.version,
        escape_json_string(&task.created_at),
    );
    if let Some(owner) = &task.owner {
        raw = upsert_json_string_field(&raw, "owner", owner);
    }
    if !task.blocked_by.is_empty() {
        raw = upsert_json_raw_field(
            &raw,
            "blocked_by",
            &format_string_array_json(&task.blocked_by),
        );
    }
    if let Some(requires_code_change) = task.requires_code_change {
        raw = upsert_json_bool_field(&raw, "requires_code_change", requires_code_change);
    }
    if let Some(result) = &task.result {
        raw = upsert_json_string_field(&raw, "result", result);
    }
    if let Some(error) = &task.error {
        raw = upsert_json_string_field(&raw, "error", error);
    }
    if let Some(claim) = &task.claim {
        raw = upsert_json_raw_field(
            &raw,
            "claim",
            &format!(
                "{{\"owner\":\"{}\",\"token\":\"{}\",\"leased_until\":\"{}\"}}",
                escape_json_string(&claim.owner),
                escape_json_string(&claim.token),
                escape_json_string(&claim.leased_until),
            ),
        );
    }
    if let Some(completed_at) = &task.completed_at {
        raw = upsert_json_string_field(&raw, "completed_at", completed_at);
    }
    raw
}

fn write_task_record(team_root: &Path, task: &TaskRecord) -> Result<(), TeamError> {
    write_atomic_text(
        &team_root
            .join("tasks")
            .join(format!("task-{}.json", task.id)),
        &(task_record_to_json(task) + "\n"),
    )
}

fn split_top_level_json_array_items(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return Vec::new();
    };
    let mut items = Vec::new();
    let mut start = 0_usize;
    let mut depth_brace = 0_i32;
    let mut depth_bracket = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in inner.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth_brace += 1,
            '}' => depth_brace -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            ',' if depth_brace == 0 && depth_bracket == 0 => {
                let value = inner[start..idx].trim();
                if !value.is_empty() {
                    items.push(value.to_string());
                }
                start = idx + 1;
            }
            _ => {}
        }
    }
    let value = inner[start..].trim();
    if !value.is_empty() {
        items.push(value.to_string());
    }
    items
}

fn generate_event_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    format!(
        "evt-{}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id()
    )
}

fn generate_claim_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    format!(
        "claim-{}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id()
    )
}

fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn json_success_execution(command: &str, operation: &str, data: &str) -> TeamExecution {
    let timestamp = iso_timestamp();
    execution(
        format!(
            "{{\"schema_version\":\"1.0\",\"timestamp\":\"{}\",\"command\":\"{}\",\"ok\":true,\"operation\":\"{}\",\"data\":{}}}\n",
            escape_json_string(&timestamp),
            escape_json_string(command),
            escape_json_string(operation),
            data
        ),
        String::new(),
        0,
    )
}

fn json_error_execution(
    command: &str,
    operation: &str,
    code: &str,
    message: &str,
) -> TeamExecution {
    let timestamp = iso_timestamp();
    execution(
        format!(
            "{{\"schema_version\":\"1.0\",\"timestamp\":\"{}\",\"command\":\"{}\",\"ok\":false,\"operation\":\"{}\",\"error\":{{\"code\":\"{}\",\"message\":\"{}\"}}}}\n",
            escape_json_string(&timestamp),
            escape_json_string(command),
            escape_json_string(operation),
            escape_json_string(code),
            escape_json_string(message)
        ),
        String::new(),
        1,
    )
}

fn iso_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    format!("{}.{:03}Z", now.as_secs(), now.subsec_millis())
}

fn generate_message_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    format!(
        "msg-{}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id()
    )
}

#[derive(Clone)]
struct EventQuery<'a> {
    after_event_id: Option<&'a str>,
    wakeable_only: bool,
    event_type: Option<&'a str>,
    worker: Option<&'a str>,
    task_id: Option<&'a str>,
}

#[derive(Clone)]
struct TeamEventRecord {
    raw: String,
    event_id: String,
    event_type: String,
    worker: String,
    task_id: Option<String>,
    state: Option<String>,
    prev_state: Option<String>,
}

fn read_team_events(
    cwd: &Path,
    team_name: &str,
    query: EventQuery<'_>,
) -> Result<Vec<TeamEventRecord>, TeamError> {
    let events_path = team_root(cwd, team_name)
        .join("events")
        .join("events.ndjson");
    let raw = match fs::read_to_string(&events_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(TeamError::runtime(format!(
                "failed to read {}: {error}",
                events_path.display()
            )));
        }
    };
    let mut started = query.after_event_id.is_none();
    let mut items = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Some(event_id) = extract_json_string_field(line, "event_id") else {
            continue;
        };
        if !started {
            if query.after_event_id == Some(event_id.as_str()) {
                started = true;
            }
            continue;
        }
        let event_type = extract_json_string_field(line, "type").unwrap_or_default();
        let source_type = extract_json_string_field(line, "source_type");
        let canonical_type = if event_type == "worker_idle" {
            "worker_state_changed".to_string()
        } else {
            event_type.clone()
        };
        if query.wakeable_only && !is_wakeable_event(&canonical_type) {
            continue;
        }
        if let Some(expected_type) = query.event_type {
            let matches = canonical_type == expected_type
                || (expected_type == "worker_idle"
                    && source_type.as_deref() == Some("worker_idle"))
                || (expected_type == "worker_idle" && event_type == "worker_idle");
            if !matches {
                continue;
            }
        }
        let worker = extract_json_string_field(line, "worker").unwrap_or_default();
        if let Some(expected_worker) = query.worker {
            if worker != expected_worker {
                continue;
            }
        }
        let task_id = extract_json_string_field(line, "task_id");
        if let Some(expected_task_id) = query.task_id {
            if task_id.as_deref() != Some(expected_task_id) {
                continue;
            }
        }
        let state = if event_type == "worker_idle" {
            Some("idle".to_string())
        } else {
            extract_json_string_field(line, "state")
        };
        let record = TeamEventRecord {
            raw: line.to_string(),
            event_id,
            event_type: canonical_type,
            worker,
            task_id,
            state,
            prev_state: extract_json_string_field(line, "prev_state"),
        };
        items.push(record);
    }
    Ok(items)
}

fn latest_event_id(cwd: &Path, team_name: &str) -> Result<String, TeamError> {
    Ok(read_team_events(
        cwd,
        team_name,
        EventQuery {
            after_event_id: None,
            wakeable_only: false,
            event_type: None,
            worker: None,
            task_id: None,
        },
    )?
    .last()
    .map(|event| event.event_id.clone())
    .unwrap_or_default())
}

fn is_wakeable_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "worker_state_changed"
            | "task_completed"
            | "task_failed"
            | "worker_stopped"
            | "message_received"
            | "leader_notification_deferred"
            | "all_workers_idle"
            | "team_leader_nudge"
    )
}

fn append_mailbox_message(
    team_root: &std::path::Path,
    worker_name: &str,
    message_json: &str,
) -> Result<(), TeamError> {
    let mailbox_dir = team_root.join("mailbox");
    fs::create_dir_all(&mailbox_dir).map_err(|error| {
        TeamError::runtime(format!(
            "failed to create {}: {error}",
            mailbox_dir.display()
        ))
    })?;
    let mailbox_path = mailbox_dir.join(format!("{worker_name}.json"));
    let updated = match fs::read_to_string(&mailbox_path) {
        Ok(raw) => {
            let existing_messages =
                extract_json_value(&raw, "messages").unwrap_or_else(|| "[]".to_string());
            let trimmed = existing_messages.trim();
            let merged_messages = if trimmed == "[]" {
                format!("[{message_json}]")
            } else {
                let without_suffix = trimmed.strip_suffix(']').unwrap_or(trimmed);
                format!("{without_suffix},{message_json}]")
            };
            format!(
                "{{\n  \"worker\": \"{}\",\n  \"messages\": {}\n}}\n",
                escape_json_string(worker_name),
                merged_messages
            )
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            format!(
                "{{\n  \"worker\": \"{}\",\n  \"messages\": [{}]\n}}\n",
                escape_json_string(worker_name),
                message_json
            )
        }
        Err(error) => {
            return Err(TeamError::runtime(format!(
                "failed to read {}: {error}",
                mailbox_path.display()
            )));
        }
    };
    write_atomic_text(&mailbox_path, &updated)
}

fn append_team_event_record(
    team_root: &std::path::Path,
    event_json: &str,
) -> Result<(), TeamError> {
    let events_dir = team_root.join("events");
    fs::create_dir_all(&events_dir).map_err(|error| {
        TeamError::runtime(format!(
            "failed to create {}: {error}",
            events_dir.display()
        ))
    })?;
    let events_path = events_dir.join("events.ndjson");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .map_err(|error| {
            TeamError::runtime(format!("failed to open {}: {error}", events_path.display()))
        })?;
    writeln!(file, "{event_json}")
        .map_err(|error| TeamError::runtime(format!("failed to append event: {error}")))?;
    Ok(())
}

fn read_team_summary(team_root: &Path) -> Result<String, TeamError> {
    let config = read_optional_json(team_root.join("config.json"))?
        .ok_or_else(|| TeamError::runtime("team_not_found"))?;
    let manifest = read_optional_json(team_root.join("manifest.v2.json"))?;
    let runtime = read_runtime_layout_evidence(&config, manifest.as_deref());
    let task_files = read_json_array_from_dir(&team_root.join("tasks"))?;
    let task_count = count_top_level_items(&task_files);
    let pending = count_occurrences(&task_files, "\"status\":\"pending\"");
    let blocked = count_occurrences(&task_files, "\"status\":\"blocked\"");
    let in_progress = count_occurrences(&task_files, "\"status\":\"in_progress\"");
    let completed = count_occurrences(&task_files, "\"status\":\"completed\"");
    let failed = count_occurrences(&task_files, "\"status\":\"failed\"");

    let workers_json = manifest
        .as_deref()
        .and_then(|raw| extract_json_value(raw, "workers"))
        .or_else(|| extract_json_value(&config, "workers"))
        .unwrap_or_else(|| "[]".to_string());

    let worker_names = collect_worker_names(&workers_json);
    let workers = worker_names
        .iter()
        .map(|worker_name| {
            let heartbeat = read_optional_json(
                team_root
                    .join("workers")
                    .join(worker_name)
                    .join("heartbeat.json"),
            )
            .unwrap_or(None)
            .unwrap_or_else(|| "null".to_string());
            let alive = if heartbeat == "null" {
                false
            } else {
                extract_json_value(&heartbeat, "alive")
                    .map(|v| v.trim() == "true")
                    .unwrap_or(false)
            };
            let last_turn_at = if heartbeat == "null" {
                "null".to_string()
            } else {
                extract_json_value(&heartbeat, "last_turn_at").unwrap_or_else(|| "null".to_string())
            };
            let turns_without_progress = if heartbeat == "null" {
                0
            } else {
                extract_json_value(&heartbeat, "turn_count")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(0)
            };
            format!(
                "{{\"name\":\"{}\",\"alive\":{},\"lastTurnAt\":{},\"turnsWithoutProgress\":{}}}",
                escape_json_string(worker_name),
                alive,
                last_turn_at,
                turns_without_progress
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    let non_reporting = worker_names
        .iter()
        .filter(|worker_name| {
            !team_root
                .join("workers")
                .join(worker_name)
                .join("heartbeat.json")
                .exists()
        })
        .map(|name| format!("\"{}\"", escape_json_string(name)))
        .collect::<Vec<_>>()
        .join(",");

    Ok(format!(
        concat!(
            "{{",
            "\"teamName\":\"{}\",",
            "\"workerCount\":{},",
            "\"tasks\":{{\"total\":{},\"pending\":{},\"blocked\":{},\"in_progress\":{},\"completed\":{},\"failed\":{}}},",
            "\"runtime\":{},",
            "\"workers\":[{}],",
            "\"nonReportingWorkers\":[{}]",
            "}}"
        ),
        escape_json_string(
            &extract_json_string_field(&config, "name").unwrap_or_else(|| "unknown".to_string())
        ),
        worker_names.len(),
        task_count,
        pending,
        blocked,
        in_progress,
        completed,
        failed,
        render_runtime_layout_json(&runtime),
        workers,
        non_reporting
    ))
}

fn read_runtime_layout_evidence_for_team_root(
    team_root: &Path,
) -> Result<RuntimeLayoutEvidence, TeamError> {
    let config = read_optional_json(team_root.join("config.json"))?
        .ok_or_else(|| TeamError::runtime("team_not_found"))?;
    let manifest = read_optional_json(team_root.join("manifest.v2.json"))?;
    Ok(read_runtime_layout_evidence(&config, manifest.as_deref()))
}

fn sync_prompt_layout(
    team_root: &Path,
    cwd: &Path,
    reason: &str,
    hud_mode_override: HudModeOverride,
    env: Option<&BTreeMap<OsString, OsString>>,
) -> Result<crate::team_layout::TeamLayoutSnapshot, TeamError> {
    sync_prompt_layout_from_state(
        team_root,
        &resolve_state_root(cwd, env.unwrap_or(&BTreeMap::new())),
        reason,
        hud_mode_override,
        env,
    )
    .map_err(|error| TeamError::runtime(format!("failed to sync native layout state: {error}")))?
    .ok_or_else(|| TeamError::runtime("native prompt layout state unavailable"))
}

fn sync_prompt_layout_if_available(
    team_root: &Path,
    cwd: &Path,
    reason: &str,
    hud_mode_override: HudModeOverride,
    env: Option<&BTreeMap<OsString, OsString>>,
) -> Option<crate::team_layout::TeamLayoutSnapshot> {
    sync_prompt_layout(team_root, cwd, reason, hud_mode_override, env).ok()
}

fn read_runtime_layout_evidence(
    config_raw: &str,
    manifest_raw: Option<&str>,
) -> RuntimeLayoutEvidence {
    let manifest_policy = manifest_raw.and_then(|raw| extract_json_value(raw, "policy"));
    let runtime_target = extract_json_string_field(config_raw, "runtime_session_id")
        .or_else(|| {
            manifest_raw.and_then(|raw| extract_json_string_field(raw, "runtime_session_id"))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let worker_launch_mode = extract_json_string_field(config_raw, "worker_launch_mode")
        .or_else(|| {
            manifest_policy
                .as_ref()
                .and_then(|raw| extract_json_string_field(raw, "worker_launch_mode"))
        })
        .unwrap_or_else(|| {
            if extract_json_string_field(config_raw, "tmux_session").is_some() {
                "interactive".to_string()
            } else {
                "prompt".to_string()
            }
        });
    let display_mode = manifest_policy
        .as_ref()
        .and_then(|raw| extract_json_string_field(raw, "display_mode"))
        .unwrap_or_else(|| "auto".to_string());
    let tmux_session = extract_json_string_field(config_raw, "tmux_session")
        .or_else(|| manifest_raw.and_then(|raw| extract_json_string_field(raw, "tmux_session")));
    let hud_pane_id = extract_json_string_field(config_raw, "hud_pane_id")
        .or_else(|| manifest_raw.and_then(|raw| extract_json_string_field(raw, "hud_pane_id")));
    let resize_hook_name =
        extract_json_string_field(config_raw, "resize_hook_name").or_else(|| {
            manifest_raw.and_then(|raw| extract_json_string_field(raw, "resize_hook_name"))
        });
    let resize_hook_target =
        extract_json_string_field(config_raw, "resize_hook_target").or_else(|| {
            manifest_raw.and_then(|raw| extract_json_string_field(raw, "resize_hook_target"))
        });
    let tmux_required = worker_launch_mode != "prompt";
    let no_tmux_proof = !tmux_required
        && tmux_session.is_none()
        && hud_pane_id.is_none()
        && resize_hook_name.is_none()
        && resize_hook_target.is_none();
    let spawn_strategy = if worker_launch_mode == "prompt" {
        "process".to_string()
    } else {
        "tmux-pane".to_string()
    };
    let reflow_strategy = if worker_launch_mode == "prompt" {
        "state-backed".to_string()
    } else if resize_hook_name.is_some() {
        "tmux-resize-hook".to_string()
    } else {
        "tmux-layout".to_string()
    };
    let hud_strategy = if worker_launch_mode == "prompt" {
        "state-backed".to_string()
    } else if hud_pane_id.is_some() {
        "tmux-pane".to_string()
    } else {
        "unregistered".to_string()
    };

    RuntimeLayoutEvidence {
        runtime_target,
        worker_launch_mode,
        display_mode,
        spawn_strategy,
        reflow_strategy,
        hud_strategy,
        tmux_required,
        tmux_session,
        hud_pane_id,
        resize_hook_name,
        resize_hook_target,
        no_tmux_proof,
    }
}

fn render_runtime_layout_line(runtime: &RuntimeLayoutEvidence) -> String {
    format!(
        "layout: display={} launch={} spawn={} reflow={} hud={}",
        runtime.display_mode,
        runtime.worker_launch_mode,
        runtime.spawn_strategy,
        runtime.reflow_strategy,
        runtime.hud_strategy
    )
}

fn render_runtime_tmux_line(runtime: &RuntimeLayoutEvidence) -> String {
    let resize_hook = runtime.resize_hook_name.as_deref().unwrap_or("none");
    format!(
        "tmux: required={} session={} hud_pane={} resize_hook={} no_tmux={}",
        if runtime.tmux_required {
            "true"
        } else {
            "false"
        },
        runtime.tmux_session.as_deref().unwrap_or("null"),
        runtime.hud_pane_id.as_deref().unwrap_or("null"),
        resize_hook,
        if runtime.no_tmux_proof {
            "true"
        } else {
            "false"
        }
    )
}

fn render_runtime_layout_json(runtime: &RuntimeLayoutEvidence) -> String {
    format!(
        concat!(
            "{{",
            "\"runtime_target\":\"{}\",",
            "\"worker_launch_mode\":\"{}\",",
            "\"display_mode\":\"{}\",",
            "\"spawn_strategy\":\"{}\",",
            "\"reflow_strategy\":\"{}\",",
            "\"hud_strategy\":\"{}\",",
            "\"tmux_required\":{},",
            "\"tmux_session\":{},",
            "\"hud_pane_id\":{},",
            "\"resize_hook_name\":{},",
            "\"resize_hook_target\":{},",
            "\"no_tmux_proof\":{}",
            "}}"
        ),
        escape_json_string(&runtime.runtime_target),
        escape_json_string(&runtime.worker_launch_mode),
        escape_json_string(&runtime.display_mode),
        escape_json_string(&runtime.spawn_strategy),
        escape_json_string(&runtime.reflow_strategy),
        escape_json_string(&runtime.hud_strategy),
        if runtime.tmux_required {
            "true"
        } else {
            "false"
        },
        render_optional_json_string(runtime.tmux_session.as_deref()),
        render_optional_json_string(runtime.hud_pane_id.as_deref()),
        render_optional_json_string(runtime.resize_hook_name.as_deref()),
        render_optional_json_string(runtime.resize_hook_target.as_deref()),
        if runtime.no_tmux_proof {
            "true"
        } else {
            "false"
        }
    )
}

fn render_optional_json_string(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("\"{}\"", escape_json_string(value)),
        None => "null".to_string(),
    }
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn collect_worker_names(workers_json: &str) -> Vec<String> {
    workers_json
        .split("\"name\"")
        .skip(1)
        .filter_map(|chunk| {
            let candidate = extract_json_string_field(&format!("{{\"name\"{chunk}"), "name")?;
            if candidate.starts_with("worker-") {
                Some(candidate)
            } else {
                None
            }
        })
        .collect()
}

fn read_recent_events_json(
    cwd: &Path,
    team_name: &str,
    window: usize,
) -> Result<String, TeamError> {
    let events = read_team_events(
        cwd,
        team_name,
        EventQuery {
            after_event_id: None,
            wakeable_only: false,
            event_type: None,
            worker: None,
            task_id: None,
        },
    )?;
    let start = events.len().saturating_sub(window);
    Ok(format!(
        "[{}]",
        events[start..]
            .iter()
            .map(|event| event.raw.as_str())
            .collect::<Vec<_>>()
            .join(",")
    ))
}

fn build_idle_state_json(
    team_name: &str,
    summary: &str,
    snapshot: &str,
    recent_events: &str,
) -> String {
    let worker_names = collect_worker_names(
        &extract_json_value(summary, "workers").unwrap_or_else(|| "[]".to_string()),
    );
    let idle_workers = worker_names
        .iter()
        .filter(|name| snapshot.contains(&format!("\"{}\":\"idle\"", name)))
        .map(|name| format!("\"{}\"", escape_json_string(name)))
        .collect::<Vec<_>>();
    let non_idle_workers = worker_names
        .iter()
        .filter(|name| !snapshot.contains(&format!("\"{}\":\"idle\"", name)))
        .map(|name| format!("\"{}\"", escape_json_string(name)))
        .collect::<Vec<_>>();
    format!(
        concat!(
            "{{",
            "\"team_name\":\"{}\",",
            "\"worker_count\":{},",
            "\"idle_worker_count\":{},",
            "\"idle_workers\":[{}],",
            "\"non_idle_workers\":[{}],",
            "\"all_workers_idle\":{},",
            "\"source\":{{\"summary_available\":{},\"snapshot_available\":{},\"recent_event_count\":{}}}",
            "}}"
        ),
        escape_json_string(team_name),
        worker_names.len(),
        idle_workers.len(),
        idle_workers.join(","),
        non_idle_workers.join(","),
        !worker_names.is_empty() && idle_workers.len() == worker_names.len(),
        summary != "null",
        snapshot != "null",
        count_top_level_items(recent_events)
    )
}

fn build_stall_state_json(
    team_name: &str,
    summary: &str,
    snapshot: &str,
    recent_events: &str,
) -> String {
    let idle = build_idle_state_json(team_name, summary, snapshot, recent_events);
    let non_reporting =
        extract_json_value(summary, "nonReportingWorkers").unwrap_or_else(|| "[]".to_string());
    let pending = extract_json_value(summary, "tasks")
        .and_then(|tasks| extract_json_value(&tasks, "pending"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let blocked = extract_json_value(summary, "tasks")
        .and_then(|tasks| extract_json_value(&tasks, "blocked"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let in_progress = extract_json_value(summary, "tasks")
        .and_then(|tasks| extract_json_value(&tasks, "in_progress"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let pending_task_count = pending + blocked + in_progress;
    let stalled = non_reporting != "[]"
        || pending_task_count > 0 && idle.contains("\"all_workers_idle\":true");
    format!(
        concat!(
            "{{",
            "\"team_name\":\"{}\",",
            "\"team_stalled\":{},",
            "\"leader_stale\":{},",
            "\"stalled_workers\":{},",
            "\"dead_workers\":[],",
            "\"pending_task_count\":{},",
            "\"all_workers_idle\":{},",
            "\"idle_workers\":{},",
            "\"reasons\":[],",
            "\"source\":{{\"summary_available\":{},\"snapshot_available\":{},\"recent_event_count\":{}}}",
            "}}"
        ),
        escape_json_string(team_name),
        stalled,
        pending_task_count > 0 && idle.contains("\"all_workers_idle\":true"),
        non_reporting,
        pending_task_count,
        if idle.contains("\"all_workers_idle\":true") {
            "true"
        } else {
            "false"
        },
        extract_json_value(&idle, "idle_workers").unwrap_or_else(|| "[]".to_string()),
        summary != "null",
        snapshot != "null",
        count_top_level_items(recent_events)
    )
}

fn write_atomic_text(path: &Path, text: &str) -> Result<(), TeamError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            TeamError::runtime(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("json")
    ));
    fs::write(&tmp_path, text).map_err(|error| {
        TeamError::runtime(format!("failed to write {}: {error}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, path).map_err(|error| {
        TeamError::runtime(format!(
            "failed to replace {} with {}: {error}",
            path.display(),
            tmp_path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TEAM_API_HELP, TEAM_HELP, extract_json_value, run_team};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("omx-team-{label}-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn prints_team_help_for_help_variants() {
        for args in [vec![], vec!["--help".to_string()], vec!["help".to_string()]] {
            let result =
                run_team(&args, std::path::Path::new("."), &BTreeMap::new()).expect("team help");
            assert_eq!(String::from_utf8(result.stdout).expect("utf8"), TEAM_HELP);
            assert!(result.stderr.is_empty());
            assert_eq!(result.exit_code, 0);
        }
    }

    #[test]
    fn prints_team_api_help_for_help_variants() {
        for args in [
            vec!["api".to_string()],
            vec!["api".to_string(), "--help".to_string()],
            vec!["api".to_string(), "help".to_string()],
        ] {
            let result = run_team(&args, std::path::Path::new("."), &BTreeMap::new())
                .expect("team api help");
            assert_eq!(
                String::from_utf8(result.stdout).expect("utf8"),
                TEAM_API_HELP
            );
            assert!(result.stderr.is_empty());
            assert_eq!(result.exit_code, 0);
        }
    }

    #[test]
    fn prints_operation_specific_help_for_send_message() {
        let result = run_team(
            &[
                "api".to_string(),
                "send-message".to_string(),
                "--help".to_string(),
            ],
            std::path::Path::new("."),
            &BTreeMap::new(),
        )
        .expect("team api send-message help");
        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert!(stdout.contains("Usage: omx team api send-message --input <json> [--json]"));
        assert!(stdout.contains("team_name"));
        assert!(stdout.contains("from_worker"));
        assert!(stdout.contains("to_worker"));
        assert!(stdout.contains("body"));
        assert!(result.stderr.is_empty());
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn team_api_help_mentions_idle_and_stall_operations() {
        assert!(TEAM_API_HELP.contains("read-idle-state"));
        assert!(TEAM_API_HELP.contains("read-stall-state"));
    }

    #[test]
    fn send_message_json_writes_mailbox_and_returns_envelope() {
        let cwd = temp_dir("send-message");
        let team_root = cwd.join(".omx/state/team/fixture-team");
        fs::create_dir_all(&team_root).expect("create team root");

        let result = run_team(
            &[
                "api".to_string(),
                "send-message".to_string(),
                "--input".to_string(),
                "{\"team_name\":\"fixture-team\",\"from_worker\":\"worker-1\",\"to_worker\":\"leader-fixed\",\"body\":\"ACK\"}".to_string(),
                "--json".to_string(),
            ],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("send-message json");

        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert!(stdout.contains("\"ok\":true"));
        assert!(stdout.contains("\"operation\":\"send-message\""));
        assert!(stdout.contains("\"to_worker\":\"leader-fixed\""));

        let mailbox =
            fs::read_to_string(team_root.join("mailbox/leader-fixed.json")).expect("read mailbox");
        assert!(mailbox.contains("\"worker\": \"leader-fixed\""));
        assert!(mailbox.contains("\"from_worker\":\"worker-1\""));
        assert!(mailbox.contains("\"body\":\"ACK\""));

        let events =
            fs::read_to_string(team_root.join("events/events.ndjson")).expect("read events");
        assert!(events.contains("\"type\":\"message_received\""));
        assert!(events.contains("\"message_id\":\"msg-"));
    }

    #[test]
    fn read_idle_and_summary_json_return_structured_data() {
        let cwd = temp_dir("idle-summary");
        let team_root = cwd.join(".omx/state/team/fixture-team");
        fs::create_dir_all(team_root.join("workers/worker-1")).expect("worker dir");
        fs::create_dir_all(team_root.join("tasks")).expect("tasks dir");
        fs::write(
            team_root.join("config.json"),
            r#"{"name":"fixture-team","workers":[{"name":"worker-1"}]}"#,
        )
        .expect("config");
        fs::write(
            team_root.join("monitor-snapshot.json"),
            r#"{"workerStateByName":{"worker-1":"idle"}}"#,
        )
        .expect("snapshot");
        fs::write(
            team_root.join("workers/worker-1/heartbeat.json"),
            r#"{"alive":true,"last_turn_at":"2026-03-11T00:00:00.000Z","turn_count":3}"#,
        )
        .expect("heartbeat");
        fs::write(
            team_root.join("tasks/task-1.json"),
            r#"{"id":"1","status":"pending"}"#,
        )
        .expect("task");

        let summary = run_team(
            &[
                "api".to_string(),
                "get-summary".to_string(),
                "--input".to_string(),
                "{\"team_name\":\"fixture-team\"}".to_string(),
                "--json".to_string(),
            ],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("summary");
        let summary_stdout = String::from_utf8(summary.stdout).expect("utf8");
        assert!(summary_stdout.contains("\"operation\":\"get-summary\""));
        assert!(summary_stdout.contains("\"teamName\":\"fixture-team\""));
        assert!(summary_stdout.contains("\"workerCount\":1"));

        let idle = run_team(
            &[
                "api".to_string(),
                "read-idle-state".to_string(),
                "--input".to_string(),
                "{\"team_name\":\"fixture-team\"}".to_string(),
                "--json".to_string(),
            ],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("idle");
        let idle_stdout = String::from_utf8(idle.stdout).expect("utf8");
        assert!(idle_stdout.contains("\"operation\":\"read-idle-state\""));
        assert!(idle_stdout.contains("\"idle_worker_count\":1"));
        assert!(idle_stdout.contains("\"all_workers_idle\":true"));
    }

    #[test]
    fn prints_missing_team_message_when_status_state_is_absent() {
        let cwd = temp_dir("missing");
        let result = run_team(
            &["status".to_string(), "missing-team".to_string()],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("team status");
        assert_eq!(
            String::from_utf8(result.stdout).expect("utf8"),
            "No team state found for missing-team\n"
        );
    }

    #[test]
    fn prints_team_status_summary_from_state_snapshots() {
        let cwd = temp_dir("status");
        let team_root = cwd.join(".omx/state/team/fixture-team");
        fs::create_dir_all(team_root.join("tasks")).expect("create task dir");
        fs::write(
            team_root.join("manifest.v2.json"),
            r#"{
  "name": "fixture-team",
  "workers": [
    { "name": "worker-1" },
    { "name": "worker-2" },
    { "name": "worker-3" }
  ]
}
"#,
        )
        .expect("write manifest");
        fs::write(
            team_root.join("phase.json"),
            r#"{
  "current_phase": "team-exec"
}
"#,
        )
        .expect("write phase");
        fs::write(
            team_root.join("monitor-snapshot.json"),
            r#"{
  "workerAliveByName": {
    "worker-1": true,
    "worker-2": false,
    "worker-3": true
  },
  "workerStateByName": {
    "worker-1": "idle",
    "worker-2": "unknown",
    "worker-3": "working"
  }
}
"#,
        )
        .expect("write snapshot");
        fs::write(
            team_root.join("tasks/task-1.json"),
            "{\"status\":\"pending\"}\n",
        )
        .expect("task 1");
        fs::write(
            team_root.join("tasks/task-2.json"),
            "{\"status\":\"in_progress\"}\n",
        )
        .expect("task 2");
        fs::write(
            team_root.join("tasks/task-3.json"),
            "{\"status\":\"completed\"}\n",
        )
        .expect("task 3");
        fs::write(
            team_root.join("tasks/task-4.json"),
            "{\"status\":\"failed\"}\n",
        )
        .expect("task 4");

        let result = run_team(
            &["status".to_string(), "fixture-team".to_string()],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("team status");
        assert_eq!(
            String::from_utf8(result.stdout).expect("utf8"),
            concat!(
                "team=fixture-team phase=team-exec\n",
                "runtime target: unknown\n",
                "layout: display=auto launch=prompt spawn=process reflow=state-backed hud=state-backed\n",
                "tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true\n",
                "workers: total=3 dead=1 non_reporting=1\n",
                "tasks: total=4 pending=1 blocked=0 in_progress=1 completed=1 failed=1\n",
            )
        );
    }

    #[test]
    fn starts_prompt_mode_team_and_writes_runtime_neutral_state() {
        let cwd = temp_dir("prompt-start");
        let env = BTreeMap::from([(OsString::from("OMX_SESSION_ID"), OsString::from("sess-123"))]);

        let result = run_team(
            &[
                "2:executor".to_string(),
                "bootstrap".to_string(),
                "native".to_string(),
                "team".to_string(),
            ],
            &cwd,
            &env,
        )
        .expect("team start");

        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert!(stdout.contains("Team started: bootstrap-native-team"));
        assert!(stdout.contains("runtime target: prompt-bootstrap-native-team"));
        assert!(stdout.contains("workers: 2"));
        assert!(stdout.contains("linked_ralph=false"));
        assert!(stdout.contains(
            "layout: display=auto launch=prompt spawn=process reflow=state-backed hud=state-backed"
        ));
        assert!(stdout.contains(
            "tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true"
        ));
        assert!(stdout.contains("tasks: total=2 pending=2"));

        let config =
            fs::read_to_string(cwd.join(".omx/state/team/bootstrap-native-team/config.json"))
                .expect("read config");
        assert!(config.contains("\"worker_launch_mode\": \"prompt\""));
        assert!(config.contains("\"runtime_session_id\": \"prompt-bootstrap-native-team\""));
        assert!(config.contains("\"tmux_session\": null"));

        let manifest =
            fs::read_to_string(cwd.join(".omx/state/team/bootstrap-native-team/manifest.v2.json"))
                .expect("read manifest");
        assert!(manifest.contains("\"worker_launch_mode\":\"prompt\""));
        assert!(manifest.contains("\"runtime_session_id\": \"prompt-bootstrap-native-team\""));
        assert!(manifest.contains("\"session_id\":\"sess-123\""));

        let status = run_team(
            &["status".to_string(), "bootstrap-native-team".to_string()],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("status");
        let status_stdout = String::from_utf8(status.stdout).expect("utf8");
        assert!(status_stdout.contains("team=bootstrap-native-team phase=team-exec"));
        assert!(status_stdout.contains("runtime target: prompt-bootstrap-native-team"));
        assert!(status_stdout.contains(
            "layout: display=auto launch=prompt spawn=process reflow=state-backed hud=state-backed"
        ));
        assert!(status_stdout.contains(
            "tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true"
        ));
        assert!(status_stdout.contains("tasks: total=2 pending=2"));
    }

    #[test]
    fn starts_linked_ralph_team_when_prefixed_with_ralph() {
        let cwd = temp_dir("linked-ralph");
        let result = run_team(
            &["ralph".to_string(), "fix".to_string(), "all".to_string()],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("team start");
        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert!(stdout.contains("linked_ralph=true"));

        let events = fs::read_to_string(cwd.join(".omx/state/team/fix-all/events/events.ndjson"))
            .expect("read events");
        assert!(events.contains("linked_ralph_bootstrap"));
    }

    #[cfg(unix)]
    #[test]
    fn prompt_team_start_spawns_worker_processes_and_shutdown_terminates_them() {
        let cwd = temp_dir("prompt-spawn");
        let bin_dir = cwd.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        let codex_path = bin_dir.join("codex");
        let capture_path = cwd.join("argv.txt");
        fs::write(
            &codex_path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"{}\"\ntrap 'exit 0' TERM\nwhile true; do sleep 1; done\n",
                capture_path.display()
            ),
        )
        .expect("write fake codex");
        let mut perms = fs::metadata(&codex_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&codex_path, perms).expect("chmod");

        let path = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let env = BTreeMap::from([(OsString::from("PATH"), OsString::from(path))]);

        let result = run_team(
            &[
                "1:executor".to_string(),
                "spawn".to_string(),
                "worker".to_string(),
            ],
            &cwd,
            &env,
        )
        .expect("team start");
        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert!(stdout.contains("Team started: spawn-worker"));

        let config = fs::read_to_string(cwd.join(".omx/state/team/spawn-worker/config.json"))
            .expect("read config");
        let pid = extract_json_value(&config, "pid")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .expect("pid");
        assert!(pid > 0);
        let deadline = Instant::now() + Duration::from_millis(2000);
        while Instant::now() < deadline && !capture_path.exists() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let argv = fs::read_to_string(&capture_path).expect("capture argv");
        assert!(argv.contains("send-message"));
        assert!(argv.contains("claim-task"));
        assert!(argv.contains("task-1.json"));
        run_team(
            &[
                "shutdown".to_string(),
                "spawn-worker".to_string(),
                "--force".to_string(),
            ],
            &cwd,
            &BTreeMap::new(),
        )
        .expect("shutdown");

        assert!(!cwd.join(".omx/state/team/spawn-worker").exists());
    }
}
