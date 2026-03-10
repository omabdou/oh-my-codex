pub mod ask;
pub mod doctor;
pub mod reasoning;
pub mod setup;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

pub const BINARY_NAME: &str = "omx";
pub const HELP_OUTPUT: &str = r#"
oh-my-codex (omx) - Multi-agent orchestration for Codex CLI

Usage:
  omx           Launch Codex CLI (HUD auto-attaches only when already inside tmux)
  omx setup     Install skills, prompts, MCP servers, and AGENTS.md
  omx uninstall Remove OMX configuration and clean up installed artifacts
  omx doctor    Check installation health
  omx doctor --team  Check team/swarm runtime health diagnostics
  omx ask       Ask local provider CLI (claude|gemini) and write artifact output
  omx session   Search prior local session transcripts and history artifacts
  omx agents-init [path]
                Bootstrap lightweight AGENTS.md files for a repo/subtree
  omx deepinit [path]
                Alias for agents-init (lightweight AGENTS bootstrap only)
  omx team      Spawn parallel worker panes in tmux and bootstrap inbox/task state
  omx ralph     Launch Codex with ralph persistence mode active
  omx version   Show version information
  omx tmux-hook Manage tmux prompt injection workaround (init|status|validate|test)
  omx hooks     Manage hook plugins (init|status|validate|test)
  omx hud       Show HUD statusline (--watch, --json, --preset=NAME)
  omx help      Show this help message
  omx status    Show active modes and state
  omx cancel    Cancel active execution modes
  omx reasoning Show or set model reasoning effort (low|medium|high|xhigh)

Options:
  --yolo        Launch Codex in yolo mode (shorthand for: omx launch --yolo)
  --high        Launch Codex with high reasoning effort
                (shorthand for: -c model_reasoning_effort="high")
  --xhigh       Launch Codex with xhigh reasoning effort
                (shorthand for: -c model_reasoning_effort="xhigh")
  --madmax      DANGEROUS: bypass Codex approvals and sandbox
                (alias for --dangerously-bypass-approvals-and-sandbox)
  --spark       Use the Codex spark model (~1.3x faster) for team workers only
                Workers get the configured low-complexity team model; leader model unchanged
  --madmax-spark  spark model for workers + bypass approvals for leader and workers
                (shorthand for: --spark --madmax)
  --notify-temp  Enable temporary notification routing for this run/session only
  --discord      Select Discord provider for temporary notification mode
  --slack        Select Slack provider for temporary notification mode
  --telegram     Select Telegram provider for temporary notification mode
  --custom <name>
                Select custom/OpenClaw gateway name for temporary notification mode
  -w, --worktree[=<name>]
                Launch Codex in a git worktree (detached when no name is given)
  --force       Force reinstall (overwrite existing files)
  --dry-run     Show what would be done without doing it
  --keep-config Skip config.toml cleanup during uninstall
  --purge       Remove .omx/ cache directory during uninstall
  --verbose     Show detailed output
  --scope       Setup scope for "omx setup" only:
                user | project

"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    Help,
    Version,
    Ask(Vec<String>),
    Reasoning(Vec<String>),
    Doctor(Vec<String>),
    Setup(Vec<String>),
    Unsupported,
}

#[must_use]
pub fn parse_args<I, S>(args: I) -> CliAction
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let values = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect::<Vec<_>>();

    match values.get(1).map(String::as_str) {
        None | Some("help" | "--help" | "-h") => CliAction::Help,
        Some("version" | "--version" | "-v") => CliAction::Version,
        Some("ask") => CliAction::Ask(values.into_iter().skip(2).collect()),
        Some("reasoning") => CliAction::Reasoning(values.into_iter().skip(2).collect()),
        Some("doctor") => CliAction::Doctor(values.into_iter().skip(2).collect()),
        Some("setup") => CliAction::Setup(values.into_iter().skip(2).collect()),
        Some(_) => CliAction::Unsupported,
    }
}

#[must_use]
pub fn help_output() -> &'static str {
    HELP_OUTPUT
}

#[must_use]
pub fn version_output() -> String {
    format!(
        "oh-my-codex v{}\nNode.js {}\nPlatform: {} {}\n",
        env!("CARGO_PKG_VERSION"),
        detect_node_version().unwrap_or_else(|| "v25.1.0".to_string()),
        std::env::consts::OS,
        display_arch(),
    )
}

#[allow(clippy::missing_errors_doc)]
pub fn run_ask_command(args: &[String]) -> Result<ask::AskExecution, ask::AskError> {
    let cwd = std::env::current_dir().map_err(|error| {
        ask::AskError::runtime(format!(
            "[ask] failed to resolve current directory: {error}"
        ))
    })?;
    let env = std::env::vars_os().collect::<BTreeMap<OsString, OsString>>();
    ask::run_ask(args, Path::new(&cwd), &env)
}

fn display_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        other => other,
    }
}

fn detect_node_version() -> Option<String> {
    use std::process::Command;

    let output = Command::new("node").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8(output.stdout).ok()?;
    let trimmed = version.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{BINARY_NAME, CliAction, help_output, parse_args, version_output};

    fn normalize_version_output(text: &str) -> String {
        text.replace(
            text.lines()
                .find(|line| line.starts_with("Node.js "))
                .unwrap_or("Node.js unknown"),
            "Node.js <NODE_VERSION>",
        )
    }

    #[test]
    fn exposes_expected_binary_name() {
        assert_eq!(BINARY_NAME, "omx");
    }

    #[test]
    fn matches_top_level_help_exactly() {
        assert_eq!(
            help_output(),
            include_str!("../../../src/compat/fixtures/help.stdout.txt")
        );
    }

    #[test]
    fn parses_help_variants() {
        assert_eq!(parse_args(["omx"]), CliAction::Help);
        assert_eq!(parse_args(["omx", "help"]), CliAction::Help);
        assert_eq!(parse_args(["omx", "--help"]), CliAction::Help);
        assert_eq!(parse_args(["omx", "-h"]), CliAction::Help);
    }

    #[test]
    fn parses_version_variants() {
        assert_eq!(parse_args(["omx", "version"]), CliAction::Version);
        assert_eq!(parse_args(["omx", "--version"]), CliAction::Version);
        assert_eq!(parse_args(["omx", "-v"]), CliAction::Version);
    }

    #[test]
    fn parses_ask_subcommand_with_passthrough_args() {
        assert_eq!(
            parse_args(["omx", "ask", "claude", "review", "this"]),
            CliAction::Ask(vec!["claude".into(), "review".into(), "this".into()])
        );
    }

    #[test]
    fn parses_reasoning_subcommand_with_passthrough_args() {
        assert_eq!(
            parse_args(["omx", "reasoning", "high"]),
            CliAction::Reasoning(vec!["high".into()])
        );
    }

    #[test]
    fn parses_doctor_subcommand_with_passthrough_args() {
        assert_eq!(
            parse_args(["omx", "doctor", "--team"]),
            CliAction::Doctor(vec!["--team".into()])
        );
    }

    #[test]
    fn parses_setup_subcommand_with_passthrough_args() {
        assert_eq!(
            parse_args(["omx", "setup", "--scope", "project"]),
            CliAction::Setup(vec!["--scope".into(), "project".into()])
        );
    }

    #[test]
    fn marks_other_commands_as_unsupported() {
        assert_eq!(parse_args(["omx", "team"]), CliAction::Unsupported);
    }

    #[test]
    fn matches_version_fixture_in_current_environment() {
        assert_eq!(
            normalize_version_output(&version_output()),
            include_str!("../../../src/compat/fixtures/version.stdout.txt")
        );
    }
}
