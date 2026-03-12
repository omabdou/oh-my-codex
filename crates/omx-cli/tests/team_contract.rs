use omx_cli::session_state::extract_json_string_field;
use omx_cli::team::run_team;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn expected_team_api_operations() -> &'static [&'static str] {
    &[
        "send-message",
        "broadcast",
        "mailbox-list",
        "mailbox-mark-delivered",
        "mailbox-mark-notified",
        "create-task",
        "read-task",
        "list-tasks",
        "update-task",
        "claim-task",
        "transition-task-status",
        "release-task-claim",
        "read-config",
        "read-manifest",
        "read-worker-status",
        "read-worker-heartbeat",
        "update-worker-heartbeat",
        "write-worker-inbox",
        "write-worker-identity",
        "append-event",
        "read-events",
        "await-event",
        "read-idle-state",
        "read-stall-state",
        "get-summary",
        "cleanup",
        "write-shutdown-request",
        "read-shutdown-ack",
        "read-monitor-snapshot",
        "write-monitor-snapshot",
        "read-task-approval",
        "write-task-approval",
    ]
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("omx-team-contract-{label}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[cfg(unix)]
fn write_worker_stub(cwd: &Path, name: &str) -> std::path::PathBuf {
    let path = cwd.join(name);
    std::fs::write(&path, "#!/bin/sh\nsleep 30\n").expect("write worker stub");
    let mut permissions = std::fs::metadata(&path)
        .expect("stub metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("set worker stub permissions");
    path
}

fn write_team_fixture(cwd: &Path, team_name: &str) {
    let team_root = cwd.join(".omx/state/team").join(team_name);
    std::fs::create_dir_all(team_root.join("tasks")).expect("create tasks dir");
    std::fs::create_dir_all(team_root.join("mailbox")).expect("create mailbox dir");
    std::fs::create_dir_all(team_root.join("workers/worker-1")).expect("create worker dir");
    std::fs::create_dir_all(team_root.join("events")).expect("create events dir");

    std::fs::write(
        team_root.join("config.json"),
        r#"{
  "name": "fixture-team",
  "runtime_session_id": "omx-team-fixture-team",
  "tmux_session": "omx-team-fixture-team",
  "leader_pane_id": "%1",
  "workers": [
    { "name": "worker-1", "pane_id": "%2" }
  ]
}
"#,
    )
    .expect("write config");

    std::fs::write(
        team_root.join("manifest.v2.json"),
        r#"{
  "name": "fixture-team",
  "runtime_session_id": "omx-team-fixture-team",
  "workers": [
    { "name": "worker-1" }
  ]
}
"#,
    )
    .expect("write manifest");

    std::fs::write(
        team_root.join("tasks/task-1.json"),
        r#"{
  "id": "1",
  "status": "pending",
  "subject": "fixture task"
}
"#,
    )
    .expect("write task");

    std::fs::write(
        team_root.join("mailbox/worker-1.json"),
        r#"{
  "worker": "worker-1",
  "messages": [
    {
      "message_id": "msg-1",
      "body": "hello",
      "from_worker": "leader-fixed",
      "to_worker": "worker-1"
    }
  ]
}
"#,
    )
    .expect("write mailbox");

    std::fs::write(
        team_root.join("events/events.ndjson"),
        concat!(
            "{\"event_id\":\"evt-1\",\"type\":\"worker_idle\",\"source_type\":\"worker_idle\",\"worker\":\"worker-1\",\"task_id\":\"1\",\"prev_state\":\"working\"}\n",
            "{\"event_id\":\"evt-2\",\"type\":\"task_completed\",\"worker\":\"worker-1\",\"task_id\":\"1\",\"state\":\"completed\"}\n"
        ),
    )
    .expect("write events");
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

