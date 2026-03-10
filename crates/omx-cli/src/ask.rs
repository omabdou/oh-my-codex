use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

pub const ASK_USAGE: &str = concat!(
    "Usage: omx ask <claude|gemini> <question or task>\n",
    "   or: omx ask <claude|gemini> -p \"<prompt>\"\n",
    "   or: omx ask claude --print \"<prompt>\"\n",
    "   or: omx ask gemini --prompt \"<prompt>\"\n",
    "   or: omx ask <claude|gemini> --agent-prompt <role> \"<prompt>\"\n",
    "   or: omx ask <claude|gemini> --agent-prompt=<role> --prompt \"<prompt>\"",
);

const ASK_ADVISOR_SCRIPT_ENV: &str = "OMX_ASK_ADVISOR_SCRIPT";
const ASK_ORIGINAL_TASK_ENV: &str = "OMX_ASK_ORIGINAL_TASK";
const OMX_LEADER_NODE_PATH_ENV: &str = "OMX_LEADER_NODE_PATH";
const NPM_NODE_EXECPATH_ENV: &str = "npm_node_execpath";
const ASK_AGENT_PROMPT_FLAG: &str = "--agent-prompt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskProvider {
    Claude,
    Gemini,
}

impl AskProvider {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAskArgs {
    pub provider: AskProvider,
    pub prompt: String,
    pub agent_prompt_role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskExecution {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRuntime {
    pub package_root: PathBuf,
    pub advisor_script_path: PathBuf,
    pub node_program: OsString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskError(String);

impl AskError {
    pub(crate) fn usage(reason: impl Into<String>) -> Self {
        Self(format!("{}\n{}", reason.into(), ASK_USAGE))
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for AskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AskError {}

#[allow(clippy::missing_errors_doc)]
pub fn parse_ask_args<I, S>(args: I) -> Result<ParsedAskArgs, AskError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect::<Vec<_>>();

    let Some((provider_raw, rest)) = args.split_first() else {
        return Err(AskError::usage(
            "Invalid provider \"\". Expected one of: claude, gemini.",
        ));
    };

    let Some(provider) = AskProvider::parse(provider_raw) else {
        return Err(AskError::usage(format!(
            "Invalid provider \"{provider_raw}\". Expected one of: claude, gemini.",
        )));
    };

    if rest.is_empty() {
        return Err(AskError::usage("Missing prompt text."));
    }

    let mut agent_prompt_role = None;
    let mut prompt = String::new();
    let mut index = 0;

    while index < rest.len() {
        let token = &rest[index];
        if token == ASK_AGENT_PROMPT_FLAG {
            let Some(role) = rest.get(index + 1).map(|value| value.trim()) else {
                return Err(AskError::usage("Missing role after --agent-prompt."));
            };
            if role.is_empty() || role.starts_with('-') {
                return Err(AskError::usage("Missing role after --agent-prompt."));
            }
            agent_prompt_role = Some(role.to_owned());
            index += 2;
            continue;
        }

        if let Some(role) = token.strip_prefix("--agent-prompt=") {
            let role = role.trim();
            if role.is_empty() {
                return Err(AskError::usage("Missing role after --agent-prompt=."));
            }
            agent_prompt_role = Some(role.to_owned());
            index += 1;
            continue;
        }

        if matches!(token.as_str(), "-p" | "--print" | "--prompt") {
            let joined = rest[index + 1..].join(" ");
            joined.trim().clone_into(&mut prompt);
            break;
        }

        if let Some(inline_prompt) = token
            .strip_prefix("-p=")
            .or_else(|| token.strip_prefix("--print="))
            .or_else(|| token.strip_prefix("--prompt="))
        {
            let remainder = rest[index + 1..].join(" ");
            let combined = format!("{} {}", inline_prompt.trim(), remainder.trim());
            combined.trim().clone_into(&mut prompt);
            break;
        }

        if !prompt.is_empty() {
            prompt.push(' ');
        }
        prompt.push_str(token);
        index += 1;
    }

    if prompt.trim().is_empty() {
        return Err(AskError::usage("Missing prompt text."));
    }

    Ok(ParsedAskArgs {
        provider,
        prompt,
        agent_prompt_role,
    })
}

#[must_use]
pub fn resolve_ask_advisor_script_path(
    package_root: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> PathBuf {
    match env.get(OsStr::new(ASK_ADVISOR_SCRIPT_ENV)) {
        Some(override_path) if !override_path.is_empty() => {
            let override_path = PathBuf::from(override_path);
            if override_path.is_absolute() {
                override_path
            } else {
                package_root.join(override_path)
            }
        }
        _ => package_root.join("scripts").join("run-provider-advisor.js"),
    }
}

#[allow(clippy::missing_errors_doc)]
pub fn resolve_ask_runtime(
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<AskRuntime, AskError> {
    let package_root = resolve_package_root(cwd)?;
    let advisor_script_path = resolve_ask_advisor_script_path(&package_root, env);
    if !advisor_script_path.is_file() {
        return Err(AskError::runtime(format!(
            "[ask] advisor script not found: {}",
            advisor_script_path.display()
        )));
    }

    Ok(AskRuntime {
        package_root,
        advisor_script_path,
        node_program: resolve_node_program(env),
    })
}

#[allow(clippy::missing_errors_doc)]
pub fn run_ask(
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<AskExecution, AskError> {
    let parsed = parse_ask_args(args.iter().map(String::as_str))?;
    let runtime = resolve_ask_runtime(cwd, env)?;

    let mut command = Command::new(&runtime.node_program);
    command
        .current_dir(cwd)
        .arg(&runtime.advisor_script_path)
        .arg(parsed.provider.as_str())
        .arg(&parsed.prompt)
        .envs(env.iter())
        .env(ASK_ORIGINAL_TASK_ENV, &parsed.prompt);

    let output = command.output().map_err(|error| {
        AskError::runtime(format!("[ask] failed to launch advisor script: {error}"))
    })?;

    Ok(AskExecution {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: exit_code_from_status(output.status),
    })
}

fn resolve_node_program(env: &BTreeMap<OsString, OsString>) -> OsString {
    env.get(OsStr::new(OMX_LEADER_NODE_PATH_ENV))
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| {
            env.get(OsStr::new(NPM_NODE_EXECPATH_ENV))
                .filter(|value| !value.is_empty())
                .cloned()
        })
        .unwrap_or_else(|| OsString::from("node"))
}

fn resolve_package_root(cwd: &Path) -> Result<PathBuf, AskError> {
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(found) = find_package_root(current_exe.as_path())
    {
        return Ok(found);
    }

    if let Some(found) = find_package_root(cwd) {
        return Ok(found);
    }

    Err(AskError::runtime(format!(
        "[ask] could not resolve package root from {}",
        cwd.display()
    )))
}

fn find_package_root(start: &Path) -> Option<PathBuf> {
    let anchor = if start.is_dir() {
        start
    } else {
        start.parent()?
    };
    anchor
        .ancestors()
        .find(|candidate| candidate.join("package.json").is_file())
        .map(Path::to_path_buf)
}

fn exit_code_from_status(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| signal_exit_code(status))
}

#[cfg(unix)]
fn signal_exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    status.signal().map_or(1, |signal| 128 + signal)
}

#[cfg(not(unix))]
fn signal_exit_code(_: &ExitStatus) -> i32 {
    1
}
