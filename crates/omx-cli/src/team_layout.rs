use crate::session_state::{extract_json_bool_field, extract_json_string_field};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const DEFAULT_LAYOUT_COLUMNS: usize = 140;
const DEFAULT_LAYOUT_ROWS: usize = 42;
const MIN_LEADER_WIDTH: usize = 56;
const MIN_WORKER_WIDTH: usize = 42;
const DEFAULT_HUD_HEIGHT: usize = 4;
const WATCH_HUD_HEIGHT: usize = 6;
const LAYOUT_TRIGGERS_JSON: &str = "[\"spawn\",\"worker-change\",\"hud\",\"resize\"]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudModeOverride {
    Preserve,
    Inline,
    Watch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamLayoutSnapshot {
    pub team_name: String,
    pub runtime_session_id: String,
    pub phase: String,
    pub worker_count: usize,
    pub columns: usize,
    pub rows: usize,
    pub density: String,
    pub hud_mode: String,
    pub hud_height: usize,
    pub leader_width: usize,
    pub leader_height: usize,
    pub worker_width: usize,
    pub worker_height: usize,
    pub reflow_revision: u64,
    pub last_reason: String,
    pub no_tmux: bool,
}

impl TeamLayoutSnapshot {
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!(
            "layout: native_equivalent density={} leader={}x{} workers={}x{} stack={} hud={}/footer/{} reflow_rev={} last={}",
            self.density,
            self.leader_width,
            self.leader_height,
            self.worker_width,
            self.worker_height,
            self.worker_count.max(1),
            self.hud_mode,
            self.hud_height,
            self.reflow_revision,
            self.last_reason,
        )
    }

    #[must_use]
    pub fn proof_line(&self) -> String {
        format!(
            "operator: primary=leader secondary=worker-stack footer=hud no_tmux={} triggers=spawn,worker-change,hud,resize viewport={}x{}",
            self.no_tmux, self.columns, self.rows,
        )
    }

    #[must_use]
    pub fn runtime_line(&self) -> String {
        format!(
            "runtime: mode=prompt session={} no_tmux={} hud_mode={}",
            self.runtime_session_id, self.no_tmux, self.hud_mode
        )
    }

    #[must_use]
    pub fn layout_state_json(&self) -> String {
        format!(
            concat!(
                "{{\n",
                "  \"schema_version\": 1,\n",
                "  \"mode\": \"native_equivalent\",\n",
                "  \"team_name\": \"{}\",\n",
                "  \"runtime_session_id\": \"{}\",\n",
                "  \"phase\": \"{}\",\n",
                "  \"worker_count\": {},\n",
                "  \"no_tmux\": {},\n",
                "  \"viewport\": {{\"columns\": {}, \"rows\": {}}},\n",
                "  \"density\": \"{}\",\n",
                "  \"hud\": {{\"mode\": \"{}\", \"placement\": \"footer\", \"height\": {}}},\n",
                "  \"leader\": {{\"placement\": \"primary\", \"width\": {}, \"height\": {}}},\n",
                "  \"workers\": {{\"placement\": \"secondary_stack\", \"width\": {}, \"height\": {}, \"slots\": {}}},\n",
                "  \"operator_contract\": \"leader-primary | workers-secondary-stack | hud-footer\",\n",
                "  \"reflow\": {{\"revision\": {}, \"last_reason\": \"{}\", \"triggers\": {}}}\n",
                "}}\n"
            ),
            escape_json_string(&self.team_name),
            escape_json_string(&self.runtime_session_id),
            escape_json_string(&self.phase),
            self.worker_count,
            if self.no_tmux { "true" } else { "false" },
            self.columns,
            self.rows,
            escape_json_string(&self.density),
            escape_json_string(&self.hud_mode),
            self.hud_height,
            self.leader_width,
            self.leader_height,
            self.worker_width,
            self.worker_height,
            self.worker_count.max(1),
            self.reflow_revision,
            escape_json_string(&self.last_reason),
            LAYOUT_TRIGGERS_JSON,
        )
    }

    #[must_use]
    pub fn team_mode_state_json(&self) -> String {
        format!(
            concat!(
                "{{\n",
                "  \"active\": true,\n",
                "  \"team_name\": \"{}\",\n",
                "  \"agent_count\": {},\n",
                "  \"current_phase\": \"{}\",\n",
                "  \"worker_launch_mode\": \"prompt\",\n",
                "  \"runtime_session_id\": \"{}\",\n",
                "  \"tmux_session\": null,\n",
                "  \"layout_mode\": \"native_equivalent\",\n",
                "  \"layout_density\": \"{}\",\n",
                "  \"layout_signature\": \"leader-primary | workers-secondary-stack | hud-footer\",\n",
                "  \"layout_columns\": {},\n",
                "  \"layout_rows\": {},\n",
                "  \"hud_mode\": \"{}\",\n",
                "  \"no_tmux\": {}\n",
                "}}\n"
            ),
            escape_json_string(&self.team_name),
            self.worker_count,
            escape_json_string(&self.phase),
            escape_json_string(&self.runtime_session_id),
            escape_json_string(&self.density),
            self.columns,
            self.rows,
            escape_json_string(&self.hud_mode),
            if self.no_tmux { "true" } else { "false" },
        )
    }
}