fn write_mutation_fixture(cwd: &Path, team_name: &str) {
    let team_root = cwd.join(".omx/state/team").join(team_name);
    std::fs::create_dir_all(team_root.join("tasks")).expect("create tasks dir");
    std::fs::create_dir_all(team_root.join("mailbox")).expect("create mailbox dir");
    std::fs::create_dir_all(team_root.join("dispatch")).expect("create dispatch dir");
    std::fs::create_dir_all(team_root.join("approvals")).expect("create approvals dir");
    std::fs::create_dir_all(team_root.join("workers/worker-1")).expect("create worker-1 dir");
    std::fs::create_dir_all(team_root.join("workers/worker-2")).expect("create worker-2 dir");
    std::fs::create_dir_all(team_root.join("events")).expect("create events dir");

    std::fs::write(
        team_root.join("config.json"),
        r#"{
  "name": "fixture-team",
  "task": "fixture work",
  "agent_type": "executor",
  "worker_count": 2,
  "runtime_session_id": "omx-team-fixture-team",
  "tmux_session": "omx-team-fixture-team",
  "leader_pane_id": "%1",
  "next_task_id": 2,
  "workers": [
    { "name": "worker-1", "index": 1, "role": "executor", "pane_id": "%2" },
    { "name": "worker-2", "index": 2, "role": "executor", "pane_id": "%3" }
  ]
}
"#,
    )
    .expect("write config");

    std::fs::write(
        team_root.join("manifest.v2.json"),
        r#"{
  "name": "fixture-team",
  "runtime_session_id": "omx-team-fixture-team",
  "worker_count": 2,
  "next_task_id": 2,
  "workers": [
    { "name": "worker-1" },
    { "name": "worker-2" }
  ]
}
"#,
    )
    .expect("write manifest");

    std::fs::write(
        team_root.join("phase.json"),
        r#"{
  "current_phase": "team-exec"
}
"#,
    )
    .expect("write phase");

    std::fs::write(
        team_root.join("monitor-snapshot.json"),
        r#"{
  "taskStatusById": { "1": "pending" },
  "workerAliveByName": { "worker-1": true, "worker-2": true },
  "workerStateByName": { "worker-1": "working", "worker-2": "idle" },
  "workerTurnCountByName": { "worker-1": 4, "worker-2": 2 },
  "workerTaskIdByName": { "worker-1": "1" },
  "mailboxNotifiedByMessageId": {},
  "completedEventTaskIds": {}
}
"#,
    )
    .expect("write snapshot");

    std::fs::write(
        team_root.join("tasks/task-1.json"),
        r#"{
  "id": "1",
  "subject": "fixture task",
  "description": "existing task",
  "status": "pending",
  "version": 2,
  "created_at": "2026-03-11T00:00:00.000Z"
}
"#,
    )
    .expect("write task");

    std::fs::write(
        team_root.join("mailbox/worker-1.json"),
        r#"{
  "worker": "worker-1",
  "messages": [
    {
      "message_id": "msg-1",
      "body": "hello",
      "from_worker": "leader-fixed",
      "to_worker": "worker-1",
      "created_at": "2026-03-11T00:00:00.000Z"
    }
  ]
}
"#,
    )
    .expect("write mailbox");

    std::fs::write(
        team_root.join("dispatch/requests.json"),
        r#"[
  {
    "request_id": "req-1",
    "kind": "mailbox",
    "team_name": "fixture-team",
    "to_worker": "worker-1",
    "message_id": "msg-1",
    "trigger_message": "notify worker-1",
    "status": "pending",
    "attempt_count": 0,
    "created_at": "2026-03-11T00:00:00.000Z",
    "updated_at": "2026-03-11T00:00:00.000Z"
  }
]
"#,
    )
    .expect("write dispatch");

    std::fs::write(
        team_root.join("workers/worker-1/shutdown-ack.json"),
        r#"{
  "status": "accept",
  "updated_at": "2026-03-11T01:00:00.000Z"
}
"#,
    )
    .expect("write shutdown ack");

    std::fs::write(
        team_root.join("events/events.ndjson"),
        "{\"event_id\":\"evt-1\",\"type\":\"worker_state_changed\",\"worker\":\"worker-1\",\"state\":\"working\",\"created_at\":\"2026-03-11T00:00:00.000Z\"}\n",
    )
    .expect("write events");
}

fn write_prompt_fixture(cwd: &Path, team_name: &str) {
    let team_root = cwd.join(".omx/state/team").join(team_name);
    std::fs::create_dir_all(team_root.join("tasks")).expect("create tasks dir");
    std::fs::create_dir_all(team_root.join("workers/worker-1")).expect("create worker dir");
    std::fs::create_dir_all(team_root.join("events")).expect("create events dir");

    std::fs::write(
        team_root.join("config.json"),
        r#"{
  "name": "prompt-team",
  "task": "prompt fixture work",
  "agent_type": "executor",
  "worker_launch_mode": "prompt",
  "worker_count": 1,
  "runtime_session_id": "prompt-prompt-team",
  "tmux_session": null,
  "next_task_id": 1,
  "workers": [
    { "name": "worker-1", "index": 1, "role": "executor" }
  ]
}
"#,
    )
    .expect("write prompt config");

    std::fs::write(
        team_root.join("manifest.v2.json"),
        r#"{
  "schema_version": 2,
  "name": "prompt-team",
  "task": "prompt fixture work",
  "leader": { "session_id": "sess-1", "worker_id": "leader-fixed", "role": "leader" },
  "policy": {
    "display_mode": "auto",
    "worker_launch_mode": "prompt",
    "dispatch_mode": "hook_preferred_with_fallback",
    "dispatch_ack_timeout_ms": 3000,
    "delegation_only": false,
    "plan_approval_required": false,
    "nested_teams_allowed": false,
    "one_team_per_leader_session": true,
    "cleanup_requires_all_workers_inactive": true
  },
  "permissions_snapshot": { "approval_mode": "never", "sandbox_mode": "danger-full-access", "network_access": true },
  "runtime_session_id": "prompt-prompt-team",
  "tmux_session": null,
  "worker_count": 1,
  "workers": [
    { "name": "worker-1", "index": 1, "role": "executor", "assigned_tasks": [] }
  ],
  "next_task_id": 1,
  "created_at": "2026-03-11T00:00:00.000Z",
  "leader_pane_id": null,
  "hud_pane_id": null,
  "resize_hook_name": null,
  "resize_hook_target": null
}
"#,
    )
    .expect("write prompt manifest");
}

fn extract_claim_token(stdout: &str) -> String {
    let marker = "\"claimToken\":\"";
    let start = stdout.find(marker).expect("claim token marker") + marker.len();
    let end = stdout[start..].find('"').expect("claim token end") + start;
    stdout[start..end].to_string()
}

#[test]
fn team_help_mentions_api_and_await_contracts() {
    let result =
        run_team(&["--help".to_string()], Path::new("."), &BTreeMap::new()).expect("team help");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("omx team api <operation>"));
    assert!(stdout.contains("omx team await <team-name>"));
}

#[test]
fn team_api_help_lists_full_operation_contract() {
    let result = run_team(
        &["api".to_string(), "--help".to_string()],
        Path::new("."),
        &BTreeMap::new(),
    )
    .expect("team api help");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    for operation in expected_team_api_operations() {
        assert!(stdout.contains(operation), "missing operation {operation}");
    }
}

