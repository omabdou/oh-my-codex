# Rust Process-Bridge Design Note

## Purpose

This note defines the Rust-side subprocess boundary needed to preserve current OMX CLI behavior before command-family cutover. It covers the launch, team/HUD, and provider-advisor execution paths that are currently implemented in:

- `src/cli/index.ts:1170-1379`
- `src/cli/ask.ts:149-215`
- `src/utils/platform-command.ts:1-169`
- `src/team/tmux-session.ts:680-724`
- `src/notifications/tmux-detector.ts:1-141`

The design target is behavior-first parity: Rust may reorganize internals, but it must preserve command assembly, environment propagation, stdio passthrough, and exit semantics expected by the current compatibility harness and team runtime.

## Scope

Covered subprocess families:

1. **Launch path** — launching Codex from `omx launch` / HUD-backed startup.
2. **Team/HUD path** — tmux/psmux probing, detached session bootstrap, pane commands, and fallback direct execution.
3. **Provider advisor path** — `omx ask` spawning `run-provider-advisor.js` and relaying output/exit status.
4. **Platform command probing** — Windows-specific resolution for `.exe`, `.cmd`, `.bat`, and `.ps1` shims.

Out of scope for this note:

- Full crate layout and command-family ownership matrix.
- MCP server transport design.
- Hook/event payload schemas beyond subprocess interaction requirements.

## Current Behavioral Contract

### 1. Launch path (`runCodex`)

Current TypeScript behavior:

- Computes a session-scoped model-instructions overlay file and injects related bypass args before launch.
- Computes `OMX_TEAM_WORKER_LAUNCH_ARGS` from leader flags and worker defaults.
- Optionally propagates `CODEX_HOME` and `OMX_NOTIFY_TEMP_CONTRACT`.
- If already inside tmux, starts a HUD watcher pane and runs Codex in the current pane.
- If not inside tmux, creates a detached tmux session, splits a HUD pane, registers resize/client-attached hooks, optionally enables mouse + WSL XT override, then attaches.
- If detached-session bootstrap fails, falls back to direct foreground Codex execution.

Rust must preserve all of the above ordering and fallback behavior.

### 2. Provider advisor path (`askCommand`)

Current TypeScript behavior:

- Resolves `scripts/run-provider-advisor.js` from package root unless overridden.
- Launches the advisor with `process.execPath` as the executable.
- Sets `ASK_ORIGINAL_TASK` in the child environment.
- Pipes child `stdout` to parent `stdout` unchanged.
- Pipes child `stderr` to parent `stderr` unchanged.
- If `spawnSync` returns an error, throws an `[ask] failed to launch advisor script: ...` error.
- If the child exits because of a signal, maps it to `128 + signal_number`.
- Sets `process.exitCode` for non-zero completion instead of rewriting output.

Rust must preserve byte-forwarding and exit semantics exactly for parity mode.

### 3. Platform command resolution

Current TypeScript behavior from `platform-command.ts`:

- Non-Windows: use the original command name directly.
- Windows: search `PATH` / `PATHEXT`, preserving priority for `.exe`, `.com`, `.ps1`, `.cmd`, `.bat`.
- `.cmd` / `.bat`: run via `cmd.exe /d /s /c <quoted command line>`.
- `.ps1`: run via PowerShell with `-NoLogo -NoProfile -ExecutionPolicy Bypass -File`.
- `.exe` / `.com`: execute directly.
- Spawn errors are classified as `missing`, `blocked`, or generic `error`.

Rust must preserve command selection because tmux/psmux and provider/tool probes rely on it, especially on native Windows.

## Rust Design

## A. Core abstraction: `ProcessBridge`

Create a small Rust boundary in `omx-core` (or equivalent shared crate) with two layers:

1. **`CommandSpec`** — declarative description of what should run.
2. **`ProcessBridge`** — executes a `CommandSpec`, captures or inherits stdio, and returns a normalized result.

