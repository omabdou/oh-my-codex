use std::env;
use std::ffi::OsString;
use std::fs::{create_dir_all, read_dir, read_to_string, remove_dir_all, write, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRunInput {
    pub team_name: String,
    pub agent_types: Vec<String>,
    pub tasks: Vec<RuntimeTaskInput>,
    pub cwd: String,
    pub worker_count: usize,
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTaskInput {
    pub subject: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeStartResult {
    pane_ids: Vec<String>,
    leader_pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeMonitorSnapshot {
    phase: String,
    pending: usize,
    in_progress: usize,
    completed: usize,
    failed: usize,
    dead_workers: Vec<String>,
    monitor_ms: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeTaskStatus {
    id: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskResult {
    task_id: String,
    status: String,
    summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeTaskStateRecord {
    id: String,
    status: String,
    requires_code_change: bool,
    result: String,
}

pub fn run_runtime(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err(format!(
            "runtime-run does not accept positional arguments yet: {}",
            args.join(" ")
        ));
    }

    let raw_input = read_runtime_input_raw()?;
    let input = parse_runtime_input(&raw_input)?;
    write_panes_sidecar_placeholder()?;

    let start_result = start_team(&input)?;
    write_panes_sidecar(&start_result.pane_ids, &start_result.leader_pane_id)?;

    let started_at = Instant::now();
    loop {
        sleep(Duration::from_millis(input.poll_interval_ms));

        let snapshot = match monitor_team(&input)? {
            Some(snapshot) => snapshot,
            None => continue,
        };

        let live_panes = read_live_pane_state(&input);
        if let Some((ref pane_ids, ref leader_pane_id)) = live_panes {
            let _ = write_panes_sidecar(pane_ids, leader_pane_id);
        }

        eprintln!(
            "[runtime-run] phase={} pending={} inProgress={} completed={} failed={} dead={} monitorMs={}",
            snapshot.phase,
            snapshot.pending,
            snapshot.in_progress,
            snapshot.completed,
            snapshot.failed,
            snapshot.dead_workers.len(),
            snapshot.monitor_ms,
        );

        if snapshot.phase == "complete" {
            shutdown_and_emit_result(&input, "completed", started_at.elapsed().as_secs_f64())?;
            return Ok(());
        }
        if snapshot.phase == "failed" || snapshot.phase == "cancelled" {
            shutdown_and_emit_result(&input, "failed", started_at.elapsed().as_secs_f64())?;
            return Err("runtime-run observed terminal failure phase".to_string());
        }

        let has_outstanding_work = snapshot.pending + snapshot.in_progress > 0;
        let live_worker_pane_count = live_panes.map(|(pane_ids, _)| pane_ids.len()).unwrap_or(0);
        let (dead_worker_failure, fixing_with_no_workers) = detect_dead_worker_failure(
            snapshot.dead_workers.len(),
            live_worker_pane_count,
            has_outstanding_work,
            &snapshot.phase,
        );
        if dead_worker_failure || fixing_with_no_workers {
            eprintln!(
                "[runtime-run] Failure detected: deadWorkerFailure={} fixingWithNoWorkers={}",
                dead_worker_failure, fixing_with_no_workers,
            );
            shutdown_and_emit_result(&input, "failed", started_at.elapsed().as_secs_f64())?;
            return Err("runtime-run detected dead worker failure".to_string());
        }
    }
}

fn start_team(input: &RuntimeRunInput) -> Result<RuntimeStartResult, String> {
    let sanitized_team_name = sanitize_team_name(&input.team_name)?;
    ensure_tmux_available()?;
    ensure_inside_tmux()?;

    let worker_clis = normalize_agent_types(&input.agent_types, input.worker_count)?;
    let start_task = input
        .tasks
        .iter()
        .map(|task| task.subject.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    let created_at = iso_timestamp();
    let team_state_root = PathBuf::from(&input.cwd).join(".omx").join("state");
    let team_root = team_state_root.join("team").join(&sanitized_team_name);
    if team_root.exists() {
        return Err(format!(
            "team state already exists for {} at {}",
            sanitized_team_name,
            team_root.display()
        ));
    }

    initialize_team_state(
        &sanitized_team_name,
        &start_task,
        input,
        &worker_clis,
        &team_state_root,
        &created_at,
    )?;

    let session =
        match create_team_session(&sanitized_team_name, input, &worker_clis, &team_state_root) {
            Ok(session) => session,
            Err(error) => {
                let _ = remove_dir_all(&team_root);
                return Err(error);
            }
        };

    finalize_team_state(
        &sanitized_team_name,
        &start_task,
        input,
        &worker_clis,
        &team_state_root,
        &created_at,
        &session,
    )?;

    send_worker_bootstrap_prompts(&sanitized_team_name, input, &session.worker_pane_ids)?;

    Ok(RuntimeStartResult {
        pane_ids: session.worker_pane_ids,
        leader_pane_id: session.leader_pane_id,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerCli {
    Codex,
    Claude,
    Gemini,
}

impl WorkerCli {
    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TeamSessionStart {
    team_target: String,
    leader_pane_id: String,
    worker_pane_ids: Vec<String>,
}

fn sanitize_team_name(name: &str) -> Result<String, String> {
    let lowered = name.to_ascii_lowercase();
    let mut out = String::new();
    let mut last_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    let truncated = trimmed
        .chars()
        .take(30)
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if truncated.is_empty() {
        Err("sanitizeTeamName: empty after sanitization".to_string())
    } else {
        Ok(truncated)
    }
}

fn normalize_agent_types(raw: &[String], worker_count: usize) -> Result<Vec<WorkerCli>, String> {
    let providers = raw
        .iter()
        .map(|entry| match entry.trim().to_ascii_lowercase().as_str() {
            "codex" => Ok(WorkerCli::Codex),
            "claude" => Ok(WorkerCli::Claude),
            "gemini" => Ok(WorkerCli::Gemini),
            other => Err(format!(
                "Invalid agentTypes entries: {other}. Expected codex|claude|gemini."
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;

    if providers.len() != 1 && providers.len() != worker_count {
        return Err(format!(
            "agentTypes length must be 1 or {worker_count}; received {}.",
            providers.len()
        ));
    }

    if providers.len() == 1 {
        Ok(vec![providers[0]; worker_count])
    } else {
        Ok(providers)
    }
}

fn initialize_team_state(
    team_name: &str,
    start_task: &str,
    input: &RuntimeRunInput,
    worker_clis: &[WorkerCli],
    team_state_root: &Path,
    created_at: &str,
) -> Result<(), String> {
    let team_root = team_state_root.join("team").join(team_name);
    let workers_root = team_root.join("workers");
    let tasks_root = team_root.join("tasks");
    let claims_root = team_root.join("claims");
    let mailbox_root = team_root.join("mailbox");
    let dispatch_root = team_root.join("dispatch");
    let events_root = team_root.join("events");
    let approvals_root = team_root.join("approvals");

    for dir in [
        &workers_root,
        &tasks_root,
        &claims_root,
        &mailbox_root,
        &dispatch_root,
        &events_root,
        &approvals_root,
    ] {
        create_dir_all(dir).map_err(|err| format!("failed creating {}: {err}", dir.display()))?;
    }
    write(
        dispatch_root.join("requests.json"),
        "[]
",
    )
    .map_err(|err| format!("failed writing dispatch requests: {err}"))?;

    for (index, task) in input.tasks.iter().enumerate() {
        let body = format!(
            "{{\"id\":{},\"subject\":{},\"description\":{},\"status\":\"pending\",\"depends_on\":[],\"version\":1,\"created_at\":{}}}
",
            json_string(&(index + 1).to_string()),
            json_string(&task.subject),
            json_string(&task.description),
            json_string(created_at),
        );
        write(tasks_root.join(format!("task-{}.json", index + 1)), body)
            .map_err(|err| format!("failed writing task state: {err}"))?;
    }

    for (index, worker_cli) in worker_clis.iter().enumerate() {
        let worker_name = format!("worker-{}", index + 1);
        let worker_dir = workers_root.join(&worker_name);
        create_dir_all(&worker_dir)
            .map_err(|err| format!("failed creating worker dir {}: {err}", worker_dir.display()))?;
        write(
            worker_dir.join("status.json"),
            format!(
                "{{\"state\":\"idle\",\"updated_at\":{}}}
",
                json_string(created_at)
            ),
        )
        .map_err(|err| format!("failed writing worker status: {err}"))?;
        write(
            worker_dir.join("inbox.md"),
            generate_worker_inbox(team_name, &worker_name, input, *worker_cli),
        )
        .map_err(|err| format!("failed writing worker inbox: {err}"))?;
    }

    write_phase_state(team_name, &input.cwd, "team-exec", true)?;

    let worker_json = (1..=input.worker_count)
        .map(|index| {
            format!(
                "{{\"name\":{},\"index\":{},\"role\":\"executor\",\"worker_cli\":{},\"assigned_tasks\":[]}}",
                json_string(&format!("worker-{index}")),
                index,
                json_string(worker_clis[index - 1].as_str()),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let team_state_root_str = team_state_root.to_string_lossy().to_string();
    let leader_worker = env::var("OMX_TEAM_WORKER").unwrap_or_else(|_| "leader-fixed".to_string());

    let config = format!(
        "{{\"name\":{},\"task\":{},\"agent_type\":\"executor\",\"worker_launch_mode\":\"interactive\",\"lifecycle_profile\":\"default\",\"worker_count\":{},\"max_workers\":20,\"workers\":[{}],\"created_at\":{},\"tmux_session\":{},\"next_task_id\":{},\"leader_cwd\":{},\"team_state_root\":{},\"workspace_mode\":\"single\",\"leader_pane_id\":null,\"hud_pane_id\":null,\"resize_hook_name\":null,\"resize_hook_target\":null,\"next_worker_index\":{}}}
",
        json_string(team_name),
        json_string(start_task),
        input.worker_count,
        worker_json,
        json_string(created_at),
        json_string(&format!("omx-team-{team_name}")),
        input.tasks.len() + 1,
        json_string(&input.cwd),
        json_string(&team_state_root_str),
        input.worker_count + 1,
    );
    write(team_root.join("config.json"), config)
        .map_err(|err| format!("failed writing team config: {err}"))?;

    let manifest = format!(
        "{{\"schema_version\":2,\"name\":{},\"task\":{},\"leader\":{{\"session_id\":{},\"worker_id\":{},\"role\":\"leader\"}},\"policy\":{{\"display_mode\":\"split_pane\",\"worker_launch_mode\":\"interactive\",\"dispatch_mode\":\"hook_preferred_with_fallback\",\"dispatch_ack_timeout_ms\":2000}},\"governance\":{{\"delegation_only\":false,\"plan_approval_required\":false,\"nested_teams_allowed\":false,\"one_team_per_leader_session\":true,\"cleanup_requires_all_workers_inactive\":true}},\"lifecycle_profile\":\"default\",\"permissions_snapshot\":{{\"approval_mode\":{},\"sandbox_mode\":{},\"network_access\":true}},\"tmux_session\":{},\"worker_count\":{},\"workers\":[{}],\"next_task_id\":{},\"created_at\":{},\"leader_cwd\":{},\"team_state_root\":{},\"workspace_mode\":\"single\",\"leader_pane_id\":null,\"hud_pane_id\":null,\"resize_hook_name\":null,\"resize_hook_target\":null,\"next_worker_index\":{}}}
",
        json_string(team_name),
        json_string(start_task),
        json_string(&format!("native-runtime-{team_name}")),
        json_string(&leader_worker),
        json_string(&env::var("CODEX_APPROVAL_MODE").unwrap_or_else(|_| "never".to_string())),
        json_string(&env::var("CODEX_SANDBOX_MODE").unwrap_or_else(|_| "danger-full-access".to_string())),
        json_string(&format!("omx-team-{team_name}")),
        input.worker_count,
        worker_json,
        input.tasks.len() + 1,
        json_string(created_at),
        json_string(&input.cwd),
        json_string(&team_state_root_str),
        input.worker_count + 1,
    );
    write(team_root.join("manifest.v2.json"), manifest)
        .map_err(|err| format!("failed writing team manifest: {err}"))?;

    Ok(())
}

fn finalize_team_state(
    team_name: &str,
    start_task: &str,
    input: &RuntimeRunInput,
    worker_clis: &[WorkerCli],
    team_state_root: &Path,
    created_at: &str,
    session: &TeamSessionStart,
) -> Result<(), String> {
    let team_root = team_state_root.join("team").join(team_name);
    let workers_json = session
        .worker_pane_ids
        .iter()
        .enumerate()
        .map(|(index, pane_id)| {
            let worker_name = format!("worker-{}", index + 1);
            let pid = get_pane_pid(pane_id).unwrap_or(0);
            let identity = format!(
                "{{\"name\":{},\"index\":{},\"role\":\"executor\",\"worker_cli\":{},\"assigned_tasks\":[],\"pid\":{},\"pane_id\":{},\"working_dir\":{},\"team_state_root\":{}}}
",
                json_string(&worker_name),
                index + 1,
                json_string(worker_clis[index].as_str()),
                pid,
                json_string(pane_id),
                json_string(&input.cwd),
                json_string(&team_state_root.to_string_lossy()),
            );
            let worker_dir = team_root.join("workers").join(&worker_name);
            let _ = write(worker_dir.join("identity.json"), &identity);
            format!(
                "{{\"name\":{},\"index\":{},\"role\":\"executor\",\"worker_cli\":{},\"assigned_tasks\":[],\"pid\":{},\"pane_id\":{},\"working_dir\":{},\"team_state_root\":{}}}",
                json_string(&worker_name),
                index + 1,
                json_string(worker_clis[index].as_str()),
                pid,
                json_string(pane_id),
                json_string(&input.cwd),
                json_string(&team_state_root.to_string_lossy()),
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    let base_tmux_session = session
        .team_target
        .split(':')
        .next()
        .unwrap_or(&session.team_target)
        .to_string();
    let leader_worker = env::var("OMX_TEAM_WORKER").unwrap_or_else(|_| "leader-fixed".to_string());

    let config = format!(
        "{{\"name\":{},\"task\":{},\"agent_type\":\"executor\",\"worker_launch_mode\":\"interactive\",\"lifecycle_profile\":\"default\",\"worker_count\":{},\"max_workers\":20,\"workers\":[{}],\"created_at\":{},\"tmux_session\":{},\"next_task_id\":{},\"leader_cwd\":{},\"team_state_root\":{},\"workspace_mode\":\"single\",\"leader_pane_id\":{},\"hud_pane_id\":null,\"resize_hook_name\":null,\"resize_hook_target\":null,\"next_worker_index\":{}}}
",
        json_string(team_name),
        json_string(start_task),
        input.worker_count,
        workers_json,
        json_string(created_at),
        json_string(&base_tmux_session),
        input.tasks.len() + 1,
        json_string(&input.cwd),
        json_string(&team_state_root.to_string_lossy()),
        json_string(&session.leader_pane_id),
        input.worker_count + 1,
    );
    write(team_root.join("config.json"), config)
        .map_err(|err| format!("failed updating team config: {err}"))?;

    let manifest = format!(
        "{{\"schema_version\":2,\"name\":{},\"task\":{},\"leader\":{{\"session_id\":{},\"worker_id\":{},\"role\":\"leader\"}},\"policy\":{{\"display_mode\":\"split_pane\",\"worker_launch_mode\":\"interactive\",\"dispatch_mode\":\"hook_preferred_with_fallback\",\"dispatch_ack_timeout_ms\":2000}},\"governance\":{{\"delegation_only\":false,\"plan_approval_required\":false,\"nested_teams_allowed\":false,\"one_team_per_leader_session\":true,\"cleanup_requires_all_workers_inactive\":true}},\"lifecycle_profile\":\"default\",\"permissions_snapshot\":{{\"approval_mode\":{},\"sandbox_mode\":{},\"network_access\":true}},\"tmux_session\":{},\"worker_count\":{},\"workers\":[{}],\"next_task_id\":{},\"created_at\":{},\"leader_cwd\":{},\"team_state_root\":{},\"workspace_mode\":\"single\",\"leader_pane_id\":{},\"hud_pane_id\":null,\"resize_hook_name\":null,\"resize_hook_target\":null,\"next_worker_index\":{}}}
",
        json_string(team_name),
        json_string(start_task),
        json_string(&format!("native-runtime-{team_name}")),
        json_string(&leader_worker),
        json_string(&env::var("CODEX_APPROVAL_MODE").unwrap_or_else(|_| "never".to_string())),
        json_string(&env::var("CODEX_SANDBOX_MODE").unwrap_or_else(|_| "danger-full-access".to_string())),
        json_string(&base_tmux_session),
        input.worker_count,
        workers_json,
        input.tasks.len() + 1,
        json_string(created_at),
        json_string(&input.cwd),
        json_string(&team_state_root.to_string_lossy()),
        json_string(&session.leader_pane_id),
        input.worker_count + 1,
    );
    write(team_root.join("manifest.v2.json"), manifest)
        .map_err(|err| format!("failed updating team manifest: {err}"))?;

    Ok(())
}

fn generate_worker_inbox(
    team_name: &str,
    worker_name: &str,
    input: &RuntimeRunInput,
    worker_cli: WorkerCli,
) -> String {
    let task_list = input
        .tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            format!(
                "- **Task {}**: {}
  Description: {}
  Status: pending",
                index + 1,
                task.subject,
                task.description,
            )
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        );

    format!(
        r#"# Worker Assignment: {worker_name}

**Team:** {team_name}
**Role:** executor
**Worker Name:** {worker_name}
**CLI:** {worker_cli}

## Available Team Tasks

{task_list}

## Instructions

1. Read this inbox first, then read the first ready task from .omx/state/team/{team_name}/tasks/task-<id>.json.
2. Send startup ACK to the lead mailbox before task work:
   omx team api send-message --input "{{"team_name":"{team_name}","from_worker":"{worker_name}","to_worker":"leader-fixed","body":"ACK: {worker_name} initialized"}}" --json
3. Claim the first non-blocked task via omx team api claim-task, complete it, and report concrete progress back to leader-fixed.
4. When notified, read mailbox messages with omx team api mailbox-list and mark them delivered.
5. Continue assigned work or the next feasible task until the lead says stop.
"#,
        worker_name = worker_name,
        team_name = team_name,
        worker_cli = worker_cli.as_str(),
        task_list = task_list,
    )
}

fn ensure_tmux_available() -> Result<(), String> {
    let output = Command::new("tmux")
        .arg("-V")
        .output()
        .map_err(|err| format!("failed to launch tmux: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err("tmux is not available".to_string())
    }
}

fn ensure_inside_tmux() -> Result<(), String> {
    if env::var("TMUX")
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("team mode requires running inside tmux leader pane".to_string())
    }
}

fn create_team_session(
    team_name: &str,
    input: &RuntimeRunInput,
    worker_clis: &[WorkerCli],
    team_state_root: &Path,
) -> Result<TeamSessionStart, String> {
    let tmux_pane_target = env::var("TMUX_PANE").ok();
    let mut args = vec!["display-message".to_string(), "-p".to_string()];
    if let Some(target) = tmux_pane_target
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        args.push("-t".to_string());
        args.push(target.clone());
    }
    args.push("#S:#I #{pane_id}".to_string());
    let context = Command::new("tmux")
        .args(&args)
        .output()
        .map_err(|err| format!("failed to detect current tmux target: {err}"))?;
    if !context.status.success() {
        return Err(String::from_utf8_lossy(&context.stderr).trim().to_string());
    }
    let context_stdout = String::from_utf8_lossy(&context.stdout).trim().to_string();
    let mut context_parts = context_stdout.split_whitespace();
    let session_and_window = context_parts.next().unwrap_or_default();
    let leader_pane_id = context_parts.next().unwrap_or_default().to_string();
    let mut sw_parts = session_and_window.split(':');
    let session_name = sw_parts.next().unwrap_or_default();
    let window_index = sw_parts.next().unwrap_or_default();
    if session_name.is_empty() || window_index.is_empty() || !leader_pane_id.starts_with('%') {
        return Err(format!(
            "failed to parse current tmux target: {context_stdout}"
        ));
    }
    let team_target = format!("{session_name}:{window_index}");

    let mut worker_pane_ids = Vec::new();
    let mut rollback_pane_ids: Vec<String> = Vec::new();
    let mut right_stack_root: Option<String> = None;

    for (index, worker_cli) in worker_clis.iter().enumerate() {
        let split_direction = if index == 0 { "-h" } else { "-v" };
        let split_target = right_stack_root.as_deref().unwrap_or(&leader_pane_id);
        let command =
            build_worker_start_command(team_name, index + 1, *worker_cli, input, team_state_root);
        let output = Command::new("tmux")
            .args([
                "split-window",
                split_direction,
                "-t",
                split_target,
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-c",
                &input.cwd,
                &command,
            ])
            .output()
            .map_err(|err| format!("failed to create worker pane {}: {err}", index + 1))?;
        if !output.status.success() {
            for pane_id in rollback_pane_ids.iter() {
                let _ = kill_tmux_pane(pane_id);
            }
            return Err(format!(
                "failed to create worker pane {}: {}",
                index + 1,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !pane_id.starts_with('%') {
            for pane_id in rollback_pane_ids.iter() {
                let _ = kill_tmux_pane(pane_id);
            }
            return Err(format!(
                "failed to capture worker pane id for worker {}",
                index + 1
            ));
        }
        if index == 0 {
            right_stack_root = Some(pane_id.clone());
        }
        rollback_pane_ids.push(pane_id.clone());
        worker_pane_ids.push(pane_id);
    }

    let _ = Command::new("tmux")
        .args(["select-layout", "-t", &team_target, "main-vertical"])
        .output();
    let _ = Command::new("tmux")
        .args(["select-pane", "-t", &leader_pane_id])
        .output();
    sleep(Duration::from_millis(500));

    Ok(TeamSessionStart {
        team_target,
        leader_pane_id,
        worker_pane_ids,
    })
}

fn resolve_worker_command(worker_cli: WorkerCli) -> String {
    match worker_cli {
        WorkerCli::Codex => env::var("OMX_LEADER_CLI_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "codex".to_string()),
        _ => worker_cli.as_str().to_string(),
    }
}

fn build_worker_start_command(
    team_name: &str,
    worker_index: usize,
    worker_cli: WorkerCli,
    input: &RuntimeRunInput,
    team_state_root: &Path,
) -> String {
    let shell = env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string());
    let rc_prefix = if shell.ends_with("zsh") {
        "if [ -f ~/.zshrc ]; then source ~/.zshrc; fi; "
    } else if shell.ends_with("bash") {
        "if [ -f ~/.bashrc ]; then source ~/.bashrc; fi; "
    } else {
        ""
    };
    let worker_name = format!("worker-{worker_index}");
    let cli_command = shell_quote_single(&resolve_worker_command(worker_cli));
    let launch_args = env::var("OMX_TEAM_WORKER_LAUNCH_ARGS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!(" {value}"))
        .unwrap_or_default();
    let inner = format!(
        "{rc_prefix}exec {cli_command}{launch_args}",
        rc_prefix = rc_prefix,
        cli_command = cli_command,
        launch_args = launch_args,
    );
    let env_parts = [
        format!(
            "OMX_TEAM_WORKER={}",
            shell_quote_single(&format!("{team_name}/{worker_name}"))
        ),
        format!(
            "OMX_TEAM_STATE_ROOT={}",
            shell_quote_single(&team_state_root.to_string_lossy())
        ),
        format!("OMX_TEAM_LEADER_CWD={}", shell_quote_single(&input.cwd)),
        format!(
            "OMX_TEAM_WORKER_CLI={}",
            shell_quote_single(worker_cli.as_str())
        ),
    ];
    format!(
        "env {} {} -lc {}",
        env_parts.join(" "),
        shell_quote_single(&shell),
        shell_quote_single(&inner),
    )
}

fn shell_quote_single(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn get_pane_pid(pane_id: &str) -> Option<u64> {
    let output = Command::new("tmux")
        .args(["list-panes", "-t", pane_id, "-F", "#{pane_pid}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn send_worker_bootstrap_prompts(
    team_name: &str,
    input: &RuntimeRunInput,
    worker_pane_ids: &[String],
) -> Result<(), String> {
    sleep(Duration::from_millis(1500));
    for (index, pane_id) in worker_pane_ids.iter().enumerate() {
        let worker_name = format!("worker-{}", index + 1);
        let prompt = format!(
            "Read .omx/state/team/{team_name}/workers/{worker_name}/inbox.md, start work now, report concrete progress, then continue assigned work or next feasible task."
        );
        crate::tmux::send_to_pane(pane_id, &prompt, true)?;
    }
    let _ = input;
    Ok(())
}

fn iso_timestamp() -> String {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !value.is_empty() {
                return value;
            }
        }
    }
    "1970-01-01T00:00:00Z".to_string()
}

fn monitor_team(input: &RuntimeRunInput) -> Result<Option<RuntimeMonitorSnapshot>, String> {
    let monitor_started = Instant::now();
    let team_dir = team_dir(&input.team_name, &input.cwd);
    if !team_dir.exists() {
        return Ok(None);
    }

    reclaim_expired_task_claims(&input.team_name, &input.cwd)?;
    let task_states = list_task_states(&input.team_name, &input.cwd);
    let pending = task_states
        .iter()
        .filter(|task| task.status.as_str() == "pending")
        .count();
    let blocked = task_states
        .iter()
        .filter(|task| task.status.as_str() == "blocked")
        .count();
    let in_progress = task_states
        .iter()
        .filter(|task| task.status.as_str() == "in_progress")
        .count();
    let completed = task_states
        .iter()
        .filter(|task| task.status.as_str() == "completed")
        .count();
    let failed = task_states
        .iter()
        .filter(|task| task.status.as_str() == "failed")
        .count();

    let verification_pending = task_states.iter().any(|task| {
        task.status == "completed"
            && task.requires_code_change
            && !has_structured_verification_evidence(&task.result)
    });

    let worker_infos = list_worker_infos(&input.team_name, &input.cwd);
    let dead_workers = worker_infos
        .iter()
        .filter(|worker| !worker.alive)
        .map(|worker| worker.name.clone())
        .collect::<Vec<_>>();

    let all_tasks_terminal = pending == 0 && blocked == 0 && in_progress == 0;
    let dead_worker_stall =
        !worker_infos.is_empty() && dead_workers.len() >= worker_infos.len() && !all_tasks_terminal;

    let phase = if dead_worker_stall {
        "failed".to_string()
    } else if all_tasks_terminal && verification_pending {
        "team-verify".to_string()
    } else if all_tasks_terminal && failed == 0 {
        "complete".to_string()
    } else if all_tasks_terminal && failed > 0 {
        "team-fix".to_string()
    } else {
        "team-exec".to_string()
    };

    let monitor_ms = monitor_started.elapsed().as_millis() as usize;
    let task_statuses = task_states
        .iter()
        .map(|task| RuntimeTaskStatus {
            id: task.id.clone(),
            status: task.status.clone(),
        })
        .collect::<Vec<_>>();
    write_phase_state(&input.team_name, &input.cwd, &phase, failed == 0)?;
    write_monitor_snapshot(
        &input.team_name,
        &input.cwd,
        &task_statuses,
        &worker_infos,
        &collect_mailbox_notified_map(&input.team_name, &input.cwd),
        monitor_ms,
    )?;
    sync_root_team_mode_state_on_terminal_phase(&input.team_name, &phase, &input.cwd)?;
    sync_linked_ralph_mode_state_on_terminal_phase(
        &input.team_name,
        &phase,
        &input.cwd,
        &iso_timestamp(),
    )?;

    Ok(Some(RuntimeMonitorSnapshot {
        phase,
        pending,
        in_progress,
        completed,
        failed,
        dead_workers,
        monitor_ms,
    }))
}

fn shutdown_and_emit_result(
    input: &RuntimeRunInput,
    status: &str,
    duration_seconds: f64,
) -> Result<(), String> {
    let ralph = read_linked_ralph_profile(&input.team_name, &input.cwd);
    let force = status != "completed";
    match shutdown_team(&input.team_name, &input.cwd, force, ralph) {
        Ok(()) => {}
        Err(error)
            if !force
                && (error.contains("shutdown_gate_blocked")
                    || error.contains("shutdown_rejected")) =>
        {
            shutdown_team(&input.team_name, &input.cwd, true, ralph)?;
        }
        Err(error) => {
            eprintln!("[runtime-run] shutdownTeam error: {error}");
        }
    }

    let task_results = collect_task_results(input);
    println!(
        "{{\"status\":{},\"teamName\":{},\"taskResults\":{},\"duration\":{},\"workerCount\":{}}}",
        json_string(status),
        json_string(&input.team_name),
        json_task_results(&task_results),
        duration_seconds,
        input.worker_count,
    );
    Ok(())
}

fn shutdown_team(team_name: &str, cwd: &str, force: bool, ralph: bool) -> Result<(), String> {
    let statuses = list_task_statuses(team_name, cwd);
    let pending = statuses
        .iter()
        .filter(|status| status.status.as_str() == "pending")
        .count();
    let blocked = statuses
        .iter()
        .filter(|status| status.status.as_str() == "blocked")
        .count();
    let in_progress = statuses
        .iter()
        .filter(|status| status.status.as_str() == "in_progress")
        .count();
    let failed = statuses
        .iter()
        .filter(|status| status.status.as_str() == "failed")
        .count();

    if !force {
        let has_active_work = pending > 0 || blocked > 0 || in_progress > 0;
        if has_active_work || (failed > 0 && !ralph) {
            let _ = append_team_event(
                team_name,
                cwd,
                "shutdown_gate",
                "leader-fixed",
                &format!(
                    "allowed=false pending={pending} blocked={blocked} in_progress={in_progress} failed={failed}"
                ),
            );
            return Err(format!(
                "shutdown_gate_blocked:pending={pending},blocked={blocked},in_progress={in_progress},failed={failed}"
            ));
        }
    }
    if force {
        let _ = append_team_event(
            team_name,
            cwd,
            "shutdown_gate_forced",
            "leader-fixed",
            "force_bypass",
        );
    }

    let config_path = team_dir(team_name, cwd).join("config.json");
    if let Ok(raw) = read_to_string(&config_path) {
        for worker_name in extract_worker_names(&raw) {
            let requested_at = iso_timestamp();
            let _ =
                write_shutdown_request(team_name, &worker_name, "leader-fixed", cwd, &requested_at);
            if let Some(ack) = read_shutdown_ack(team_name, &worker_name, cwd, Some(&requested_at))
            {
                let reason = ack
                    .reason
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "no_reason".to_string());
                let event_reason = if ack.status == "reject" {
                    format!("reject:{reason}")
                } else {
                    "accept".to_string()
                };
                let _ =
                    append_team_event(team_name, cwd, "shutdown_ack", &worker_name, &event_reason);
                if ack.status == "reject" && !force {
                    return Err(format!("shutdown_rejected:{worker_name}:{reason}"));
                }
            }
        }
        for pane_id in extract_object_string_values(&raw, "pane_id") {
            let _ = kill_tmux_pane(&pane_id);
        }
        if let Some(hud_pane_id) = extract_json_string(&raw, "hud_pane_id") {
            let _ = kill_tmux_pane(&hud_pane_id);
        }
        if let Some(hook_target) = extract_json_string(&raw, "resize_hook_target") {
            if let Some(hook_name) = extract_json_string(&raw, "resize_hook_name") {
                let _ = unregister_resize_hook(&hook_target, &hook_name);
            }
        }
    }

    if ralph {
        let _ = append_team_event(
            team_name,
            cwd,
            "ralph_cleanup_summary",
            "leader-fixed",
            &format!(
                "total={} completed={} failed={} pending={} force={force}",
                statuses.len(),
                statuses
                    .iter()
                    .filter(|status| status.status == "completed")
                    .count(),
                failed,
                pending,
            ),
        );
        sync_linked_ralph_mode_state_on_terminal_phase(
            team_name,
            "cancelled",
            cwd,
            &iso_timestamp(),
        )?;
    }

    let team_root = team_dir(team_name, cwd);
    if team_root.exists() {
        remove_dir_all(&team_root).map_err(|err| format!("cleanupTeamState failed: {err}"))?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerMonitorInfo {
    name: String,
    alive: bool,
    pane_id: Option<String>,
    state: String,
    turn_count: u64,
    current_task_id: String,
}

fn team_dir(team_name: &str, cwd: &str) -> PathBuf {
    PathBuf::from(cwd)
        .join(".omx")
        .join("state")
        .join("team")
        .join(team_name)
}

fn list_task_states(team_name: &str, cwd: &str) -> Vec<RuntimeTaskStateRecord> {
    let tasks_dir = team_dir(team_name, cwd).join("tasks");
    let Ok(entries) = read_dir(tasks_dir) else {
        return Vec::new();
    };

    let mut tasks = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let raw = read_to_string(&path).ok()?;
            let id = extract_json_string(&raw, "id").or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().replace("task-", ""))
            })?;
            let status = extract_json_string(&raw, "status")?;
            let requires_code_change =
                extract_json_bool(&raw, "requires_code_change").unwrap_or(false);
            let result = extract_json_string(&raw, "result")
                .or_else(|| extract_json_string(&raw, "summary"))
                .unwrap_or_default();
            Some(RuntimeTaskStateRecord {
                id,
                status,
                requires_code_change,
                result,
            })
        })
        .collect::<Vec<_>>();
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    tasks
}

fn list_task_statuses(team_name: &str, cwd: &str) -> Vec<RuntimeTaskStatus> {
    list_task_states(team_name, cwd)
        .into_iter()
        .map(|task| RuntimeTaskStatus {
            id: task.id,
            status: task.status,
        })
        .collect()
}

fn reclaim_expired_task_claims(team_name: &str, cwd: &str) -> Result<(), String> {
    let tasks_dir = team_dir(team_name, cwd).join("tasks");
    let Ok(entries) = read_dir(tasks_dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(raw) = read_to_string(&path) else {
            continue;
        };
        let Some(status) = extract_json_string(&raw, "status") else {
            continue;
        };
        if status != "in_progress" {
            continue;
        }
        let Some(leased_until) = extract_json_string(&raw, "leased_until") else {
            continue;
        };
        if !iso_timestamp_is_expired(&leased_until) {
            continue;
        }

        let Some(id) = extract_json_string(&raw, "id").or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().replace("task-", ""))
        }) else {
            continue;
        };
        let version = extract_json_number(&raw, "version").unwrap_or(1) + 1;
        let subject = extract_json_string(&raw, "subject").unwrap_or_default();
        let description = extract_json_string(&raw, "description").unwrap_or_default();
        let created_at = extract_json_string(&raw, "created_at").unwrap_or_else(iso_timestamp);

        write(
            &path,
            format!(
                "{{\"id\":{},\"subject\":{},\"description\":{},\"status\":\"pending\",\"depends_on\":[],\"version\":{},\"created_at\":{}}}\n",
                json_string(&id),
                json_string(&subject),
                json_string(&description),
                version,
                json_string(&created_at),
            ),
        )
        .map_err(|err| format!("failed reclaiming expired task claim: {err}"))?;
    }

    Ok(())
}

fn iso_timestamp_is_expired(value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    let now = iso_timestamp();
    value <= now.as_str()
}

fn has_structured_verification_evidence(summary: &str) -> bool {
    let text = summary.trim();
    if text.is_empty() {
        return false;
    }

    let lowercase = text.to_ascii_lowercase();
    let has_verification_section = lowercase.contains("verification:")
        || lowercase.contains("verification evidence:")
        || lowercase.contains("## verification");
    if !has_verification_section {
        return false;
    }

    lowercase.contains("pass")
        || lowercase.contains("passed")
        || lowercase.contains("fail")
        || lowercase.contains("failed")
        || text.contains('`')
        || lowercase.contains("command")
        || lowercase.contains("test")
        || lowercase.contains("build")
        || lowercase.contains("typecheck")
        || lowercase.contains("lint")
}

fn list_worker_infos(team_name: &str, cwd: &str) -> Vec<WorkerMonitorInfo> {
    let workers_dir = team_dir(team_name, cwd).join("workers");
    let Ok(entries) = read_dir(workers_dir) else {
        return Vec::new();
    };

    let mut workers = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
        else {
            continue;
        };
        let identity_raw = read_to_string(path.join("identity.json")).unwrap_or_default();
        let status_raw = read_to_string(path.join("status.json")).unwrap_or_default();
        let heartbeat_raw = read_to_string(path.join("heartbeat.json")).unwrap_or_default();
        let pane_id = extract_json_string(&identity_raw, "pane_id");
        let pid = extract_json_number(&identity_raw, "pid");
        let alive = if let Some(ref pane_id) = pane_id {
            is_tmux_pane_alive(&pane_id)
        } else if let Some(pid) = pid {
            is_pid_alive(pid as i32)
        } else {
            false
        };
        let state =
            extract_json_string(&status_raw, "state").unwrap_or_else(|| "unknown".to_string());
        let current_task_id =
            extract_json_string(&status_raw, "current_task_id").unwrap_or_default();
        let turn_count = extract_json_number(&heartbeat_raw, "turn_count").unwrap_or(0);
        workers.push(WorkerMonitorInfo {
            name,
            alive,
            pane_id,
            state,
            turn_count,
            current_task_id,
        });
    }
    workers
}

fn write_monitor_snapshot(
    team_name: &str,
    cwd: &str,
    task_statuses: &[RuntimeTaskStatus],
    worker_infos: &[WorkerMonitorInfo],
    mailbox_notified: &str,
    monitor_ms: usize,
) -> Result<(), String> {
    let previous_snapshot_raw =
        read_to_string(team_dir(team_name, cwd).join("monitor-snapshot.json")).unwrap_or_default();
    let previous_mailbox_notified =
        extract_json_object_body(&previous_snapshot_raw, "mailboxNotifiedByMessageId")
            .unwrap_or_default();
    let previous_completed_event_task_ids =
        extract_json_object_body(&previous_snapshot_raw, "completedEventTaskIds")
            .unwrap_or_default();
    let merged_mailbox_notified =
        merge_json_object_entries(&previous_mailbox_notified, mailbox_notified);

    let task_status_by_id = task_statuses
        .iter()
        .map(|task| format!("{}:{}", json_string(&task.id), json_string(&task.status)))
        .collect::<Vec<_>>()
        .join(",");
    let worker_alive_by_name = worker_infos
        .iter()
        .map(|worker| {
            format!(
                "{}:{}",
                json_string(&worker.name),
                if worker.alive { "true" } else { "false" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let worker_state_by_name = worker_infos
        .iter()
        .map(|worker| {
            format!(
                "{}:{}",
                json_string(&worker.name),
                json_string(&worker.state)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let worker_turn_count_by_name = worker_infos
        .iter()
        .map(|worker| format!("{}:{}", json_string(&worker.name), worker.turn_count))
        .collect::<Vec<_>>()
        .join(",");
    let worker_task_id_by_name = worker_infos
        .iter()
        .map(|worker| {
            format!(
                "{}:{}",
                json_string(&worker.name),
                json_string(&worker.current_task_id)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let updated_at = iso_timestamp();
    let snapshot = format!(
        "{{\"taskStatusById\":{{{}}},\"workerAliveByName\":{{{}}},\"workerStateByName\":{{{}}},\"workerTurnCountByName\":{{{}}},\"workerTaskIdByName\":{{{}}},\"mailboxNotifiedByMessageId\":{{{}}},\"completedEventTaskIds\":{{{}}},\"monitorTimings\":{{\"list_tasks_ms\":{},\"worker_scan_ms\":{},\"mailbox_delivery_ms\":0,\"total_ms\":{},\"updated_at\":{}}}}}\n",
        task_status_by_id,
        worker_alive_by_name,
        worker_state_by_name,
        worker_turn_count_by_name,
        worker_task_id_by_name,
        merged_mailbox_notified,
        previous_completed_event_task_ids,
        monitor_ms,
        monitor_ms,
        monitor_ms,
        json_string(&updated_at),
    );

    write(
        team_dir(team_name, cwd).join("monitor-snapshot.json"),
        snapshot,
    )
    .map_err(|err| format!("failed writing monitor snapshot: {err}"))
}

fn collect_mailbox_notified_map(team_name: &str, cwd: &str) -> String {
    let mailbox_dir = team_dir(team_name, cwd).join("mailbox");
    let Ok(entries) = read_dir(mailbox_dir) else {
        return String::new();
    };

    let mut notified = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(raw) = read_to_string(path) else {
            continue;
        };
        for message_body in extract_json_array_entries_for_key(&raw, "messages") {
            let Some(message_id) = extract_json_string(message_body, "message_id") else {
                continue;
            };
            let Some(notified_at) = extract_json_string(message_body, "notified_at") else {
                continue;
            };
            if notified_at.trim().is_empty() {
                continue;
            }
            notified.push(format!(
                "{}:{}",
                json_string(&message_id),
                json_string(&notified_at)
            ));
        }
    }
    notified.join(",")
}

fn merge_json_object_entries(previous: &str, current: &str) -> String {
    if previous.trim().is_empty() {
        return current.to_string();
    }
    if current.trim().is_empty() {
        return previous.to_string();
    }

    let mut entries = previous.to_string();
    let current_ids = split_json_object_entries(current)
        .into_iter()
        .filter_map(|entry| extract_json_object_entry_key(entry))
        .collect::<Vec<_>>();
    for entry in split_json_object_entries(previous) {
        if let Some(key) = extract_json_object_entry_key(entry) {
            if current_ids.iter().any(|current_key| current_key == &key) {
                entries = remove_json_object_entry(&entries, &key);
            }
        }
    }
    if entries.trim().is_empty() {
        current.to_string()
    } else {
        format!("{entries},{current}")
    }
}

fn split_json_object_entries(raw: &str) -> Vec<&str> {
    split_top_level_entries(raw, '{', '}')
}

fn split_top_level_entries(raw: &str, open: char, close: char) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in raw.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            c if c == open => depth += 1,
            c if c == close => depth -= 1,
            ',' if depth == 0 => {
                let entry = raw[start..index].trim();
                if !entry.is_empty() {
                    entries.push(entry);
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = raw[start..].trim();
    if !tail.is_empty() {
        entries.push(tail);
    }
    entries
}

fn extract_json_object_entry_key(entry: &str) -> Option<String> {
    let trimmed = entry.trim();
    if !trimmed.starts_with('"') {
        return None;
    }
    let end = trimmed[1..].find('"')?;
    Some(trimmed[1..1 + end].to_string())
}

fn remove_json_object_entry(raw: &str, key: &str) -> String {
    split_json_object_entries(raw)
        .into_iter()
        .filter(|entry| extract_json_object_entry_key(entry).as_deref() != Some(key))
        .collect::<Vec<_>>()
        .join(",")
}

fn read_phase_state_raw(team_name: &str, cwd: &str) -> String {
    read_to_string(team_dir(team_name, cwd).join("phase.json")).unwrap_or_default()
}

fn mode_state_path(cwd: &str, mode: &str) -> PathBuf {
    PathBuf::from(cwd)
        .join(".omx")
        .join("state")
        .join(format!("{mode}-state.json"))
}

fn append_team_event(
    team_name: &str,
    cwd: &str,
    event_type: &str,
    worker: &str,
    reason: &str,
) -> Result<(), String> {
    let events_path = team_dir(team_name, cwd)
        .join("events")
        .join("events.ndjson");
    if let Some(parent) = events_path.parent() {
        create_dir_all(parent).map_err(|err| format!("failed creating event dir: {err}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .map_err(|err| format!("failed opening event log: {err}"))?;
    writeln!(
        file,
        "{{\"event_id\":{},\"team\":{},\"type\":{},\"worker\":{},\"reason\":{},\"created_at\":{}}}",
        json_string(&format!(
            "native-runtime-{}-{}",
            event_type,
            iso_timestamp()
        )),
        json_string(team_name),
        json_string(event_type),
        json_string(worker),
        json_string(reason),
        json_string(&iso_timestamp()),
    )
    .map_err(|err| format!("failed writing event log: {err}"))
}

fn write_shutdown_request(
    team_name: &str,
    worker_name: &str,
    requested_by: &str,
    cwd: &str,
    requested_at: &str,
) -> Result<(), String> {
    let path = team_dir(team_name, cwd)
        .join("workers")
        .join(worker_name)
        .join("shutdown-request.json");
    write(
        path,
        format!(
            "{{\"requested_at\":{},\"requested_by\":{}}}\n",
            json_string(requested_at),
            json_string(requested_by),
        ),
    )
    .map_err(|err| format!("failed writing shutdown request: {err}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShutdownAckRecord {
    status: String,
    reason: Option<String>,
    updated_at: String,
}

fn read_shutdown_ack(
    team_name: &str,
    worker_name: &str,
    cwd: &str,
    min_updated_at: Option<&str>,
) -> Option<ShutdownAckRecord> {
    let path = team_dir(team_name, cwd)
        .join("workers")
        .join(worker_name)
        .join("shutdown-ack.json");
    let raw = read_to_string(path).ok()?;
    let status = extract_json_string(&raw, "status")?;
    if status != "accept" && status != "reject" {
        return None;
    }
    let updated_at = extract_json_string(&raw, "updated_at")?;
    if let Some(min_updated_at) = min_updated_at {
        if updated_at.as_str() < min_updated_at {
            return None;
        }
    }
    Some(ShutdownAckRecord {
        status,
        reason: extract_json_string(&raw, "reason"),
        updated_at,
    })
}

fn extract_worker_names(raw: &str) -> Vec<String> {
    extract_json_array_entries_for_key(raw, "workers")
        .into_iter()
        .filter_map(|body| extract_json_string(body, "name"))
        .collect()
}

fn sync_root_team_mode_state_on_terminal_phase(
    team_name: &str,
    phase: &str,
    cwd: &str,
) -> Result<(), String> {
    if phase != "complete" && phase != "failed" && phase != "cancelled" {
        return Ok(());
    }

    let state_path = mode_state_path(cwd, "team");
    let Ok(raw) = read_to_string(&state_path) else {
        return Ok(());
    };

    let state_team_name = extract_json_string(&raw, "team_name").unwrap_or_default();
    if !state_team_name.trim().is_empty() && state_team_name != team_name {
        return Ok(());
    }

    let current_phase = extract_json_string(&raw, "current_phase").unwrap_or_default();
    let active = extract_json_bool(&raw, "active").unwrap_or(true);
    let completed_at = extract_json_string(&raw, "completed_at").unwrap_or_default();
    if !active && current_phase == phase && !completed_at.is_empty() {
        return Ok(());
    }

    let linked_ralph = extract_json_bool(&raw, "linked_ralph").unwrap_or(false);
    let completed_at = if completed_at.is_empty() {
        iso_timestamp()
    } else {
        completed_at
    };

    let mut fields = vec![
        "\"active\":false".to_string(),
        format!("\"current_phase\":{}", json_string(phase)),
        format!("\"team_name\":{}", json_string(team_name)),
        format!("\"completed_at\":{}", json_string(&completed_at)),
    ];
    if linked_ralph {
        fields.push("\"linked_ralph\":true".to_string());
    }

    write(&state_path, format!("{{{}}}\n", fields.join(",")))
        .map_err(|err| format!("failed syncing team mode state: {err}"))
}

fn sync_linked_ralph_mode_state_on_terminal_phase(
    team_name: &str,
    phase: &str,
    cwd: &str,
    now_iso: &str,
) -> Result<(), String> {
    if phase != "complete" && phase != "failed" && phase != "cancelled" {
        return Ok(());
    }

    let team_state_path = mode_state_path(cwd, "team");
    let ralph_state_path = mode_state_path(cwd, "ralph");
    let Ok(team_raw) = read_to_string(&team_state_path) else {
        return Ok(());
    };
    let Ok(ralph_raw) = read_to_string(&ralph_state_path) else {
        return Ok(());
    };

    let state_team_name = extract_json_string(&team_raw, "team_name").unwrap_or_default();
    if !state_team_name.trim().is_empty() && state_team_name != team_name {
        return Ok(());
    }
    if !extract_json_bool(&team_raw, "linked_ralph").unwrap_or(false)
        || !extract_json_bool(&ralph_raw, "linked_team").unwrap_or(false)
    {
        return Ok(());
    }

    let terminal_at = extract_json_string(&team_raw, "completed_at")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| now_iso.to_string());
    let current_phase = extract_json_string(&ralph_raw, "current_phase").unwrap_or_default();
    let active = extract_json_bool(&ralph_raw, "active").unwrap_or(true);
    let linked_terminal_phase =
        extract_json_string(&ralph_raw, "linked_team_terminal_phase").unwrap_or_default();
    let linked_terminal_at =
        extract_json_string(&ralph_raw, "linked_team_terminal_at").unwrap_or_default();
    let completed_at = extract_json_string(&ralph_raw, "completed_at").unwrap_or_default();
    if !active
        && current_phase == phase
        && linked_terminal_phase == phase
        && linked_terminal_at == terminal_at
        && completed_at == terminal_at
    {
        return Ok(());
    }

    let mut fields = vec![
        "\"active\":false".to_string(),
        format!("\"current_phase\":{}", json_string(phase)),
        "\"linked_mode\":\"team\"".to_string(),
        "\"linked_team\":true".to_string(),
        format!("\"linked_team_terminal_phase\":{}", json_string(phase)),
        format!("\"linked_team_terminal_at\":{}", json_string(&terminal_at)),
        format!("\"completed_at\":{}", json_string(&terminal_at)),
        format!("\"last_turn_at\":{}", json_string(now_iso)),
    ];
    if let Some(iteration) = extract_json_number(&ralph_raw, "iteration") {
        fields.push(format!("\"iteration\":{iteration}"));
    }
    if let Some(max_iterations) = extract_json_number(&ralph_raw, "max_iterations") {
        fields.push(format!("\"max_iterations\":{max_iterations}"));
    }
    if let Some(started_at) = extract_json_string(&ralph_raw, "started_at") {
        fields.push(format!("\"started_at\":{}", json_string(&started_at)));
    }

    write(&ralph_state_path, format!("{{{}}}\n", fields.join(",")))
        .map_err(|err| format!("failed syncing linked ralph mode state: {err}"))
}

fn extract_json_array_body(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let key_index = raw.find(&needle)?;
    let after_key = &raw[key_index + needle.len()..];
    let colon_index = after_key.find(':')?;
    let value_start = after_key[colon_index + 1..].trim_start();
    let body = slice_array_body(value_start)?;
    Some(body.to_string())
}

fn extract_json_object_body(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let key_index = raw.find(&needle)?;
    let after_key = &raw[key_index + needle.len()..];
    let colon_index = after_key.find(':')?;
    let value_start = after_key[colon_index + 1..].trim_start();
    let body = slice_object_body(value_start)?;
    Some(body.to_string())
}

fn extract_json_array_entries_for_key<'a>(raw: &'a str, key: &str) -> Vec<&'a str> {
    let needle = format!("\"{key}\"");
    let Some(key_index) = raw.find(&needle) else {
        return Vec::new();
    };
    let after_key = &raw[key_index + needle.len()..];
    let Some(colon_index) = after_key.find(':') else {
        return Vec::new();
    };
    let value_start = after_key[colon_index + 1..].trim_start();
    let Some(array_body) = slice_array_body(value_start) else {
        return Vec::new();
    };
    split_json_array_entries(array_body)
}

fn write_phase_state(
    team_name: &str,
    cwd: &str,
    phase: &str,
    _no_failed_tasks: bool,
) -> Result<(), String> {
    let phase_path = team_dir(team_name, cwd).join("phase.json");
    let previous = read_phase_state_raw(team_name, cwd);
    let previous_phase = extract_json_string(&previous, "current_phase");
    let previous_fix_attempt = extract_json_number(&previous, "current_fix_attempt").unwrap_or(0);
    let previous_max_fix_attempts = extract_json_number(&previous, "max_fix_attempts").unwrap_or(3);
    let previous_transitions =
        extract_json_array_body(&previous, "transitions").unwrap_or_default();
    let updated_at = iso_timestamp();

    let transitions = if let Some(ref prev) = previous_phase {
        append_phase_transitions(&previous_transitions, prev, phase, &updated_at)
    } else {
        String::new()
    };
    let current_fix_attempt = if phase == "team-fix" {
        previous_fix_attempt.saturating_add(1)
    } else {
        0
    };

    write(
        phase_path,
        format!(
            "{{\"current_phase\":{},\"max_fix_attempts\":{},\"current_fix_attempt\":{},\"transitions\":[{}],\"updated_at\":{}}}\n",
            json_string(phase),
            previous_max_fix_attempts,
            current_fix_attempt,
            transitions,
            json_string(&updated_at),
        ),
    )
    .map_err(|err| format!("failed writing phase state: {err}"))
}

fn append_phase_transitions(
    previous_transitions: &str,
    previous_phase: &str,
    next_phase: &str,
    updated_at: &str,
) -> String {
    let transition_path = build_phase_transition_path(previous_phase, next_phase);
    if transition_path.is_empty() {
        return previous_transitions.to_string();
    }

    let mut phases = Vec::with_capacity(transition_path.len() + 1);
    phases.push(previous_phase.to_string());
    phases.extend(transition_path);

    let mut entries = previous_transitions.trim().to_string();
    for pair in phases.windows(2) {
        let entry = format!(
            "{{\"from\":{},\"to\":{},\"at\":{}}}",
            json_string(&pair[0]),
            json_string(&pair[1]),
            json_string(updated_at),
        );
        if entries.is_empty() {
            entries = entry;
        } else {
            entries.push(',');
            entries.push_str(&entry);
        }
    }
    entries
}

fn build_phase_transition_path(previous_phase: &str, next_phase: &str) -> Vec<String> {
    if previous_phase == next_phase {
        return Vec::new();
    }

    match (previous_phase, next_phase) {
        ("team-plan", "team-verify") => vec!["team-prd", "team-exec", "team-verify"],
        ("team-prd", "team-verify") => vec!["team-exec", "team-verify"],
        ("team-exec", "team-verify") => vec!["team-verify"],
        ("team-fix", "team-verify") => vec!["team-exec", "team-verify"],
        ("team-plan", "team-exec") => vec!["team-prd", "team-exec"],
        ("team-prd", "team-exec") => vec!["team-exec"],
        ("team-fix", "team-exec") => vec!["team-exec"],
        ("team-plan", "team-fix") => vec!["team-prd", "team-exec", "team-verify", "team-fix"],
        ("team-prd", "team-fix") => vec!["team-exec", "team-verify", "team-fix"],
        ("team-exec", "team-fix") => vec!["team-verify", "team-fix"],
        ("team-verify", "team-fix") => vec!["team-fix"],
        ("team-plan", "complete") => vec!["team-prd", "team-exec", "team-verify", "complete"],
        ("team-prd", "complete") => vec!["team-exec", "team-verify", "complete"],
        ("team-exec", "complete") => vec!["team-verify", "complete"],
        ("team-verify", "complete") => vec!["complete"],
        ("team-fix", "complete") => vec!["complete"],
        ("team-plan", "failed") => vec!["team-prd", "team-exec", "team-verify", "failed"],
        ("team-prd", "failed") => vec!["team-exec", "team-verify", "failed"],
        ("team-exec", "failed") => vec!["team-verify", "failed"],
        ("team-verify", "failed") => vec!["failed"],
        ("team-fix", "failed") => vec!["failed"],
        _ => vec![next_phase],
    }
    .into_iter()
    .map(|phase| phase.to_string())
    .collect()
}

fn is_tmux_pane_alive(pane_id: &str) -> bool {
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-t",
            pane_id,
            "-F",
            "#{pane_dead} #{pane_pid}",
        ])
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let mut parts = line.split_whitespace();
    let pane_dead = parts.next().unwrap_or_default();
    let pid = parts
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    pane_dead != "1" && pid > 0 && is_pid_alive(pid)
}

fn is_pid_alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn kill_tmux_pane(pane_id: &str) -> Result<(), String> {
    let output = Command::new("tmux")
        .args(["kill-pane", "-t", pane_id])
        .output()
        .map_err(|err| format!("failed to launch tmux: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn unregister_resize_hook(hook_target: &str, hook_name: &str) -> Result<(), String> {
    let output = Command::new("tmux")
        .args(["set-hook", "-u", "-t", hook_target, hook_name])
        .output()
        .map_err(|err| format!("failed to launch tmux: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn read_live_pane_state(input: &RuntimeRunInput) -> Option<(Vec<String>, String)> {
    let config_path = PathBuf::from(&input.cwd)
        .join(".omx")
        .join("state")
        .join("team")
        .join(&input.team_name)
        .join("config.json");
    let raw = read_to_string(config_path).ok()?;
    Some((
        extract_object_string_values(&raw, "pane_id"),
        extract_json_string(&raw, "leader_pane_id").unwrap_or_default(),
    ))
}

fn read_linked_ralph_profile(team_name: &str, cwd: &str) -> bool {
    let config_path = PathBuf::from(cwd)
        .join(".omx")
        .join("state")
        .join("team")
        .join(team_name)
        .join("config.json");
    read_to_string(config_path)
        .ok()
        .and_then(|raw| extract_json_string(&raw, "lifecycle_profile"))
        .map(|profile| profile == "linked_ralph")
        .unwrap_or(false)
}

fn collect_task_results(input: &RuntimeRunInput) -> Vec<TaskResult> {
    let tasks_dir = PathBuf::from(&input.cwd)
        .join(".omx")
        .join("state")
        .join("team")
        .join(&input.team_name)
        .join("tasks");
    let Ok(entries) = read_dir(tasks_dir) else {
        return Vec::new();
    };

    let mut results = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(&OsString::from("json")) {
            continue;
        }
        let Ok(raw) = read_to_string(&path) else {
            continue;
        };
        let task_id = extract_json_string(&raw, "id")
            .or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().replace("task-", ""))
            })
            .unwrap_or_default();
        let status = extract_json_string(&raw, "status").unwrap_or_else(|| "unknown".to_string());
        let summary = extract_json_string(&raw, "result")
            .or_else(|| extract_json_string(&raw, "summary"))
            .unwrap_or_default();
        results.push(TaskResult {
            task_id,
            status,
            summary,
        });
    }
    results.sort_by(|left, right| left.task_id.cmp(&right.task_id));
    results
}

fn detect_dead_worker_failure(
    dead_worker_count: usize,
    live_worker_pane_count: usize,
    has_outstanding_work: bool,
    phase: &str,
) -> (bool, bool) {
    let all_workers_dead =
        live_worker_pane_count > 0 && dead_worker_count >= live_worker_pane_count;
    (
        all_workers_dead && has_outstanding_work,
        phase == "team-fix" && all_workers_dead,
    )
}

fn write_panes_sidecar_placeholder() -> Result<(), String> {
    let Some(job_id) = env::var("OMX_JOB_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };
    let Some(jobs_dir) = env::var("OMX_JOBS_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };

    let jobs_path = PathBuf::from(jobs_dir);
    create_dir_all(&jobs_path).map_err(|err| format!("failed creating OMX_JOBS_DIR: {err}"))?;
    let panes_path = jobs_path.join(format!("{job_id}-panes.json"));
    if panes_path.exists() {
        return Ok(());
    }
    write(&panes_path, "{\"paneIds\":[],\"leaderPaneId\":\"\"}\n")
        .map_err(|err| format!("failed writing panes sidecar: {err}"))
}

fn write_panes_sidecar(pane_ids: &[String], leader_pane_id: &str) -> Result<(), String> {
    let Some(job_id) = env::var("OMX_JOB_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };
    let Some(jobs_dir) = env::var("OMX_JOBS_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };

    let jobs_path = PathBuf::from(jobs_dir);
    create_dir_all(&jobs_path).map_err(|err| format!("failed creating OMX_JOBS_DIR: {err}"))?;
    let panes_path = jobs_path.join(format!("{job_id}-panes.json"));
    write(
        panes_path,
        format!(
            "{{\"paneIds\":{},\"leaderPaneId\":{}}}\n",
            json_string_array(pane_ids),
            json_string(leader_pane_id),
        ),
    )
    .map_err(|err| format!("failed writing panes sidecar: {err}"))
}

fn read_runtime_input_raw() -> Result<String, String> {
    let mut raw = String::new();
    io::stdin()
        .read_to_string(&mut raw)
        .map_err(|err| format!("failed reading runtime-run stdin: {err}"))?;
    Ok(raw)
}

pub fn parse_runtime_input(raw: &str) -> Result<RuntimeRunInput, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("runtime-run requires JSON stdin".to_string());
    }

    let team_name = sanitize_team_name(
        &extract_json_string(trimmed, "teamName")
            .ok_or_else(|| "runtime-run missing teamName".to_string())?,
    )?;
    let cwd =
        extract_json_string(trimmed, "cwd").ok_or_else(|| "runtime-run missing cwd".to_string())?;
    let agent_types = extract_string_array(trimmed, "agentTypes");
    if agent_types.is_empty() {
        return Err("runtime-run missing agentTypes".to_string());
    }
    let tasks = extract_tasks(trimmed);
    if tasks.is_empty() {
        return Err("runtime-run missing tasks".to_string());
    }
    let worker_count = extract_json_number(trimmed, "workerCount")
        .map(|value| value as usize)
        .unwrap_or(agent_types.len());
    let poll_interval_ms = extract_json_number(trimmed, "pollIntervalMs").unwrap_or(5_000);

    Ok(RuntimeRunInput {
        team_name,
        agent_types,
        tasks,
        cwd,
        worker_count,
        poll_interval_ms,
    })
}

fn json_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

fn json_string_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| json_string(value))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_task_results(values: &[TaskResult]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!(
                "{{\"taskId\":{},\"status\":{},\"summary\":{}}}",
                json_string(&value.task_id),
                json_string(&value.status),
                json_string(&value.summary),
            ))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn extract_json_string(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let key_index = raw.find(&needle)?;
    let after_key = &raw[key_index + needle.len()..];
    let colon_index = after_key.find(':')?;
    let value_start = after_key[colon_index + 1..].trim_start();
    if value_start.starts_with("null") {
        return None;
    }
    if !value_start.starts_with('"') {
        return None;
    }

    let mut escaped = false;
    let mut value = String::new();
    for ch in value_start[1..].chars() {
        if escaped {
            value.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(value),
            other => value.push(other),
        }
    }
    None
}

fn extract_json_number(raw: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"");
    let key_index = raw.find(&needle)?;
    let after_key = &raw[key_index + needle.len()..];
    let colon_index = after_key.find(':')?;
    let value_start = after_key[colon_index + 1..].trim_start();
    let digits: String = value_start
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

fn extract_json_bool(raw: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let key_index = raw.find(&needle)?;
    let after_key = &raw[key_index + needle.len()..];
    let colon_index = after_key.find(':')?;
    let value_start = after_key[colon_index + 1..].trim_start();
    if value_start.starts_with("true") {
        Some(true)
    } else if value_start.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_string_array(raw: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let Some(key_index) = raw.find(&needle) else {
        return Vec::new();
    };
    let after_key = &raw[key_index + needle.len()..];
    let Some(colon_index) = after_key.find(':') else {
        return Vec::new();
    };
    let value_start = after_key[colon_index + 1..].trim_start();
    let Some(array_body) = slice_array_body(value_start) else {
        return Vec::new();
    };

    split_json_array_entries(array_body)
        .into_iter()
        .filter_map(|entry| {
            let trimmed = entry.trim();
            if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
                Some(trimmed[1..trimmed.len() - 1].to_string())
            } else {
                None
            }
        })
        .collect()
}

fn extract_object_string_values(raw: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let mut values = Vec::new();
    let mut slice = raw;
    while let Some(index) = slice.find(&needle) {
        let after = &slice[index..];
        if let Some(value) = extract_json_string(after, key) {
            if !value.trim().is_empty() {
                values.push(value);
            }
        }
        slice = &after[needle.len()..];
    }
    values
}

fn extract_tasks(raw: &str) -> Vec<RuntimeTaskInput> {
    let needle = "\"tasks\"";
    let Some(key_index) = raw.find(needle) else {
        return Vec::new();
    };
    let after_key = &raw[key_index + needle.len()..];
    let Some(colon_index) = after_key.find(':') else {
        return Vec::new();
    };
    let value_start = after_key[colon_index + 1..].trim_start();
    let Some(array_body) = slice_array_body(value_start) else {
        return Vec::new();
    };

    let mut tasks = Vec::new();
    for object in split_json_array_entries(array_body) {
        let normalized = object.trim().trim_start_matches('{').trim_end_matches('}');
        let subject = extract_json_string(normalized, "subject");
        let description = extract_json_string(normalized, "description");
        if let (Some(subject), Some(description)) = (subject, description) {
            tasks.push(RuntimeTaskInput {
                subject,
                description,
            });
        }
    }
    tasks
}

fn split_json_array_entries(body: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut start = 0_usize;

    for (index, ch) in body.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            ',' if depth == 0 => {
                entries.push(body[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }

    if start < body.len() {
        entries.push(body[start..].trim());
    }
    entries
        .into_iter()
        .filter(|entry| !entry.is_empty())
        .collect()
}

fn slice_array_body(value_start: &str) -> Option<&str> {
    if !value_start.starts_with('[') {
        return None;
    }

    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in value_start.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&value_start[1..index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn slice_object_body(value_start: &str) -> Option<&str> {
    if !value_start.starts_with('{') {
        return None;
    }

    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in value_start.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&value_start[1..index]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        collect_task_results, detect_dead_worker_failure, extract_json_bool, extract_string_array,
        has_structured_verification_evidence, monitor_team, parse_runtime_input,
        read_linked_ralph_profile, shutdown_team, split_json_array_entries,
        write_panes_sidecar_placeholder, write_phase_state, RuntimeRunInput, RuntimeTaskInput,
    };
    use crate::test_support::env_lock;
    use std::env;
    use std::fs::{create_dir_all, read_to_string, write};
    use std::path::PathBuf;

    #[test]
    fn parses_minimal_runtime_run_input() {
        let parsed = parse_runtime_input(
            r#"{"teamName":"alpha","agentTypes":["codex"],"tasks":[{"subject":"one","description":"desc"}],"cwd":"/tmp/repo"}"#,
        )
        .expect("expected runtime-run input to parse");

        assert_eq!(parsed.team_name, "alpha");
        assert_eq!(parsed.agent_types, vec!["codex".to_string()]);
        assert_eq!(parsed.tasks.len(), 1);
        assert_eq!(parsed.cwd, "/tmp/repo");
        assert_eq!(parsed.worker_count, 1);
        assert_eq!(parsed.poll_interval_ms, 5_000);
    }

    #[test]
    fn rejects_missing_team_name() {
        let error = parse_runtime_input(
            r#"{"agentTypes":["codex"],"tasks":[{"subject":"one","description":"desc"}],"cwd":"/tmp/repo"}"#,
        )
        .expect_err("expected missing teamName error");
        assert!(error.contains("teamName"));
    }

    #[test]
    fn supports_commas_inside_task_descriptions() {
        let parsed = parse_runtime_input(
            r#"{"teamName":"alpha","agentTypes":["codex"],"tasks":[{"subject":"one","description":"desc, with comma"},{"subject":"two","description":"next"}],"cwd":"/tmp/repo"}"#,
        )
        .expect("expected parse");
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.tasks[0].description, "desc, with comma");
    }

    #[test]
    fn split_json_array_entries_handles_nested_objects() {
        let entries = split_json_array_entries(
            r#"{"subject":"one","description":"a,b"},{"subject":"two","description":"c"}"#,
        );
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("a,b"));
    }

    #[test]
    fn detects_dead_worker_failure_from_live_pane_count() {
        let stale_snapshot_behavior = detect_dead_worker_failure(2, 3, true, "team-exec");
        assert!(!stale_snapshot_behavior.0);

        let live_behavior = detect_dead_worker_failure(2, 2, true, "team-exec");
        assert!(live_behavior.0);
        assert!(!live_behavior.1);
    }

    #[test]
    fn reads_linked_ralph_profile_from_team_config() {
        let temp =
            std::env::temp_dir().join(format!("omx-runtime-run-linked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let config_dir = temp.join(".omx").join("state").join("team").join("alpha");
        create_dir_all(&config_dir).expect("expected config dir");
        write(
            config_dir.join("config.json"),
            r#"{"lifecycle_profile":"linked_ralph"}"#,
        )
        .expect("expected config");

        assert!(read_linked_ralph_profile(
            "alpha",
            temp.to_string_lossy().as_ref()
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn collects_task_results_from_state_files() {
        let temp =
            std::env::temp_dir().join(format!("omx-runtime-run-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let tasks_dir = temp
            .join(".omx")
            .join("state")
            .join("team")
            .join("alpha")
            .join("tasks");
        create_dir_all(&tasks_dir).expect("expected task dir");
        write(
            tasks_dir.join("task-1.json"),
            r#"{"id":"1","status":"completed","result":"done"}"#,
        )
        .expect("expected task file");

        let results = collect_task_results(&RuntimeRunInput {
            team_name: "alpha".into(),
            agent_types: vec!["codex".into()],
            tasks: vec![RuntimeTaskInput {
                subject: "one".into(),
                description: "desc".into(),
            }],
            cwd: temp.to_string_lossy().into_owned(),
            worker_count: 1,
            poll_interval_ms: 100,
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, "1");
        assert_eq!(results[0].summary, "done");
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn extracts_string_arrays_with_multiple_entries() {
        let values = extract_string_array(r#"{"paneIds":["%1","%2"]}"#, "paneIds");
        assert_eq!(values, vec!["%1".to_string(), "%2".to_string()]);
    }

    #[test]
    fn writes_panes_sidecar_placeholder_when_job_env_is_present() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = std::env::temp_dir().join(format!("omx-runtime-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        unsafe { env::set_var("OMX_JOB_ID", "job-1") };
        unsafe { env::set_var("OMX_JOBS_DIR", &temp) };

        write_panes_sidecar_placeholder().expect("expected panes sidecar write");

        let content = read_to_string(PathBuf::from(&temp).join("job-1-panes.json"))
            .expect("expected panes file content");
        assert!(content.contains("\"paneIds\":[]"));
        assert!(content.contains("\"leaderPaneId\":\"\""));

        unsafe { env::remove_var("OMX_JOB_ID") };
        unsafe { env::remove_var("OMX_JOBS_DIR") };
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn verification_evidence_heuristic_matches_ts_gate_shape() {
        assert!(has_structured_verification_evidence(
            "Verification:
- PASS tests
- Build: `cargo test`"
        ));
        assert!(!has_structured_verification_evidence(
            "Implemented fix only"
        ));
    }

    #[test]
    fn extract_json_bool_reads_true_and_false_flags() {
        assert_eq!(
            extract_json_bool(r#"{"requires_code_change":true}"#, "requires_code_change"),
            Some(true)
        );
        assert_eq!(
            extract_json_bool(r#"{"requires_code_change":false}"#, "requires_code_change"),
            Some(false)
        );
    }

    #[test]
    fn monitor_team_holds_terminal_success_in_team_verify_without_verification_evidence() {
        let temp =
            std::env::temp_dir().join(format!("omx-runtime-run-monitor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        let tasks_dir = team_dir.join("tasks");
        create_dir_all(&tasks_dir).expect("task dir");
        write(
            tasks_dir.join("task-1.json"),
            r#"{"id":"1","status":"completed","requires_code_change":true,"result":"done"}"#,
        )
        .expect("task state");

        let first = monitor_team(&RuntimeRunInput {
            team_name: "alpha".into(),
            agent_types: vec!["codex".into()],
            tasks: vec![RuntimeTaskInput {
                subject: "one".into(),
                description: "desc".into(),
            }],
            cwd: temp.to_string_lossy().into_owned(),
            worker_count: 1,
            poll_interval_ms: 10,
        })
        .expect("monitor ok")
        .expect("snapshot");
        assert_eq!(first.phase, "team-verify");

        write(
            tasks_dir.join("task-1.json"),
            r#"{"id":"1","status":"completed","requires_code_change":true,"result":"Summary: done\nVerification:\n- PASS build: `cargo build`\n- PASS tests: `cargo test`"}"#,
        )
        .expect("task state with verification");

        let second = monitor_team(&RuntimeRunInput {
            team_name: "alpha".into(),
            agent_types: vec!["codex".into()],
            tasks: vec![RuntimeTaskInput {
                subject: "one".into(),
                description: "desc".into(),
            }],
            cwd: temp.to_string_lossy().into_owned(),
            worker_count: 1,
            poll_interval_ms: 10,
        })
        .expect("monitor ok")
        .expect("snapshot");
        assert_eq!(second.phase, "complete");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn monitor_snapshot_preserves_previous_notified_and_completed_maps() {
        let temp = std::env::temp_dir().join(format!(
            "omx-runtime-monitor-preserve-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        let tasks_dir = team_dir.join("tasks");
        let worker_dir = team_dir.join("workers").join("worker-1");
        create_dir_all(&tasks_dir).expect("expected task dir");
        create_dir_all(&worker_dir).expect("expected worker dir");

        write(
            tasks_dir.join("task-1.json"),
            r#"{"id":"1","status":"in_progress"}"#,
        )
        .expect("task");
        write(
            worker_dir.join("identity.json"),
            format!(r#"{{"pid":{}}}"#, std::process::id()),
        )
        .expect("identity");
        write(
            worker_dir.join("status.json"),
            r#"{"state":"working","current_task_id":"1"}"#,
        )
        .expect("status");
        write(worker_dir.join("heartbeat.json"), r#"{"turn_count":2}"#).expect("heartbeat");
        write(
            team_dir.join("monitor-snapshot.json"),
            r#"{"taskStatusById":{"1":"completed"},"workerAliveByName":{"worker-1":true},"workerStateByName":{"worker-1":"idle"},"workerTurnCountByName":{"worker-1":1},"workerTaskIdByName":{"worker-1":"1"},"mailboxNotifiedByMessageId":{"msg-1":"2026-03-14T00:00:00Z"},"completedEventTaskIds":{"1":true},"monitorTimings":{"list_tasks_ms":1,"worker_scan_ms":1,"mailbox_delivery_ms":1,"total_ms":1,"updated_at":"2026-03-14T00:00:00Z"}}"#,
        ).expect("seed snapshot");

        let _snapshot = monitor_team(&RuntimeRunInput {
            team_name: "alpha".into(),
            agent_types: vec!["codex".into()],
            tasks: vec![RuntimeTaskInput {
                subject: "one".into(),
                description: "desc".into(),
            }],
            cwd: temp.to_string_lossy().into_owned(),
            worker_count: 1,
            poll_interval_ms: 10,
        })
        .expect("monitor ok")
        .expect("snapshot");

        let monitor_snapshot =
            read_to_string(team_dir.join("monitor-snapshot.json")).expect("snapshot reread");
        assert!(monitor_snapshot
            .contains("\"mailboxNotifiedByMessageId\":{\"msg-1\":\"2026-03-14T00:00:00Z\"}"));
        assert!(monitor_snapshot.contains("\"completedEventTaskIds\":{\"1\":true}"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn write_phase_state_preserves_and_appends_transitions() {
        let temp =
            std::env::temp_dir().join(format!("omx-runtime-phase-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        create_dir_all(&team_dir).expect("team dir");
        write(
            team_dir.join("phase.json"),
            r#"{"current_phase":"team-exec","max_fix_attempts":3,"current_fix_attempt":0,"transitions":[{"from":"team-plan","to":"team-exec","at":"2026-03-14T00:00:00Z"}],"updated_at":"2026-03-14T00:00:00Z"}"#,
        )
        .expect("phase file");

        write_phase_state(
            "alpha",
            temp.to_string_lossy().as_ref(),
            "team-verify",
            true,
        )
        .expect("write phase");
        let phase = read_to_string(team_dir.join("phase.json")).expect("phase reread");
        assert!(phase.contains("\"current_phase\":\"team-verify\""));
        assert!(phase.contains("\"from\":\"team-plan\",\"to\":\"team-exec\""));
        assert!(phase.contains("\"from\":\"team-exec\",\"to\":\"team-verify\""));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn monitor_team_writes_monitor_snapshot_with_worker_state() {
        let temp = std::env::temp_dir().join(format!(
            "omx-runtime-monitor-snapshot-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        let tasks_dir = team_dir.join("tasks");
        let worker_dir = team_dir.join("workers").join("worker-1");
        create_dir_all(&tasks_dir).expect("expected task dir");
        create_dir_all(&worker_dir).expect("expected worker dir");

        write(
            tasks_dir.join("task-1.json"),
            r#"{"id":"1","status":"in_progress"}"#,
        )
        .expect("expected task file");
        write(
            worker_dir.join("identity.json"),
            format!(r#"{{"pid":{}}}"#, std::process::id()),
        )
        .expect("expected identity file");
        write(
            worker_dir.join("status.json"),
            r#"{"state":"working","current_task_id":"1","updated_at":"2026-03-14T00:00:00Z"}"#,
        )
        .expect("expected status file");
        write(
            worker_dir.join("heartbeat.json"),
            format!(
                r#"{{"pid":{},"last_turn_at":"2026-03-14T00:00:00Z","turn_count":7,"alive":true}}"#,
                std::process::id()
            ),
        )
        .expect("expected heartbeat file");

        let _snapshot = monitor_team(&RuntimeRunInput {
            team_name: "alpha".into(),
            agent_types: vec!["codex".into()],
            tasks: vec![RuntimeTaskInput {
                subject: "one".into(),
                description: "desc".into(),
            }],
            cwd: temp.to_string_lossy().into_owned(),
            worker_count: 1,
            poll_interval_ms: 10,
        })
        .expect("expected monitor ok")
        .expect("expected snapshot");

        let monitor_snapshot = read_to_string(team_dir.join("monitor-snapshot.json"))
            .expect("expected monitor snapshot file");
        assert!(monitor_snapshot.contains("\"taskStatusById\":{\"1\":\"in_progress\"}"));
        assert!(monitor_snapshot.contains("\"workerAliveByName\":{\"worker-1\":"));
        assert!(monitor_snapshot.contains("\"workerStateByName\":{\"worker-1\":\"working\"}"));
        assert!(monitor_snapshot.contains("\"workerTurnCountByName\":{\"worker-1\":7}"));
        assert!(monitor_snapshot.contains("\"workerTaskIdByName\":{\"worker-1\":\"1\"}"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn monitor_team_syncs_root_and_linked_ralph_terminal_state() {
        let temp = std::env::temp_dir().join(format!(
            "omx-runtime-linked-ralph-monitor-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        let tasks_dir = team_dir.join("tasks");
        create_dir_all(&tasks_dir).expect("expected task dir");
        write(
            tasks_dir.join("task-1.json"),
            r#"{"id":"1","status":"completed","requires_code_change":false}"#,
        )
        .expect("expected task file");
        write(
            temp.join(".omx").join("state").join("team-state.json"),
            r#"{"active":true,"current_phase":"team-exec","linked_ralph":true,"team_name":"alpha"}"#,
        )
        .expect("expected team state");
        write(
            temp.join(".omx").join("state").join("ralph-state.json"),
            r#"{"active":true,"iteration":1,"max_iterations":10,"current_phase":"executing","started_at":"2026-03-11T00:00:00.000Z","linked_team":true}"#,
        )
        .expect("expected ralph state");

        let snapshot = monitor_team(&RuntimeRunInput {
            team_name: "alpha".into(),
            agent_types: vec!["codex".into()],
            tasks: vec![RuntimeTaskInput {
                subject: "one".into(),
                description: "desc".into(),
            }],
            cwd: temp.to_string_lossy().into_owned(),
            worker_count: 1,
            poll_interval_ms: 10,
        })
        .expect("expected monitor ok")
        .expect("expected snapshot");
        assert_eq!(snapshot.phase, "complete");

        let team_state = read_to_string(temp.join(".omx").join("state").join("team-state.json"))
            .expect("expected reread team state");
        assert!(team_state.contains("\"active\":false"));
        assert!(team_state.contains("\"current_phase\":\"complete\""));
        assert!(team_state.contains("\"team_name\":\"alpha\""));
        assert!(team_state.contains("\"linked_ralph\":true"));
        assert!(team_state.contains("\"completed_at\":\""));

        let ralph_state = read_to_string(temp.join(".omx").join("state").join("ralph-state.json"))
            .expect("expected reread ralph state");
        assert!(ralph_state.contains("\"active\":false"));
        assert!(ralph_state.contains("\"current_phase\":\"complete\""));
        assert!(ralph_state.contains("\"linked_team\":true"));
        assert!(ralph_state.contains("\"linked_mode\":\"team\""));
        assert!(ralph_state.contains("\"linked_team_terminal_phase\":\"complete\""));
        assert!(ralph_state.contains("\"linked_team_terminal_at\":\""));
        assert!(ralph_state.contains("\"completed_at\":\""));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn shutdown_team_with_ralph_syncs_linked_ralph_cancellation_before_cleanup() {
        let temp = std::env::temp_dir().join(format!(
            "omx-runtime-linked-ralph-shutdown-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        let tasks_dir = team_dir.join("tasks");
        create_dir_all(&tasks_dir).expect("expected task dir");
        write(
            tasks_dir.join("task-1.json"),
            r#"{"id":"1","status":"completed"}"#,
        )
        .expect("expected task file");
        write(
            team_dir.join("config.json"),
            r#"{"name":"alpha","worker_launch_mode":"interactive","lifecycle_profile":"linked_ralph","workers":[],"tmux_session":"omx-team-alpha","leader_pane_id":null,"hud_pane_id":null,"resize_hook_name":null,"resize_hook_target":null}"#,
        )
        .expect("expected team config");
        write(
            temp.join(".omx").join("state").join("team-state.json"),
            r#"{"active":true,"current_phase":"team-exec","linked_ralph":true,"team_name":"alpha"}"#,
        )
        .expect("expected team state");
        write(
            temp.join(".omx").join("state").join("ralph-state.json"),
            r#"{"active":true,"iteration":1,"max_iterations":10,"current_phase":"executing","started_at":"2026-03-11T00:00:00.000Z","linked_team":true}"#,
        )
        .expect("expected ralph state");

        shutdown_team("alpha", temp.to_string_lossy().as_ref(), false, true)
            .expect("expected shutdown ok");

        assert!(!team_dir.exists());
        let ralph_state = read_to_string(temp.join(".omx").join("state").join("ralph-state.json"))
            .expect("expected reread ralph state");
        assert!(ralph_state.contains("\"active\":false"));
        assert!(ralph_state.contains("\"current_phase\":\"cancelled\""));
        assert!(ralph_state.contains("\"linked_team_terminal_phase\":\"cancelled\""));
        assert!(ralph_state.contains("\"linked_team_terminal_at\":\""));
        assert!(ralph_state.contains("\"completed_at\":\""));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn shutdown_team_emits_shutdown_ack_event_for_rejection() {
        let temp = std::env::temp_dir().join(format!(
            "omx-runtime-shutdown-ack-event-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        let worker_dir = team_dir.join("workers").join("worker-1");
        create_dir_all(&worker_dir).expect("expected worker dir");
        write(
            team_dir.join("config.json"),
            r#"{"name":"alpha","worker_launch_mode":"interactive","workers":[{"name":"worker-1","pane_id":"%999"}],"tmux_session":"omx-team-alpha","leader_pane_id":null,"hud_pane_id":null,"resize_hook_name":null,"resize_hook_target":null}"#,
        )
        .expect("expected team config");
        write(
            worker_dir.join("shutdown-ack.json"),
            r#"{"status":"reject","reason":"busy","updated_at":"9999-01-01T00:00:00.000Z"}"#,
        )
        .expect("expected shutdown ack");

        let error = shutdown_team("alpha", temp.to_string_lossy().as_ref(), false, false)
            .expect_err("expected shutdown reject");
        assert!(error.contains("shutdown_rejected:worker-1:busy"));

        let events = read_to_string(team_dir.join("events").join("events.ndjson"))
            .expect("expected event log");
        assert!(events.contains("\"type\":\"shutdown_ack\""));
        assert!(events.contains("\"worker\":\"worker-1\""));
        assert!(events.contains("\"reason\":\"reject:busy\""));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn shutdown_team_writes_shutdown_request_before_processing_ack() {
        let temp = std::env::temp_dir().join(format!(
            "omx-runtime-shutdown-request-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let team_dir = temp.join(".omx").join("state").join("team").join("alpha");
        let worker_dir = team_dir.join("workers").join("worker-1");
        create_dir_all(&worker_dir).expect("expected worker dir");
        write(
            team_dir.join("config.json"),
            r#"{"name":"alpha","worker_launch_mode":"interactive","workers":[{"name":"worker-1","pane_id":"%999"}],"tmux_session":"omx-team-alpha","leader_pane_id":null,"hud_pane_id":null,"resize_hook_name":null,"resize_hook_target":null}"#,
        )
        .expect("expected team config");
        write(
            worker_dir.join("shutdown-ack.json"),
            r#"{"status":"reject","reason":"busy","updated_at":"9999-01-01T00:00:00.000Z"}"#,
        )
        .expect("expected shutdown ack");

        let _ = shutdown_team("alpha", temp.to_string_lossy().as_ref(), false, false);

        let request = read_to_string(worker_dir.join("shutdown-request.json"))
            .expect("expected shutdown request");
        assert!(request.contains("\"requested_by\":\"leader-fixed\""));
        assert!(
            request.contains("\"requested_at\":\""),
            "expected shutdown request timestamp"
        );

        let _ = std::fs::remove_dir_all(&temp);
    }
}
