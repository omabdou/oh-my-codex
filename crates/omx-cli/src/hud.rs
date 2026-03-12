use crate::session_state::{
    extract_json_bool_field, extract_json_string_field, read_current_session_id, resolve_state_root,
};
use crate::team_layout::{
    HudModeOverride, find_active_prompt_team_root, sync_prompt_layout_from_state,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudExecution {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudError(String);

impl HudError {
    fn runtime(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for HudError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HudError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HudPreset {
    Minimal,
    Focused,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HudFlags {
    watch: bool,
    json: bool,
    tmux: bool,
    preset: Option<HudPreset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RalphStateForHud {
    iteration: String,
    max_iterations: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AutopilotStateForHud {
    current_phase: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TeamStateForHud {
    team_name: Option<String>,
    agent_count: Option<String>,
    layout_mode: Option<String>,
    layout_density: Option<String>,
    layout_signature: Option<String>,
    layout_columns: Option<String>,
    layout_rows: Option<String>,
    hud_mode: Option<String>,
    no_tmux: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HudMetrics {
    session_turns: Option<String>,
    total_turns: Option<String>,
    session_total_tokens: Option<String>,
    session_input_tokens: Option<String>,
    session_output_tokens: Option<String>,
    five_hour_limit_pct: Option<String>,
    weekly_limit_pct: Option<String>,
    last_activity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HudNotifyState {
    last_turn_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionStateForHud {
    started_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HudContext {
    version: Option<String>,
    git_branch: Option<String>,
    ralph: Option<RalphStateForHud>,
    ultrawork_active: bool,
    autopilot: Option<AutopilotStateForHud>,
    team: Option<TeamStateForHud>,
    metrics: Option<HudMetrics>,
    hud_notify: Option<HudNotifyState>,
    session: Option<SessionStateForHud>,
}

#[allow(clippy::missing_errors_doc)]
pub fn run_hud(args: &[String], cwd: &Path, help_output: &str) -> Result<HudExecution, HudError> {
    let flags = parse_flags(args)?;
    if matches!(
        args.first().map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        return Ok(HudExecution {
            stdout: help_output.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
        });
    }

    if flags.tmux {
        return Ok(HudExecution {
            stdout: Vec::new(),
            stderr: b"[omx] native Rust HUD does not support a tmux-only launch path; use `omx hud` or `omx hud --watch`\n".to_vec(),
            exit_code: 1,
        });
    }

    sync_active_team_layout_for_hud(cwd, flags.watch);

    if flags.watch {
        run_hud_watch(cwd, flags)
    } else {
        let context = read_hud_context(cwd);
        if flags.json {
            let json = render_hud_json(&context, flags.preset.unwrap_or(HudPreset::Focused));
            Ok(HudExecution {
                stdout: json.into_bytes(),
                stderr: Vec::new(),
                exit_code: 0,
            })
        } else {
            let line = render_hud_line(&context, flags.preset.unwrap_or(HudPreset::Focused));
            Ok(HudExecution {
                stdout: format!("{line}\n").into_bytes(),
                stderr: Vec::new(),
                exit_code: 0,
            })
        }
    }
}

fn sync_active_team_layout_for_hud(cwd: &Path, watch: bool) {
    let state_root = resolve_state_root(cwd, &BTreeMap::<OsString, OsString>::new());
    let Ok(Some(team_root)) = find_active_prompt_team_root(cwd, &state_root) else {
        return;
    };
    let hud_mode = if watch {
        HudModeOverride::Watch
    } else {
        HudModeOverride::Inline
    };
    let _ = sync_prompt_layout_from_state(
        &team_root,
        &state_root,
        if watch { "hud-watch" } else { "hud-render" },
        hud_mode,
        None,
    );
}

fn run_hud_watch(cwd: &Path, flags: HudFlags) -> Result<HudExecution, HudError> {
    run_hud_watch_with_limit(cwd, flags, None)
}

fn run_hud_watch_with_limit(
    cwd: &Path,
    flags: HudFlags,
    max_ticks_override: Option<usize>,
) -> Result<HudExecution, HudError> {
    let mut stdout = io::stdout();
    let mut first_render = true;
    let max_ticks = max_ticks_override.or_else(|| {
        std::env::var("OMX_HUD_MAX_TICKS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
    });
    let mut ticks = 0_usize;
    loop {
        let context = read_hud_context(cwd);
        let line = if flags.json {
            render_hud_json(&context, flags.preset.unwrap_or(HudPreset::Focused))
        } else {
            render_hud_line(&context, flags.preset.unwrap_or(HudPreset::Focused))
        };
        if first_render {
            stdout.write_all(b"\x1b[2J\x1b[H").ok();
            first_render = false;
        } else {
            stdout.write_all(b"\x1b[H").ok();
        }
        stdout.write_all(line.as_bytes()).ok();
        stdout.write_all(b"\x1b[K\n\x1b[J").ok();
        stdout.flush().ok();

        ticks += 1;
        if max_ticks.is_some_and(|limit| ticks >= limit) {
            break;
        }
        thread::sleep(Duration::from_millis(1000));
    }
    stdout.write_all(b"\x1b[?25h\x1b[2J\x1b[H").ok();
    stdout.flush().ok();
    Ok(HudExecution {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: 0,
    })
}

fn parse_flags(args: &[String]) -> Result<HudFlags, HudError> {
    let mut flags = HudFlags {
        watch: false,
        json: false,
        tmux: false,
        preset: None,
    };
    for arg in args {
        match arg.as_str() {
            "--watch" | "-w" => flags.watch = true,
            "--json" => flags.json = true,
            "--tmux" => flags.tmux = true,
            "--help" | "-h" | "help" => {}
            other if other.starts_with("--preset=") => {
                flags.preset = Some(parse_preset(&other["--preset=".len()..])?);
            }
            other => {
                return Err(HudError::runtime(format!(
                    "Unknown argument for `omx hud`: {other}"
                )));
            }
        }
    }
    Ok(flags)
}

fn parse_preset(value: &str) -> Result<HudPreset, HudError> {
    match value {
        "minimal" => Ok(HudPreset::Minimal),
        "focused" => Ok(HudPreset::Focused),
        "full" => Ok(HudPreset::Full),
        other => Err(HudError::runtime(format!(
            "Unknown HUD preset: {other}. Expected one of: minimal, focused, full"
        ))),
    }
}

fn read_hud_context(cwd: &Path) -> HudContext {
    HudContext {
        version: Some(format!("v{}", env!("CARGO_PKG_VERSION"))),
        git_branch: read_git_branch(cwd),
        ralph: read_ralph_state(cwd),
        ultrawork_active: read_mode_active(cwd, "ultrawork"),
        autopilot: read_autopilot_state(cwd),
        team: read_team_state(cwd),
        metrics: read_metrics(cwd),
        hud_notify: read_hud_notify_state(cwd),
        session: read_session_state(cwd),
    }
}

fn read_mode_file(cwd: &Path, mode: &str) -> Option<String> {
    let state_root = resolve_state_root(cwd, &BTreeMap::<OsString, OsString>::new());
    let mut candidates = Vec::new();
    if let Some(session_id) = read_current_session_id(&state_root) {
        candidates.push(
            state_root
                .join("sessions")
                .join(session_id)
                .join(format!("{mode}-state.json")),
        );
    }
    candidates.push(state_root.join(format!("{mode}-state.json")));
    candidates
        .into_iter()
        .find_map(|path| fs::read_to_string(path).ok())
}

fn read_mode_active(cwd: &Path, mode: &str) -> bool {
    read_mode_file(cwd, mode).and_then(|raw| extract_json_bool_field(&raw, "active")) == Some(true)
}

fn read_ralph_state(cwd: &Path) -> Option<RalphStateForHud> {
    let raw = read_mode_file(cwd, "ralph")?;
    if extract_json_bool_field(&raw, "active") != Some(true) {
        return None;
    }
    Some(RalphStateForHud {
        iteration: extract_json_number_like_field(&raw, "iteration")
            .unwrap_or_else(|| "0".to_string()),
        max_iterations: extract_json_number_like_field(&raw, "max_iterations")
            .unwrap_or_else(|| "0".to_string()),
    })
}

fn read_autopilot_state(cwd: &Path) -> Option<AutopilotStateForHud> {
    let raw = read_mode_file(cwd, "autopilot")?;
    if extract_json_bool_field(&raw, "active") != Some(true) {
        return None;
    }
    Some(AutopilotStateForHud {
        current_phase: extract_json_string_field(&raw, "current_phase"),
    })
}

fn read_team_state(cwd: &Path) -> Option<TeamStateForHud> {
    let raw = read_mode_file(cwd, "team")?;
    if extract_json_bool_field(&raw, "active") != Some(true) {
        return None;
    }
    Some(TeamStateForHud {
        team_name: extract_json_string_field(&raw, "team_name"),
        agent_count: extract_json_number_like_field(&raw, "agent_count"),
        layout_mode: extract_json_string_field(&raw, "layout_mode"),
        layout_density: extract_json_string_field(&raw, "layout_density"),
        layout_signature: extract_json_string_field(&raw, "layout_signature"),
        layout_columns: extract_json_number_like_field(&raw, "layout_columns"),
        layout_rows: extract_json_number_like_field(&raw, "layout_rows"),
        hud_mode: extract_json_string_field(&raw, "hud_mode"),
        no_tmux: extract_json_bool_field(&raw, "no_tmux"),
    })
}

fn read_metrics(cwd: &Path) -> Option<HudMetrics> {
    let raw = fs::read_to_string(cwd.join(".omx").join("metrics.json")).ok()?;
    Some(HudMetrics {
        session_turns: extract_json_number_like_field(&raw, "session_turns"),
        total_turns: extract_json_number_like_field(&raw, "total_turns"),
        session_total_tokens: extract_json_number_like_field(&raw, "session_total_tokens"),
        session_input_tokens: extract_json_number_like_field(&raw, "session_input_tokens"),
        session_output_tokens: extract_json_number_like_field(&raw, "session_output_tokens"),
        five_hour_limit_pct: extract_json_number_like_field(&raw, "five_hour_limit_pct"),
        weekly_limit_pct: extract_json_number_like_field(&raw, "weekly_limit_pct"),
        last_activity: extract_json_string_field(&raw, "last_activity"),
    })
}

fn read_hud_notify_state(cwd: &Path) -> Option<HudNotifyState> {
    let raw = fs::read_to_string(
        resolve_state_root(cwd, &BTreeMap::<OsString, OsString>::new()).join("hud-state.json"),
    )
    .ok()?;
    Some(HudNotifyState {
        last_turn_at: extract_json_string_field(&raw, "last_turn_at"),
    })
}

fn read_session_state(cwd: &Path) -> Option<SessionStateForHud> {
    let raw = fs::read_to_string(
        resolve_state_root(cwd, &BTreeMap::<OsString, OsString>::new()).join("session.json"),
    )
    .ok()?;
    Some(SessionStateForHud {
        started_at: extract_json_string_field(&raw, "started_at"),
    })
}

fn read_git_branch(cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

fn render_hud_json(context: &HudContext, preset: HudPreset) -> String {
    format!(
        concat!(
            "{{\n",
            "  \"preset\": \"{}\",\n",
            "  \"version\": {},\n",
            "  \"gitBranch\": {},\n",
            "  \"ralph\": {},\n",
            "  \"ultrawork\": {},\n",
            "  \"autopilot\": {},\n",
            "  \"team\": {},\n",
            "  \"metrics\": {},\n",
            "  \"hudNotify\": {},\n",
            "  \"session\": {}\n",
            "}}\n"
        ),
        match preset {
            HudPreset::Minimal => "minimal",
            HudPreset::Focused => "focused",
            HudPreset::Full => "full",
        },
        render_json_string_opt(context.version.as_deref()),
        render_json_string_opt(context.git_branch.as_deref()),
        render_ralph_json(context.ralph.as_ref()),
        if context.ultrawork_active {
            "true"
        } else {
            "false"
        },
        render_autopilot_json(context.autopilot.as_ref()),
        render_team_json(context.team.as_ref()),
        render_metrics_json(context.metrics.as_ref()),
        render_hud_notify_json(context.hud_notify.as_ref()),
        render_session_json(context.session.as_ref()),
    )
}

fn render_hud_line(context: &HudContext, preset: HudPreset) -> String {
    let mut parts = Vec::new();
    if let Some(branch) = &context.git_branch {
        parts.push(branch.clone());
    }
    if let Some(ralph) = &context.ralph {
        parts.push(format!(
            "ralph:{}/{}",
            ralph.iteration, ralph.max_iterations
        ));
    }
    if context.ultrawork_active {
        parts.push("ultrawork".to_string());
    }
    if let Some(autopilot) = &context.autopilot {
        parts.push(format!(
            "autopilot:{}",
            autopilot
                .current_phase
                .clone()
                .unwrap_or_else(|| "active".to_string())
        ));
    }
    if let Some(team) = &context.team {
        if let Some(agent_count) = &team.agent_count {
            parts.push(format!("team:{agent_count} workers"));
        } else if let Some(team_name) = &team.team_name {
            parts.push(format!("team:{team_name}"));
        } else {
            parts.push("team".to_string());
        }
        if let Some(layout_mode) = &team.layout_mode {
            let density = team.layout_density.as_deref().unwrap_or("balanced");
            parts.push(format!("layout:{layout_mode}/{density}"));
        }
        if let Some(hud_mode) = &team.hud_mode {
            parts.push(format!("hud:{hud_mode}"));
        }
        if let (Some(columns), Some(rows)) = (&team.layout_columns, &team.layout_rows) {
            parts.push(format!("viewport:{columns}x{rows}"));
        }
        if team.no_tmux == Some(true) {
            parts.push("no-tmux".to_string());
        }
    }
    if matches!(
        preset,
        HudPreset::Minimal | HudPreset::Focused | HudPreset::Full
    ) {
        if let Some(metrics) = &context.metrics {
            if let Some(session_turns) = &metrics.session_turns {
                parts.push(format!("turns:{session_turns}"));
            }
        }
    }
    if matches!(preset, HudPreset::Focused | HudPreset::Full) {
        if let Some(metrics) = &context.metrics {
            if let Some(total_tokens) = metrics
                .session_total_tokens
                .as_ref()
                .or(metrics.session_input_tokens.as_ref())
            {
                parts.push(format!("tokens:{total_tokens}"));
            }
            let mut quota_parts = Vec::new();
            if let Some(five_hour) = &metrics.five_hour_limit_pct {
                quota_parts.push(format!("5h:{five_hour}%"));
            }
            if let Some(weekly) = &metrics.weekly_limit_pct {
                quota_parts.push(format!("wk:{weekly}%"));
            }
            if !quota_parts.is_empty() {
                parts.push(format!("quota:{}", quota_parts.join(",")));
            }
        }
        if let Some(session) = &context.session {
            if let Some(started_at) = &session.started_at {
                if let Some(duration) = format_duration_from_iso(started_at) {
                    parts.push(format!("session:{duration}"));
                }
            }
        }
        if let Some(hud_notify) = &context.hud_notify {
            if let Some(last_turn_at) = &hud_notify.last_turn_at {
                if let Some(last_seen) = format_last_seen(last_turn_at) {
                    parts.push(format!("last:{last_seen}"));
                }
            }
        }
    }
    if matches!(preset, HudPreset::Full) {
        if let Some(metrics) = &context.metrics {
            if let Some(total_turns) = &metrics.total_turns {
                parts.push(format!("total-turns:{total_turns}"));
            }
        }
        if let Some(team) = &context.team {
            if let Some(layout_signature) = &team.layout_signature {
                parts.push(format!("operator:{layout_signature}"));
            }
        }
    }

    let version = context
        .version
        .as_deref()
        .unwrap_or("unknown")
        .trim_start_matches('v')
        .to_string();
    if parts.is_empty() {
        return format!("[OMX#{version}] No active modes.");
    }
    format!("[OMX#{version}] {}", parts.join(" | "))
}

fn format_duration_from_iso(value: &str) -> Option<String> {
    let started_at = parse_iso_seconds(value)?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let diff = now.saturating_sub(started_at);
    if diff < 60 {
        Some(format!("{diff}s"))
    } else if diff < 3600 {
        Some(format!("{}m", diff / 60))
    } else {
        Some(format!("{}h{}m", diff / 3600, (diff % 3600) / 60))
    }
}

fn format_last_seen(value: &str) -> Option<String> {
    let last_at = parse_iso_seconds(value)?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let diff = now.saturating_sub(last_at);
    if diff < 60 {
        Some(format!("{diff}s ago"))
    } else {
        Some(format!("{}m ago", diff / 60))
    }
}

fn parse_iso_seconds(value: &str) -> Option<u64> {
    let ts = value.trim();
    let year: i32 = ts.get(0..4)?.parse().ok()?;
    let month: i32 = ts.get(5..7)?.parse().ok()?;
    let day: i32 = ts.get(8..10)?.parse().ok()?;
    let hour: i32 = ts.get(11..13)?.parse().ok()?;
    let minute: i32 = ts.get(14..16)?.parse().ok()?;
    let second: i32 = ts.get(17..19)?.parse().ok()?;

    let days = days_from_civil(year, month, day)?;
    Some((days as u64) * 86_400 + (hour as u64) * 3600 + (minute as u64) * 60 + second as u64)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - ((month <= 2) as i32);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) as i64)
}

fn extract_json_number_like_field(raw: &str, key: &str) -> Option<String> {
    let key = format!("\"{key}\"");
    let key_start = raw.find(&key)? + key.len();
    let after_key = raw.get(key_start..)?;
    let colon_idx = after_key.find(':')?;
    let after_colon = after_key.get(colon_idx + 1..)?.trim_start();
    let end = after_colon
        .find([',', '}', '\n', '\r'])
        .unwrap_or(after_colon.len());
    let value = after_colon[..end].trim();
    if value.is_empty() || value == "null" {
        None
    } else {
        Some(value.trim_matches('"').to_string())
    }
}

fn render_json_string_opt(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")),
        None => "null".to_string(),
    }
}

fn render_ralph_json(value: Option<&RalphStateForHud>) -> String {
    match value {
        Some(value) => format!(
            "{{\"iteration\":\"{}\",\"max_iterations\":\"{}\"}}",
            value.iteration, value.max_iterations
        ),
        None => "null".to_string(),
    }
}

fn render_autopilot_json(value: Option<&AutopilotStateForHud>) -> String {
    match value {
        Some(value) => format!(
            "{{\"current_phase\":{}}}",
            render_json_string_opt(value.current_phase.as_deref())
        ),
        None => "null".to_string(),
    }
}

fn render_team_json(value: Option<&TeamStateForHud>) -> String {
    match value {
        Some(value) => format!(
            "{{\"team_name\":{},\"agent_count\":{},\"layout_mode\":{},\"layout_density\":{},\"layout_signature\":{},\"layout_columns\":{},\"layout_rows\":{},\"hud_mode\":{},\"no_tmux\":{}}}",
            render_json_string_opt(value.team_name.as_deref()),
            render_json_string_opt(value.agent_count.as_deref()),
            render_json_string_opt(value.layout_mode.as_deref()),
            render_json_string_opt(value.layout_density.as_deref()),
            render_json_string_opt(value.layout_signature.as_deref()),
            render_json_string_opt(value.layout_columns.as_deref()),
            render_json_string_opt(value.layout_rows.as_deref()),
            render_json_string_opt(value.hud_mode.as_deref()),
            match value.no_tmux {
                Some(true) => "true",
                Some(false) => "false",
                None => "null",
            }
        ),
        None => "null".to_string(),
    }
}

fn render_metrics_json(value: Option<&HudMetrics>) -> String {
    match value {
        Some(value) => format!(
            concat!(
                "{{\"session_turns\":{},\"total_turns\":{},\"session_total_tokens\":{},",
                "\"session_input_tokens\":{},\"session_output_tokens\":{},",
                "\"five_hour_limit_pct\":{},\"weekly_limit_pct\":{},\"last_activity\":{}}}"
            ),
            render_json_string_opt(value.session_turns.as_deref()),
            render_json_string_opt(value.total_turns.as_deref()),
            render_json_string_opt(value.session_total_tokens.as_deref()),
            render_json_string_opt(value.session_input_tokens.as_deref()),
            render_json_string_opt(value.session_output_tokens.as_deref()),
            render_json_string_opt(value.five_hour_limit_pct.as_deref()),
            render_json_string_opt(value.weekly_limit_pct.as_deref()),
            render_json_string_opt(value.last_activity.as_deref()),
        ),
        None => "null".to_string(),
    }
}

fn render_hud_notify_json(value: Option<&HudNotifyState>) -> String {
    match value {
        Some(value) => format!(
            "{{\"last_turn_at\":{}}}",
            render_json_string_opt(value.last_turn_at.as_deref())
        ),
        None => "null".to_string(),
    }
}

fn render_session_json(value: Option<&SessionStateForHud>) -> String {
    match value {
        Some(value) => format!(
            "{{\"started_at\":{}}}",
            render_json_string_opt(value.started_at.as_deref())
        ),
        None => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{HudFlags, HudPreset, parse_flags, run_hud};
    use std::fs;
    use std::path::Path;

    const HELP: &str = "top-level help\n";

    #[test]
    fn prints_top_level_help_for_help_variants() {
        let cwd = std::env::current_dir().expect("cwd");
        for args in [
            vec!["--help".to_string()],
            vec!["-h".to_string()],
            vec!["help".to_string()],
        ] {
            let result = run_hud(&args, &cwd, HELP).expect("hud help");
            assert_eq!(result.stdout, HELP.as_bytes());
            assert!(result.stderr.is_empty());
            assert_eq!(result.exit_code, 0);
        }
    }

    #[test]
    fn parses_watch_json_and_preset_flags() {
        let flags = parse_flags(&[
            "--watch".to_string(),
            "--json".to_string(),
            "--preset=full".to_string(),
        ])
        .expect("flags");
        assert_eq!(
            flags,
            HudFlags {
                watch: true,
                json: true,
                tmux: false,
                preset: Some(HudPreset::Full),
            }
        );
    }

    #[test]
    fn renders_non_tmux_hud_line_from_state() {
        let cwd = temp_dir("hud-render");
        seed_hud_state(&cwd);
        let result = run_hud(&[], &cwd, HELP).expect("hud");
        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert_eq!(result.exit_code, 0);
        assert!(stdout.contains("[OMX#0.8.11]"));
        assert!(stdout.contains("ralph:2/5"));
        assert!(stdout.contains("team:3 workers"));
        assert!(stdout.contains("layout:native_equivalent/balanced"));
        assert!(stdout.contains("hud:watch"));
        assert!(stdout.contains("viewport:144x48"));
        assert!(stdout.contains("no-tmux"));
        assert!(stdout.contains("turns:12"));
    }

    #[test]
    fn renders_json_mode_from_state() {
        let cwd = temp_dir("hud-json");
        seed_hud_state(&cwd);
        let result = run_hud(&["--json".to_string()], &cwd, HELP).expect("hud json");
        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert!(stdout.contains("\"preset\": \"focused\""));
        assert!(stdout.contains("\"ralph\": {\"iteration\":\"2\",\"max_iterations\":\"5\"}"));
        assert!(stdout.contains("\"team\": {\"team_name\":\"alpha\",\"agent_count\":\"3\",\"layout_mode\":\"native_equivalent\",\"layout_density\":\"balanced\""));
        assert!(stdout.contains(
            "\"layout_signature\":\"leader-primary | workers-secondary-stack | hud-footer\""
        ));
        assert!(stdout.contains("\"layout_columns\":\"144\""));
        assert!(stdout.contains("\"layout_rows\":\"48\""));
        assert!(stdout.contains("\"hud_mode\":\"watch\""));
        assert!(stdout.contains("\"no_tmux\":true"));
    }

    #[test]
    fn rejects_tmux_only_launch_path() {
        let cwd = std::env::current_dir().expect("cwd");
        let result = run_hud(&["--tmux".to_string()], &cwd, HELP).expect("hud tmux error");
        let stderr = String::from_utf8(result.stderr).expect("utf8 stderr");
        assert_eq!(result.exit_code, 1);
        assert!(stderr.contains("does not support a tmux-only launch path"));
    }

    #[test]
    fn watch_mode_honors_test_tick_limit() {
        let cwd = temp_dir("hud-watch");
        seed_hud_state(&cwd);
        let result = super::run_hud_watch_with_limit(
            &cwd,
            HudFlags {
                watch: true,
                json: false,
                tmux: false,
                preset: Some(HudPreset::Minimal),
            },
            Some(1),
        )
        .expect("watch");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn full_preset_surfaces_operator_layout_signature() {
        let cwd = temp_dir("hud-full-layout");
        seed_hud_state(&cwd);
        let result = run_hud(&["--preset=full".to_string()], &cwd, HELP).expect("hud full");
        let stdout = String::from_utf8(result.stdout).expect("utf8");
        assert!(stdout.contains("operator:leader-primary | workers-secondary-stack | hud-footer"));
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("omx-rust-hud-{label}-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn seed_hud_state(cwd: &Path) {
        fs::create_dir_all(cwd.join(".omx/state")).expect("create state dir");
        fs::create_dir_all(cwd.join(".omx/state/sessions/sess-1")).expect("create session dir");
        fs::write(
            cwd.join(".omx/state/session.json"),
            "{\"session_id\":\"sess-1\",\"started_at\":\"2026-03-11T00:00:00Z\"}",
        )
        .expect("write session");
        fs::write(
            cwd.join(".omx/state/sessions/sess-1/ralph-state.json"),
            "{\"active\":true,\"iteration\":2,\"max_iterations\":5}",
        )
        .expect("write ralph");
        fs::write(
            cwd.join(".omx/state/sessions/sess-1/team-state.json"),
            concat!(
                "{",
                "\"active\":true,",
                "\"team_name\":\"alpha\",",
                "\"agent_count\":3,",
                "\"layout_mode\":\"native_equivalent\",",
                "\"layout_density\":\"balanced\",",
                "\"layout_signature\":\"leader-primary | workers-secondary-stack | hud-footer\",",
                "\"layout_columns\":144,",
                "\"layout_rows\":48,",
                "\"hud_mode\":\"watch\",",
                "\"no_tmux\":true",
                "}"
            ),
        )
        .expect("write team");
        fs::write(
            cwd.join(".omx/metrics.json"),
            "{\"session_turns\":12,\"total_turns\":44,\"session_total_tokens\":1200}",
        )
        .expect("write metrics");
        fs::write(
            cwd.join(".omx/state/hud-state.json"),
            "{\"last_turn_at\":\"2026-03-11T00:00:30Z\"}",
        )
        .expect("write hud state");
    }
}