Suggested model:

```rust
struct CommandSpec {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env_additions: BTreeMap<OsString, OsString>,
    env_removals: Vec<OsString>,
    stdio_mode: StdioMode,
    platform_resolution: PlatformResolution,
}

enum StdioMode {
    Inherit,
    Capture,
    Mixed { stdin: StdioPipe, stdout: StdioPipe, stderr: StdioPipe },
}

struct ProcessResult {
    status_code: Option<i32>,
    terminating_signal: Option<String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    spawn_error_kind: Option<SpawnErrorKind>,
}
```

Rules:

- `ProcessBridge` owns all spawn/exec logic.
- Higher-level modules build `CommandSpec` values only; they do not perform ad hoc platform wrapping.
- Windows command wrapping is centralized under the bridge.
- Exit-code mapping for signaled children is normalized in one place.

## B. Execution modes

Rust must support three execution modes matching current behavior:

### 1. Blocking foreground execution

Used for:

- direct Codex execution fallback
- provider advisor execution
- tmux probe commands like `tmux -V`

Behavior:

- If parity requires passthrough, use inherited stdio or immediate byte replay.
- Return a `ProcessResult` with either `status_code` or `terminating_signal`.
- Do not rewrite child output.

### 2. Captured probe execution

Used for:

- `tmux -V`
- `display-message` / pane-id capture
- helper probes where stdout is parsed by OMX

Behavior:

- Capture stdout/stderr as bytes.
- Preserve trailing newlines; call sites decide whether to trim.
- Surface spawn failure kind separately from non-zero exit status.

### 3. Detached bootstrap step execution

Used for:

- `tmux new-session`
- `tmux split-window`
- hook registration / delayed resize commands

Behavior:

- Execute steps sequentially.
- Annotate each step with a stable name for logging and rollback.
- If a step fails after session creation, invoke rollback steps best-effort and then execute the direct-launch fallback.

## C. Environment propagation contract

The Rust bridge must preserve the parent environment by default and then apply explicit overrides in-order.

Required inherited variables for launch/team parity:

- `PATH`, `PATHEXT`, `ComSpec`, shell-related vars
- `TMUX`, `TMUX_PANE`
- `WSL_DISTRO_NAME`, `WSL_INTEROP`
- `OMX_TEAM_WORKER_LAUNCH_ARGS`
- `OMX_NOTIFY_TEMP_CONTRACT`
- `CODEX_HOME`
- any session/model-instruction overlay vars already expected by Codex launch

Rules:

1. Start from `std::env::vars_os()`.
2. Apply explicit additions last.
3. Never silently drop unknown vars.
4. Preserve exact string payloads for JSON-valued env vars such as `OMX_NOTIFY_TEMP_CONTRACT`.
5. Use OS-string-safe APIs so Windows paths and shell fragments are not lossy.

## D. Stdio passthrough contract

### Launch / direct Codex fallback

- Child stdin/stdout/stderr should be inherited.
- Rust must not buffer or transform terminal output.
- Parent process exit behavior should remain tied to the child outcome.

### `omx ask`

To match current behavior most closely:

- Spawn the advisor with captured stdout/stderr.
- After completion, write captured `stdout` bytes directly to parent stdout, then `stderr` bytes to parent stderr.
- Preserve ordering semantics documented by the current Node implementation: stdout is flushed before stderr once the child exits.
- If exact interleaving becomes necessary later, add a streaming parity mode flag, but initial cutover should preserve the currently observed behavior.

### tmux helper commands

- Use capture mode.
- Parse stdout as UTF-8 only at the API boundary that already assumes text.
- Keep raw bytes available for diagnostics.

## E. Exit-code and signal mapping

Rust must normalize completion exactly as the Node implementation expects:

1. If the child exits normally, use its exit status.
2. If the child terminates by signal on Unix, map to `128 + signal_number`.
3. If spawn itself fails:
   - return a classified spawn error (`missing`, `blocked`, `error`)
   - let callers decide whether to warn, throw, or degrade.