#[test]
fn team_api_help_alias_lists_full_operation_contract() {
    let result = run_team(
        &["api".to_string(), "help".to_string()],
        Path::new("."),
        &BTreeMap::new(),
    )
    .expect("team api help alias");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    for operation in expected_team_api_operations() {
        assert!(stdout.contains(operation), "missing operation {operation}");
    }
}

#[test]
fn team_status_summary_matches_state_snapshot_contract() {
    let cwd = temp_dir("status");
    let team_root = cwd.join(".omx/state/team/fixture-team");
    std::fs::create_dir_all(team_root.join("tasks")).expect("create task dir");
    std::fs::write(
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
    std::fs::write(
        team_root.join("phase.json"),
        r#"{
  "current_phase": "team-exec"
}
"#,
    )
    .expect("write phase");
    std::fs::write(
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
    std::fs::write(
        team_root.join("tasks/task-1.json"),
        "{\"status\":\"pending\"}\n",
    )
    .expect("task 1");
    std::fs::write(
        team_root.join("tasks/task-2.json"),
        "{\"status\":\"in_progress\"}\n",
    )
    .expect("task 2");
    std::fs::write(
        team_root.join("tasks/task-3.json"),
        "{\"status\":\"completed\"}\n",
    )
    .expect("task 3");
    std::fs::write(
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
    let stdout = String::from_utf8(result.stdout).expect("utf8");
    assert_eq!(
        stdout,
        concat!(
            "team=fixture-team phase=team-exec\n",
            "runtime target: unknown\n",
            "layout: display=auto launch=prompt spawn=process reflow=state-backed hud=state-backed\n",
            "tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true\n",
            "workers: total=3 dead=1 non_reporting=1\n",
            "tasks: total=4 pending=1 blocked=0 in_progress=1 completed=1 failed=1\n",
        )
    );

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn operation_specific_help_is_available_for_send_message() {
    let result = run_team(
        &[
            "api".to_string(),
            "send-message".to_string(),
            "--help".to_string(),
        ],
        Path::new("."),
        &BTreeMap::new(),
    )
    .expect("team api operation help should render");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("Usage: omx team api send-message --input <json> [--json]"));
    assert!(stdout.contains("from_worker"));
    assert!(stdout.contains("to_worker"));
    assert!(stdout.contains("body"));
}

#[test]
fn operation_specific_help_alias_is_available_for_send_message() {
    let result = run_team(
        &[
            "api".to_string(),
            "send-message".to_string(),
            "help".to_string(),
        ],
        Path::new("."),
        &BTreeMap::new(),
    )
    .expect("team api operation help alias should render");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("Usage: omx team api send-message --input <json> [--json]"));
    assert!(stdout.contains("from_worker"));
    assert!(stdout.contains("to_worker"));
    assert!(stdout.contains("body"));
}

#[test]
fn read_only_team_api_json_contracts_are_stable() {
    let cwd = temp_dir("read-only-api");
    write_team_fixture(&cwd, "fixture-team");

    let config = run_team(
        &[
            "api".to_string(),
            "read-config".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-config");
    let config_stdout = String::from_utf8(config.stdout).expect("utf8");
    assert!(config_stdout.contains("\"command\":\"omx team api read-config\""));
    assert!(config_stdout.contains("\"ok\":true"));
    assert!(config_stdout.contains("\"operation\":\"read-config\""));
    assert!(config_stdout.contains("\"runtime_session_id\": \"omx-team-fixture-team\""));
    assert!(config_stdout.contains("\"tmux_session\": \"omx-team-fixture-team\""));

    let manifest = run_team(
        &[
            "api".to_string(),
            "read-manifest".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-manifest");
    let manifest_stdout = String::from_utf8(manifest.stdout).expect("utf8");
    assert!(manifest_stdout.contains("\"operation\":\"read-manifest\""));
    assert!(manifest_stdout.contains("\"name\": \"fixture-team\""));

    let task = run_team(
        &[
            "api".to_string(),
            "read-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"1"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-task");
    let task_stdout = String::from_utf8(task.stdout).expect("utf8");
    assert!(task_stdout.contains("\"operation\":\"read-task\""));
    assert!(task_stdout.contains("\"subject\": \"fixture task\""));

    let tasks = run_team(
        &[
            "api".to_string(),
            "list-tasks".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("list-tasks");
    let tasks_stdout = String::from_utf8(tasks.stdout).expect("utf8");
    assert!(tasks_stdout.contains("\"operation\":\"list-tasks\""));
    assert!(tasks_stdout.contains("\"count\":1"));
    assert!(tasks_stdout.contains("\"subject\": \"fixture task\""));

    let mailbox = run_team(
        &[
            "api".to_string(),
            "mailbox-list".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-1"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("mailbox-list");
    let mailbox_stdout = String::from_utf8(mailbox.stdout).expect("utf8");
    assert!(mailbox_stdout.contains("\"operation\":\"mailbox-list\""));
    assert!(mailbox_stdout.contains("\"message_id\": \"msg-1\""));
    assert!(mailbox_stdout.contains("\"messages\":["));

    let events = run_team(
        &[
            "api".to_string(),
            "read-events".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","type":"worker_idle","worker":"worker-1"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-events");
    let events_stdout = String::from_utf8(events.stdout).expect("utf8");
    assert!(events_stdout.contains("\"operation\":\"read-events\""));
    assert!(events_stdout.contains("\"count\":1"));
    assert!(events_stdout.contains("\"source_type\":\"worker_idle\""));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn read_only_team_api_supports_prompt_runtime_metadata() {
    let cwd = temp_dir("read-only-prompt-api");
    write_prompt_fixture(&cwd, "prompt-team");

    let config = run_team(
        &[
            "api".to_string(),
            "read-config".to_string(),
            "--input".to_string(),
            r#"{"team_name":"prompt-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-config");
    let config_stdout = String::from_utf8(config.stdout).expect("utf8");
    assert!(config_stdout.contains("\"runtime_session_id\": \"prompt-prompt-team\""));
    assert!(config_stdout.contains("\"tmux_session\": null"));

    let manifest = run_team(
        &[
            "api".to_string(),
            "read-manifest".to_string(),
            "--input".to_string(),
            r#"{"team_name":"prompt-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-manifest");
    let manifest_stdout = String::from_utf8(manifest.stdout).expect("utf8");
    assert!(manifest_stdout.contains("\"runtime_session_id\": \"prompt-prompt-team\""));
    assert!(manifest_stdout.contains("\"tmux_session\": null"));

    let summary = run_team(
        &[
            "api".to_string(),
            "get-summary".to_string(),
            "--input".to_string(),
            r#"{"team_name":"prompt-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("get-summary");
    let summary_stdout = String::from_utf8(summary.stdout).expect("utf8");
    assert!(summary_stdout.contains("\"runtime_target\":\"prompt-prompt-team\""));
    assert!(summary_stdout.contains("\"worker_launch_mode\":\"prompt\""));
    assert!(summary_stdout.contains("\"spawn_strategy\":\"process\""));
    assert!(summary_stdout.contains("\"hud_strategy\":\"state-backed\""));
    assert!(summary_stdout.contains("\"tmux_required\":false"));
    assert!(summary_stdout.contains("\"tmux_session\":null"));
    assert!(summary_stdout.contains("\"no_tmux_proof\":true"));

    let status = run_team(
        &["status".to_string(), "prompt-team".to_string()],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("status");
    let status_stdout = String::from_utf8(status.stdout).expect("utf8");
    assert!(status_stdout.contains("runtime target: prompt-prompt-team"));
    assert!(status_stdout.contains(
        "layout: display=auto launch=prompt spawn=process reflow=state-backed hud=state-backed"
    ));
    assert!(
        status_stdout.contains(
            "tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true"
        )
    );

    let resume = run_team(
        &["resume".to_string(), "prompt-team".to_string()],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("resume");
    let resume_stdout = String::from_utf8(resume.stdout).expect("utf8");
    assert!(resume_stdout.contains("team=prompt-team resumed phase=unknown"));
    assert!(resume_stdout.contains("runtime target: prompt-prompt-team"));
    assert!(resume_stdout.contains(
        "layout: display=auto launch=prompt spawn=process reflow=state-backed hud=state-backed"
    ));
    assert!(
        resume_stdout.contains(
            "tmux: required=false session=null hud_pane=null resize_hook=none no_tmux=true"
        )
    );
    assert!(resume_stdout.contains("workers=1"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn missing_read_only_team_api_returns_null_shapes() {
    let cwd = temp_dir("missing-read-only");
    write_team_fixture(&cwd, "fixture-team");

    let missing_task = run_team(
        &[
            "api".to_string(),
            "read-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"999"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("missing read-task");
    let missing_task_stdout = String::from_utf8(missing_task.stdout).expect("utf8");
    assert!(missing_task_stdout.contains("\"task\":null"));

    let missing_mailbox = run_team(
        &[
            "api".to_string(),
            "mailbox-list".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-9"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("missing mailbox-list");
    let missing_mailbox_stdout = String::from_utf8(missing_mailbox.stdout).expect("utf8");
    assert!(missing_mailbox_stdout.contains("\"mailbox\":null"));
    assert!(missing_mailbox_stdout.contains("\"messages\":[]"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn await_json_semantics_cover_event_timeout_and_missing_cases() {
    let cwd = temp_dir("await-json");
    write_team_fixture(&cwd, "fixture-team");

    let api_event = run_team(
        &[
            "api".to_string(),
            "await-event".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","after_event_id":"evt-1","timeout_ms":1}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("await-event event");
    let api_event_stdout = String::from_utf8(api_event.stdout).expect("utf8");
    assert!(api_event_stdout.contains("\"operation\":\"await-event\""));
    assert!(api_event_stdout.contains("\"status\":\"event\""));
    assert!(api_event_stdout.contains("\"cursor\":\"evt-2\""));
    assert!(api_event_stdout.contains("\"type\":\"task_completed\""));

    let api_timeout = run_team(
        &[
            "api".to_string(),
            "await-event".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","after_event_id":"evt-2","timeout_ms":1}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("await-event timeout");
    let api_timeout_stdout = String::from_utf8(api_timeout.stdout).expect("utf8");
    assert!(api_timeout_stdout.contains("\"status\":\"timeout\""));
    assert!(api_timeout_stdout.contains("\"cursor\":\"evt-2\""));

    let cli_missing = run_team(
        &[
            "await".to_string(),
            "missing-team".to_string(),
            "--timeout-ms".to_string(),
            "1".to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("cli await missing");
    let cli_missing_stdout = String::from_utf8(cli_missing.stdout).expect("utf8");
    assert!(cli_missing_stdout.contains("\"team_name\":\"missing-team\""));
    assert!(cli_missing_stdout.contains("\"status\":\"missing\""));

    let cli_timeout = run_team(
        &[
            "await".to_string(),
            "fixture-team".to_string(),
            "--after-event-id".to_string(),
            "evt-2".to_string(),
            "--timeout-ms".to_string(),
            "1".to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("cli await timeout");
    let cli_timeout_stdout = String::from_utf8(cli_timeout.stdout).expect("utf8");
    assert!(cli_timeout_stdout.contains("\"team_name\":\"fixture-team\""));
    assert!(cli_timeout_stdout.contains("\"status\":\"timeout\""));
    assert!(cli_timeout_stdout.contains("\"cursor\":\"evt-2\""));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn send_message_json_contract_reports_success_and_message_metadata() {
    let cwd = temp_dir("send-message-success");
    write_team_fixture(&cwd, "fixture-team");

    let result = run_team(
        &[
            "api".to_string(),
            "send-message".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","from_worker":"worker-1","to_worker":"leader-fixed","body":"ACK"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("send-message should return json envelope");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("\"command\":\"omx team api send-message\""));
    assert!(stdout.contains("\"ok\":true"));
    assert!(stdout.contains("\"operation\":\"send-message\""));
    assert!(stdout.contains("\"from_worker\":\"worker-1\""));
    assert!(stdout.contains("\"to_worker\":\"leader-fixed\""));
    assert!(stdout.contains("\"body\":\"ACK\""));
    assert!(stdout.contains("\"message_id\":\"msg-"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn send_message_mutates_target_mailbox() {
    let cwd = temp_dir("send-message-mailbox");
    write_team_fixture(&cwd, "fixture-team");
    let mailbox_path = cwd.join(".omx/state/team/fixture-team/mailbox/worker-1.json");
    let before = std::fs::read_to_string(&mailbox_path).expect("read mailbox before");

    let result = run_team(
        &[
            "api".to_string(),
            "send-message".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","from_worker":"leader-fixed","to_worker":"worker-1","body":"follow-up"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("send-message should return json envelope");
    let after = std::fs::read_to_string(&mailbox_path).expect("read mailbox after");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert_ne!(
        before, after,
        "mailbox should change after native send-message succeeds"
    );
    assert!(after.contains("follow-up"));
    assert!(after.contains("leader-fixed"));
    assert!(stdout.contains("\"ok\":true"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn send_message_json_contract_reports_invalid_input_errors() {
    let cwd = temp_dir("send-message-error");
    write_team_fixture(&cwd, "fixture-team");

    let result = run_team(
        &[
            "api".to_string(),
            "send-message".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","to_worker":"worker-1","body":"follow-up"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("invalid send-message should still return json envelope");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("\"ok\":false"));
    assert!(stdout.contains("\"operation\":\"send-message\""));
    assert!(stdout.contains("\"code\":\"runtime_error\""));
    assert!(stdout.contains("from_worker is required"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn read_idle_state_json_contract_reports_structured_success() {
    let cwd = temp_dir("read-idle-state");
    write_team_fixture(&cwd, "fixture-team");

    let result = run_team(
        &[
            "api".to_string(),
            "read-idle-state".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-idle-state json");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("\"operation\":\"read-idle-state\""));
    assert!(stdout.contains("\"ok\":true"));
    assert!(stdout.contains("\"team_name\":\"fixture-team\""));
    assert!(stdout.contains("\"idle_worker_count\":0"));
    assert!(stdout.contains("\"all_workers_idle\":false"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn read_stall_state_json_contract_reports_structured_success() {
    let cwd = temp_dir("read-stall-state");
    write_team_fixture(&cwd, "fixture-team");

    let result = run_team(
        &[
            "api".to_string(),
            "read-stall-state".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-stall-state json");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("\"operation\":\"read-stall-state\""));
    assert!(stdout.contains("\"ok\":true"));
    assert!(stdout.contains("\"team_name\":\"fixture-team\""));
    assert!(stdout.contains("\"team_stalled\":true"));
    assert!(stdout.contains("\"pending_task_count\":0"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn get_summary_json_contract_reports_structured_success() {
    let cwd = temp_dir("get-summary");
    write_team_fixture(&cwd, "fixture-team");

    let result = run_team(
        &[
            "api".to_string(),
            "get-summary".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("get-summary json");
    let stdout = String::from_utf8(result.stdout).expect("utf8");

    assert!(stdout.contains("\"operation\":\"get-summary\""));
    assert!(stdout.contains("\"ok\":true"));
    assert!(stdout.contains("\"teamName\":\"fixture-team\""));
    assert!(stdout.contains("\"workerCount\":1"));
    assert!(stdout.contains("\"pending\":0"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn mutation_team_api_contracts_write_mailbox_worker_and_control_state() {
    let cwd = temp_dir("mutation-ops");
    write_mutation_fixture(&cwd, "fixture-team");

    let broadcast = run_team(
        &[
            "api".to_string(),
            "broadcast".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","from_worker":"worker-1","body":"ping"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("broadcast");
    let broadcast_stdout = String::from_utf8(broadcast.stdout).expect("utf8");
    assert!(broadcast_stdout.contains("\"operation\":\"broadcast\""));
    assert!(broadcast_stdout.contains("\"count\":1"));
    let worker_two_mailbox =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/mailbox/worker-2.json"))
            .expect("read worker-2 mailbox");
    assert!(worker_two_mailbox.contains("ping"));

    let notified = run_team(
        &[
            "api".to_string(),
            "mailbox-mark-notified".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-1","message_id":"msg-1"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("mailbox-mark-notified");
    let notified_stdout = String::from_utf8(notified.stdout).expect("utf8");
    assert!(notified_stdout.contains("\"notified\":true"));

    let delivered = run_team(
        &[
            "api".to_string(),
            "mailbox-mark-delivered".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-1","message_id":"msg-1"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("mailbox-mark-delivered");
    let delivered_stdout = String::from_utf8(delivered.stdout).expect("utf8");
    assert!(delivered_stdout.contains("\"updated\":true"));
    assert!(delivered_stdout.contains("\"dispatch_request_id\":\"req-1\""));
    let mailbox_after =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/mailbox/worker-1.json"))
            .expect("read worker-1 mailbox");
    assert!(mailbox_after.contains("notified_at"));
    assert!(mailbox_after.contains("delivered_at"));
    let dispatch_after =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/dispatch/requests.json"))
            .expect("read dispatch");
    let dispatch_items = split_top_level_json_array_items(&dispatch_after);
    let dispatch_item = dispatch_items.last().expect("dispatch item");
    assert_eq!(
        extract_json_string_field(dispatch_item, "status").as_deref(),
        Some("delivered")
    );

    run_team(
        &[
            "api".to_string(),
            "update-worker-heartbeat".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-1","pid":12345,"turn_count":9,"alive":true}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("update-worker-heartbeat");
    let heartbeat = std::fs::read_to_string(
        cwd.join(".omx/state/team/fixture-team/workers/worker-1/heartbeat.json"),
    )
    .expect("heartbeat");
    assert!(heartbeat.contains("\"pid\": 12345"));
    assert!(heartbeat.contains("\"turn_count\": 9"));

    run_team(
        &[
            "api".to_string(),
            "write-worker-inbox".to_string(),
            "--input".to_string(),
            "{\"team_name\":\"fixture-team\",\"worker\":\"worker-2\",\"content\":\"# Inbox\\nContinue.\"}".to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("write-worker-inbox");
    let inbox =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/workers/worker-2/inbox.md"))
            .expect("inbox");
    assert!(inbox.contains("Continue."));

    run_team(
        &[
            "api".to_string(),
            "write-worker-identity".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-2","index":2,"role":"executor","assigned_tasks":["1"],"pane_id":"%3"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("write-worker-identity");
    let identity = std::fs::read_to_string(
        cwd.join(".omx/state/team/fixture-team/workers/worker-2/identity.json"),
    )
    .expect("identity");
    assert!(identity.contains("\"assigned_tasks\":[\"1\"]"));
    assert!(identity.contains("\"pane_id\":\"%3\""));

    run_team(
        &[
            "api".to_string(),
            "write-shutdown-request".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-2","requested_by":"leader-fixed"}"#
                .to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("write-shutdown-request");
    let shutdown_request = std::fs::read_to_string(
        cwd.join(".omx/state/team/fixture-team/workers/worker-2/shutdown-request.json"),
    )
    .expect("shutdown request");
    assert!(shutdown_request.contains("\"requested_by\":\"leader-fixed\""));

    run_team(
        &[
            "api".to_string(),
            "write-monitor-snapshot".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","snapshot":{"taskStatusById":{"1":"completed"}}}"#
                .to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("write-monitor-snapshot");
    let snapshot =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/monitor-snapshot.json"))
            .expect("snapshot");
    assert!(snapshot.contains("\"completed\""));

    run_team(
        &[
            "api".to_string(),
            "append-event".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","type":"worker_stopped","worker":"worker-2","reason":"manual-stop"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("append-event");
    let events =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/events/events.ndjson"))
            .expect("events");
    assert!(events.contains("manual-stop"));

    run_team(
        &[
            "api".to_string(),
            "write-task-approval".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"1","status":"approved","reviewer":"leader-fixed","decision_reason":"looks good"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("write-task-approval");
    let approval =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/approvals/task-1.json"))
            .expect("approval");
    assert!(approval.contains("\"status\":\"approved\""));
    let events_after =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/events/events.ndjson"))
            .expect("events after approval");
    assert!(events_after.contains("\"type\":\"approval_decision\""));

    let shutdown_ack = run_team(
        &[
            "api".to_string(),
            "read-shutdown-ack".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","worker":"worker-1","min_updated_at":"2026-03-11T01:30:00.000Z"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("read-shutdown-ack");
    let shutdown_ack_stdout = String::from_utf8(shutdown_ack.stdout).expect("utf8");
    assert!(shutdown_ack_stdout.contains("\"ack\":null"));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn task_lifecycle_team_api_contracts_update_versions_and_completion_state() {
    let cwd = temp_dir("task-lifecycle");
    write_mutation_fixture(&cwd, "fixture-team");

    let create = run_team(
        &[
            "api".to_string(),
            "create-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","subject":"new task","description":"from rust","requires_code_change":true}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("create-task");
    let create_stdout = String::from_utf8(create.stdout).expect("utf8");
    assert!(create_stdout.contains("\"operation\":\"create-task\""));
    assert!(create_stdout.contains("\"id\":\"2\""));
    let config = std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/config.json"))
        .expect("config");
    let manifest =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/manifest.v2.json"))
            .expect("manifest");
    assert_eq!(
        extract_json_value(&config, "next_task_id").as_deref(),
        Some("3")
    );
    assert_eq!(
        extract_json_value(&manifest, "next_task_id").as_deref(),
        Some("3")
    );

    let update = run_team(
        &[
            "api".to_string(),
            "update-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"1","subject":"updated subject","blocked_by":["2"],"requires_code_change":true}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("update-task");
    let update_stdout = String::from_utf8(update.stdout).expect("utf8");
    assert!(update_stdout.contains("\"updated subject\""));
    assert!(update_stdout.contains("\"version\":3"));

    let blocked_claim = run_team(
        &[
            "api".to_string(),
            "claim-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"1","worker":"worker-1","expected_version":3}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("blocked claim");
    let blocked_claim_stdout = String::from_utf8(blocked_claim.stdout).expect("utf8");
    assert!(blocked_claim_stdout.contains("\"blocked_dependency\""));

    let claim_two = run_team(
        &[
            "api".to_string(),
            "claim-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"2","worker":"worker-2","expected_version":1}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("claim task 2");
    let claim_two_stdout = String::from_utf8(claim_two.stdout).expect("utf8");
    let claim_two_token = extract_claim_token(&claim_two_stdout);

    let transition_two = run_team(
        &[
            "api".to_string(),
            "transition-task-status".to_string(),
            "--input".to_string(),
            format!(
                "{{\"team_name\":\"fixture-team\",\"task_id\":\"2\",\"from\":\"in_progress\",\"to\":\"completed\",\"claim_token\":\"{}\"}}",
                claim_two_token
            ),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("transition task 2");
    let transition_two_stdout = String::from_utf8(transition_two.stdout).expect("utf8");
    assert!(transition_two_stdout.contains("\"ok\":true"));

    let claim_one = run_team(
        &[
            "api".to_string(),
            "claim-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"1","worker":"worker-1","expected_version":3}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("claim task 1");
    let claim_one_stdout = String::from_utf8(claim_one.stdout).expect("utf8");
    let claim_one_token = extract_claim_token(&claim_one_stdout);
    assert!(claim_one_stdout.contains("\"status\":\"in_progress\""));

    let release = run_team(
        &[
            "api".to_string(),
            "release-task-claim".to_string(),
            "--input".to_string(),
            format!(
                "{{\"team_name\":\"fixture-team\",\"task_id\":\"1\",\"claim_token\":\"{}\",\"worker\":\"worker-1\"}}",
                claim_one_token
            ),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("release-task-claim");
    let release_stdout = String::from_utf8(release.stdout).expect("utf8");
    assert!(release_stdout.contains("\"status\":\"pending\""));

    let claim_again = run_team(
        &[
            "api".to_string(),
            "claim-task".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team","task_id":"1","worker":"worker-1","expected_version":5}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("claim task 1 again");
    let claim_again_stdout = String::from_utf8(claim_again.stdout).expect("utf8");
    let claim_again_token = extract_claim_token(&claim_again_stdout);

    run_team(
        &[
            "api".to_string(),
            "transition-task-status".to_string(),
            "--input".to_string(),
            format!(
                "{{\"team_name\":\"fixture-team\",\"task_id\":\"1\",\"from\":\"in_progress\",\"to\":\"completed\",\"claim_token\":\"{}\"}}",
                claim_again_token
            ),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("transition task 1");

    let task_one =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/tasks/task-1.json"))
            .expect("task one");
    assert!(task_one.contains("\"status\":\"completed\""));
    assert!(task_one.contains("\"version\":7"));
    let snapshot =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/monitor-snapshot.json"))
            .expect("snapshot");
    assert!(snapshot.contains("\"1\":true"));
    let events =
        std::fs::read_to_string(cwd.join(".omx/state/team/fixture-team/events/events.ndjson"))
            .expect("events");
    assert!(events.contains("\"type\":\"task_completed\""));

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn resume_shutdown_and_cleanup_commands_are_state_backed() {
    let cwd = temp_dir("resume-shutdown");
    write_mutation_fixture(&cwd, "fixture-team");

    let resume = run_team(
        &["resume".to_string(), "fixture-team".to_string()],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("resume");
    let resume_stdout = String::from_utf8(resume.stdout).expect("utf8");
    assert!(resume_stdout.contains("team=fixture-team resumed phase=team-exec"));
    assert!(resume_stdout.contains("workers=2"));

    let shutdown_blocked = run_team(
        &["shutdown".to_string(), "fixture-team".to_string()],
        &cwd,
        &BTreeMap::new(),
    )
    .expect_err("shutdown should be blocked while work remains");
    assert!(
        shutdown_blocked
            .to_string()
            .contains("shutdown_gate_blocked")
    );

    let cleanup = run_team(
        &[
            "api".to_string(),
            "cleanup".to_string(),
            "--input".to_string(),
            r#"{"team_name":"fixture-team"}"#.to_string(),
            "--json".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("cleanup");
    let cleanup_stdout = String::from_utf8(cleanup.stdout).expect("utf8");
    assert!(cleanup_stdout.contains("\"operation\":\"cleanup\""));
    assert_eq!(
        String::from_utf8(
            run_team(
                &["status".to_string(), "fixture-team".to_string()],
                &cwd,
                &BTreeMap::new(),
            )
            .expect("status after cleanup")
            .stdout
        )
        .expect("utf8"),
        "No team state found for fixture-team\n"
    );

    write_mutation_fixture(&cwd, "fixture-team");
    let shutdown_force = run_team(
        &[
            "shutdown".to_string(),
            "fixture-team".to_string(),
            "--force".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("force shutdown");
    let shutdown_force_stdout = String::from_utf8(shutdown_force.stdout).expect("utf8");
    assert_eq!(
        shutdown_force_stdout,
        "Team shutdown complete: fixture-team\n"
    );
    assert!(!cwd.join(".omx/state/team/fixture-team").exists());

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[test]
fn missing_team_status_message_is_stable() {
    let cwd = temp_dir("missing");

    let result = run_team(
        &["status".to_string(), "missing-team".to_string()],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("team status");
    let stdout = String::from_utf8(result.stdout).expect("utf8");
    assert_eq!(stdout, "No team state found for missing-team\n");

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}

#[cfg(unix)]
#[test]
fn prompt_team_start_and_shutdown_prove_tmux_free_worker_bootstrap_contract() {
    let cwd = temp_dir("Prompt-Worker-CLI");
    let worker_cli = write_worker_stub(&cwd, "WorkerCLI");
    let mut env = BTreeMap::new();
    env.insert(
        "OMX_TEAM_WORKER_CLI".into(),
        worker_cli.as_os_str().to_os_string(),
    );
    env.insert("OMX_SESSION_ID".into(), "sess-native-team".into());

    let start =
        run_team(&["prompt parity proof".to_string()], &cwd, &env).expect("prompt team start");
    let start_stdout = String::from_utf8(start.stdout).expect("utf8");
    assert!(start_stdout.contains("Team started: prompt-parity-proof"));
    assert!(start_stdout.contains("runtime target: prompt-prompt-parity-proof"));

    let team_root = cwd.join(".omx/state/team/prompt-parity-proof");
    let config = std::fs::read_to_string(team_root.join("config.json")).expect("read config");
    let manifest =
        std::fs::read_to_string(team_root.join("manifest.v2.json")).expect("read manifest");
    let layout_state =
        std::fs::read_to_string(team_root.join("layout-state.json")).expect("read layout state");
    let team_mode_state = std::fs::read_to_string(cwd.join(".omx/state/team-state.json"))
        .expect("read team mode state");
    let identity = std::fs::read_to_string(team_root.join("workers/worker-1/identity.json"))
        .expect("read worker identity");
    let inbox =
        std::fs::read_to_string(team_root.join("workers/worker-1/inbox.md")).expect("read inbox");
    let launch =
        std::fs::read_to_string(team_root.join("workers/worker-1/launch.json")).expect("launch");

    assert!(config.contains("\"worker_launch_mode\": \"prompt\""));
    assert!(config.contains("\"tmux_session\": null"));
    assert!(config.contains("\"leader_pane_id\": null"));
    assert!(config.contains("\"hud_pane_id\": null"));
    assert!(config.contains("\"resize_hook_name\": null"));
    assert!(config.contains(&format!("\"leader_cwd\": \"{}\"", cwd.display())));
    assert!(config.contains(&format!(
        "\"team_state_root\": \"{}\"",
        cwd.join(".omx/state").display()
    )));
    assert!(config.contains("\"workspace_mode\": \"single\""));
    assert!(config.contains("\"assigned_tasks\":[\"1\"]"));
    assert!(config.contains("\"working_dir\":\""));
    assert!(config.contains("\"worker_cli\":\""));

    assert!(manifest.contains("\"display_mode\":\"auto\""));
    assert!(manifest.contains("\"worker_launch_mode\":\"prompt\""));
    assert!(manifest.contains("\"tmux_session\": null"));
    assert!(manifest.contains("\"hud_pane_id\": null"));
    assert!(manifest.contains("\"resize_hook_target\": null"));
    assert!(manifest.contains("\"session_id\":\"sess-native-team\""));
    assert!(layout_state.contains("\"mode\": \"native_equivalent\""));
    assert!(layout_state.contains(
        "\"operator_contract\": \"leader-primary | workers-secondary-stack | hud-footer\""
    ));
    assert!(layout_state.contains("\"last_reason\": \"spawn\""));
    assert!(layout_state.contains("\"no_tmux\": true"));
    assert!(team_mode_state.contains("\"active\": true"));
    assert!(team_mode_state.contains("\"layout_mode\": \"native_equivalent\""));
    assert!(team_mode_state.contains("\"hud_mode\": \"inline\""));
    assert!(team_mode_state.contains("\"no_tmux\": true"));

    assert!(identity.contains("\"assigned_tasks\":[\"1\"]"));
    assert!(identity.contains(&format!("\"working_dir\":\"{}\"", cwd.display())));
    assert!(identity.contains(&format!(
        "\"team_state_root\":\"{}\"",
        cwd.join(".omx/state").display()
    )));

    assert!(inbox.contains("# Worker Assignment: worker-1"));
    assert!(inbox.contains("**Role:** executor"));
    assert!(inbox.contains("**Task 1**: prompt parity proof"));
    assert!(inbox.contains("Verification:"));
    assert!(inbox.contains(&format!(
        "{}/team/prompt-parity-proof/tasks/task-1.json",
        cwd.join(".omx/state").display()
    )));

    assert!(launch.contains(&format!("\"worker_cli\": \"{}\"", worker_cli.display())));

    let shutdown = run_team(
        &[
            "shutdown".to_string(),
            "prompt-parity-proof".to_string(),
            "--force".to_string(),
        ],
        &cwd,
        &BTreeMap::new(),
    )
    .expect("prompt team shutdown");
    let shutdown_stdout = String::from_utf8(shutdown.stdout).expect("utf8");
    assert_eq!(
        shutdown_stdout,
        "Team shutdown complete: prompt-parity-proof\n"
    );
    let team_mode_after =
        std::fs::read_to_string(cwd.join(".omx/state/team-state.json")).expect("team mode after");
    assert!(team_mode_after.contains("\"active\": false"));
    assert!(team_mode_after.contains("\"team_name\": \"prompt-parity-proof\""));
    assert!(!team_root.exists());

    std::fs::remove_dir_all(&cwd).expect("cleanup temp dir");
}