pub fn sync_prompt_layout_from_state(
    team_root: &Path,
    state_root: &Path,
    reason: &str,
    hud_mode_override: HudModeOverride,
    env: Option<&BTreeMap<OsString, OsString>>,
) -> io::Result<Option<TeamLayoutSnapshot>> {
    let config_raw = read_optional(team_root.join("config.json"))?;
    let manifest_raw = read_optional(team_root.join("manifest.v2.json"))?;
    let Some(primary_raw) = manifest_raw.as_deref().or(config_raw.as_deref()) else {
        return Ok(None);
    };

    let worker_launch_mode = config_raw
        .as_deref()
        .and_then(|raw| extract_json_string_field(raw, "worker_launch_mode"))
        .or_else(|| extract_json_string_field(primary_raw, "worker_launch_mode"))
        .unwrap_or_else(|| "interactive".to_string());
    if worker_launch_mode != "prompt" {
        return Ok(None);
    }

    let team_name = extract_json_string_field(primary_raw, "name")
        .or_else(|| {
            team_root
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "team".to_string());
    let runtime_session_id = extract_json_string_field(primary_raw, "runtime_session_id")
        .unwrap_or_else(|| format!("prompt-{team_name}"));
    let phase = read_optional(team_root.join("phase.json"))?
        .as_deref()
        .and_then(|raw| extract_json_string_field(raw, "current_phase"))
        .unwrap_or_else(|| "unknown".to_string());
    let worker_count = worker_count_from_json(primary_raw).max(1);
    let previous = read_optional(team_root.join("layout-state.json"))?;
    let previous_revision = previous
        .as_deref()
        .and_then(|raw| extract_json_u64(raw, "revision"));
    let previous_hud_mode = previous.as_deref().and_then(extract_hud_mode_from_layout);
    let (columns, rows) = resolve_terminal_dimensions(env);
    let hud_mode = match hud_mode_override {
        HudModeOverride::Preserve => previous_hud_mode.unwrap_or_else(|| "inline".to_string()),
        HudModeOverride::Inline => "inline".to_string(),
        HudModeOverride::Watch => "watch".to_string(),
    };
    let hud_height = if hud_mode == "watch" {
        WATCH_HUD_HEIGHT.min(rows.saturating_sub(8)).max(4)
    } else {
        DEFAULT_HUD_HEIGHT.min(rows.saturating_sub(8)).max(3)
    };
    let leader_width = (columns * 58 / 100).max(MIN_LEADER_WIDTH).min(
        columns
            .saturating_sub(MIN_WORKER_WIDTH)
            .max(MIN_LEADER_WIDTH),
    );
    let worker_width = columns.saturating_sub(leader_width).max(MIN_WORKER_WIDTH);
    let content_rows = rows.saturating_sub(hud_height).max(12);
    let worker_height = (content_rows / worker_count.max(1)).max(6);
    let density = if columns < 116 {
        "compact"
    } else if columns < 168 {
        "balanced"
    } else {
        "wide"
    };
    let no_tmux = config_raw
        .as_deref()
        .and_then(|raw| extract_json_value(raw, "tmux_session"))
        .is_none_or(|value| value.trim() == "null");
    let snapshot = TeamLayoutSnapshot {
        team_name,
        runtime_session_id,
        phase,
        worker_count,
        columns,
        rows,
        density: density.to_string(),
        hud_mode,
        hud_height,
        leader_width,
        leader_height: content_rows,
        worker_width,
        worker_height,
        reflow_revision: previous_revision.unwrap_or(0) + 1,
        last_reason: reason.to_string(),
        no_tmux,
    };

    write_text(
        &team_root.join("layout-state.json"),
        &snapshot.layout_state_json(),
    )?;
    write_text(
        &state_root.join("team-state.json"),
        &snapshot.team_mode_state_json(),
    )?;
    Ok(Some(snapshot))
}

pub fn deactivate_team_mode_state(
    state_root: &Path,
    team_name: &str,
    phase: &str,
) -> io::Result<()> {
    write_text(
        &state_root.join("team-state.json"),
        &format!(
            concat!(
                "{{\n",
                "  \"active\": false,\n",
                "  \"team_name\": \"{}\",\n",
                "  \"current_phase\": \"{}\"\n",
                "}}\n"
            ),
            escape_json_string(team_name),
            escape_json_string(phase),
        ),
    )
}

pub fn find_active_prompt_team_root(cwd: &Path, state_root: &Path) -> io::Result<Option<PathBuf>> {
    let Some(team_state_raw) = read_optional(state_root.join("team-state.json"))? else {
        return Ok(None);
    };
    if extract_json_bool_field(&team_state_raw, "active") != Some(true) {
        return Ok(None);
    }
    let Some(team_name) = extract_json_string_field(&team_state_raw, "team_name") else {
        return Ok(None);
    };
    let team_root = cwd.join(".omx").join("state").join("team").join(team_name);
    if team_root.exists() {
        Ok(Some(team_root))
    } else {
        Ok(None)
    }
}

fn worker_count_from_json(raw: &str) -> usize {
    extract_json_value(raw, "workers")
        .map(|workers| {
            split_top_level_json_array_items(&workers)
                .into_iter()
                .filter(|item| {
                    extract_json_string_field(item, "name")
                        .is_some_and(|name| name.starts_with("worker-"))
                })
                .count()
        })
        .filter(|count| *count > 0)
        .or_else(|| {
            extract_json_u64(raw, "worker_count").and_then(|value| usize::try_from(value).ok())
        })
        .unwrap_or(0)
}

fn resolve_terminal_dimensions(env: Option<&BTreeMap<OsString, OsString>>) -> (usize, usize) {
    let columns = read_env_usize("COLUMNS", env).unwrap_or(DEFAULT_LAYOUT_COLUMNS);
    let rows = read_env_usize("LINES", env).unwrap_or(DEFAULT_LAYOUT_ROWS);
    (columns.max(88), rows.max(24))
}

fn read_env_usize(key: &str, env: Option<&BTreeMap<OsString, OsString>>) -> Option<usize> {
    env.and_then(|map| map.get(&OsString::from(key)).cloned())
        .map(|value| value.to_string_lossy().to_string())
        .or_else(|| std::env::var(key).ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
}

fn read_optional(path: impl AsRef<Path>) -> io::Result<Option<String>> {
    match fs::read_to_string(path.as_ref()) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_text(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn extract_json_u64(raw: &str, key: &str) -> Option<u64> {
    extract_json_value(raw, key)?.trim().parse::<u64>().ok()
}

fn extract_hud_mode_from_layout(raw: &str) -> Option<String> {
    extract_json_value(raw, "hud")
        .as_deref()
        .and_then(|value| extract_json_string_field(value, "mode"))
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

fn escape_json_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::{HudModeOverride, sync_prompt_layout_from_state};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("omx-rust-team-layout-{label}-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn syncs_layout_state_and_team_mode_state_for_prompt_teams() {
        let cwd = temp_dir("sync");
        let team_root = cwd.join(".omx/state/team/prompty");
        let state_root = cwd.join(".omx/state");
        fs::create_dir_all(&team_root).expect("team root");
        fs::write(
            team_root.join("config.json"),
            r#"{"name":"prompty","worker_launch_mode":"prompt","runtime_session_id":"prompt-prompty","tmux_session":null,"workers":[{"name":"worker-1"},{"name":"worker-2"}]}"#,
        )
        .expect("config");
        fs::write(
            team_root.join("phase.json"),
            r#"{"current_phase":"team-exec"}"#,
        )
        .expect("phase");

        let env = BTreeMap::from([
            (OsString::from("COLUMNS"), OsString::from("160")),
            (OsString::from("LINES"), OsString::from("44")),
        ]);
        let snapshot = sync_prompt_layout_from_state(
            &team_root,
            &state_root,
            "spawn",
            HudModeOverride::Inline,
            Some(&env),
        )
        .expect("sync")
        .expect("snapshot");
        assert_eq!(snapshot.worker_count, 2);
        assert_eq!(snapshot.columns, 160);
        assert_eq!(snapshot.rows, 44);
        assert_eq!(snapshot.hud_mode, "inline");

        let layout = fs::read_to_string(team_root.join("layout-state.json")).expect("layout");
        assert!(layout.contains("\"mode\": \"native_equivalent\""));
        assert!(layout.contains("\"last_reason\": \"spawn\""));
        let team_state =
            fs::read_to_string(state_root.join("team-state.json")).expect("team-state");
        assert!(team_state.contains("\"layout_mode\": \"native_equivalent\""));
        assert!(team_state.contains("\"hud_mode\": \"inline\""));
    }
}