4. For `omx ask`, non-zero completion sets the command exit code without rewriting child output.
5. For launch fallback, the parent should exit with the child status after post-launch cleanup.

On Windows, where Unix signals are limited, the bridge should return no signal number and rely on process status or spawn error classification.

## F. tmux / psmux bootstrap representation

Represent the current detached-session flow as explicit step lists, not ad hoc shell strings.

Suggested shape:

```rust
struct TmuxStep {
    name: &'static str,
    spec: CommandSpec,
    rollback: Option<CommandSpec>,
    parse: Option<StepParseKind>,
}
```

Required ordered phases:

1. `new-session`
2. `split-and-capture-hud-pane`
3. parse pane id + detect window index
4. optional `register-resize-hook`
5. optional `register-client-attached-reconcile`
6. optional `schedule-delayed-resize`
7. optional `reconcile-hud-resize`
8. optional `set-mouse`
9. optional `set-wsl-xt`
10. `attach-session`

Fallback rule:

- Any fatal failure in detached bootstrap must log the failure, run rollback if the session was created, and continue with direct Codex foreground launch.

## G. Shell command assembly for tmux panes

Current TS behavior generates a login-shell command string like:

- source `~/.zshrc` for zsh
- source `~/.bashrc` for bash
- run `exec <quoted command>` under `shell -lc '<inner>'`

Rust must preserve this behavior for tmux pane payloads because it affects PATH/tool resolution inside worker panes.

Rules:

1. Restrict shell binaries to the current allowlist equivalent; otherwise fall back to `/bin/sh`.
2. Preserve single-quote-safe escaping semantics.
3. Keep rc sourcing behavior for zsh and bash.
4. Generate pane payload strings in one utility shared by launch/team runtime.

## H. Error handling policy

Classify subprocess failures into:

- **missing dependency** — e.g. `tmux`/`psmux` absent
- **blocked/permission** — e.g. denied execution
- **non-zero child exit** — command ran but failed
- **unexpected bridge failure** — malformed config, encoding, or internal error

Caller behavior must stay consistent with TS:

- native Windows tmux probe warns and degrades without aborting launch
- detached tmux bootstrap degrades to direct launch
- `omx ask` throws only on spawn failure, not merely on non-zero child exit

## Source-to-Rust ownership mapping

- `src/utils/platform-command.ts` → Rust `process_bridge::platform`
- `src/cli/ask.ts` child-launch logic → Rust `commands::ask::advisor_bridge`
- `src/cli/index.ts` launch/HUD subprocess logic → Rust `commands::launch::bootstrap`
- `src/team/tmux-session.ts` tmux availability + WSL-sensitive behavior → Rust `team::tmux_runtime`
- `src/notifications/tmux-detector.ts` pane utilities → Rust `team::tmux_pane_io`

## Acceptance checks

The process bridge is ready for cutover only when all are true:

1. `omx ask` parity tests pass for stdout, stderr, and exit-code passthrough.
2. Windows command-resolution tests cover `.exe`, `.cmd`, `.bat`, and `.ps1` routing.
3. Team/tmux tests prove detached bootstrap step ordering and direct-launch fallback.
4. Native Windows launch still degrades cleanly when tmux/psmux is absent.
5. WSL-specific XT override behavior remains gated to WSL flows only.
6. No higher-level Rust command code performs direct platform-specific wrapping outside the bridge.

## Open decisions to keep explicit

1. **psmux naming and probe path** — decide whether Rust treats `psmux` as a tmux-compatible binary alias or a separate capability provider with its own command builder.
2. **streaming vs post-exit replay for `omx ask`** — current Node behavior is effectively replay-after-exit; keep that for initial parity unless new fixtures require true interleaving.
3. **signal portability contract** — document how much Unix signal fidelity is required on Windows-native builds beyond ordinary exit-status preservation.
