# Platform Capability Matrix for Rust CLI Refactor

## Purpose

This matrix defines supported execution expectations for the Rust OMX CLI across Linux, macOS, Windows native, and WSL. It is the design-time acceptance contract required by the PRD/test spec before team-runtime and launch-path cutover.

## Capability dimensions

The matrix tracks these capabilities because they directly affect current OMX behavior:

- CLI launch without tmux
- tmux-backed launch/HUD
- team runtime orchestration
- native shell/profile loading for tmux panes
- provider-advisor subprocess execution
- degraded-mode behavior when tmux is unavailable
- platform-specific executable resolution (`.exe`, `.cmd`, `.bat`, `.ps1`)

## Owners

- **Process bridge owner:** Rust core/process layer
- **Team runtime owner:** Rust team/tmux runtime layer
- **CLI launch owner:** Rust launch command layer
- **Verification owner:** compatibility harness + targeted platform smoke tests

## Matrix

| Platform/runtime | tmux/psmux expectation | Launch + HUD expectation | Team runtime expectation | Provider/advisor expectation | Degraded mode | Acceptance checks |
|---|---|---|---|---|---|---|
| Linux native | `tmux` expected and commonly available | Full tmux-backed launch supported; detached session + HUD + mouse mode supported | Full team runtime supported | Direct subprocess execution via native binary resolution | If `tmux` missing, direct launch fallback where current flow allows; team creation must fail with clear error | `tmux -V` probe passes; detached session bootstrap test passes; `ask` parity suite passes |
| macOS native | `tmux` expected if using HUD/team | Full tmux-backed launch supported; shell rc sourcing must preserve user PATH/toolchain | Full team runtime supported | Direct subprocess execution via native binary resolution | Same as Linux: direct launch may degrade; team runtime remains explicit failure if tmux absent | launch smoke in tmux session; verify `.zshrc`/`.bashrc` sourced in pane command; `ask` parity suite passes |
| Windows native | `psmux` or tmux-compatible command may exist; absence is common and must not hard-block plain launch | No hard block on launch. If tmux probe fails, warn and continue without tmux/HUD | Team runtime only supported when tmux-compatible backend is actually available | Must support Windows command resolution and wrapper execution (`cmd.exe`, PowerShell, direct `.exe`) | Warning + no-HUD fallback for launch; unsupported combinations fail clearly for team runtime | `tmux -V` missing -> warning text/degraded launch; `.cmd`/`.bat`/`.ps1` resolution tests pass; `ask` passthrough passes |
| WSL | Linux-style `tmux` expected inside WSL environment | Full tmux-backed launch supported; WSL XT terminal override applied only in WSL path when mouse enabled | Full team runtime supported when tmux available | Direct subprocess execution in WSL environment | If tmux missing, direct launch fallback where allowed; no native-Windows warning path | WSL env detection test passes; `terminal-overrides ,xterm*:XT` gate is WSL-only; detached tmux bootstrap passes |

## Detailed requirements by platform

## Linux native

Required support:

- `tmux -V` probe works through the generic process bridge.
- Detached-session launch path works exactly as current TS flow.
- Team runtime requires tmux and should not silently downgrade into a non-team emulation.
- `omx ask` uses direct subprocess execution with inherited environment.

Required verification:

- compat harness for `help`, `version`, and `ask`
- at least one launch smoke inside tmux
- one team-runtime smoke creating panes/workers

## macOS native

Required support:

- Same feature set as Linux.
- tmux pane command builder must source `~/.zshrc` or `~/.bashrc` consistently because macOS users often rely on shell-managed PATH setup.

Required verification:

- compat harness for `help`, `version`, and `ask`
- pane payload test verifying selected shell wrapper shape
- tmux-backed launch smoke

## Windows native

Required support:

- Plain CLI commands must run without tmux.
- Launch path must probe for tmux/psmux using Windows-aware resolution.
- Missing tmux/psmux must produce a warning and continue without HUD.
- Team runtime must not claim support unless a tmux-compatible backend is actually available.
- Command bridge must correctly launch:
  - `.exe` / `.com` directly
  - `.cmd` / `.bat` through `cmd.exe /d /s /c`
  - `.ps1` through PowerShell `-NoLogo -NoProfile -ExecutionPolicy Bypass -File`

Required verification:

- unit tests for PATHEXT search order and wrapper selection
- launch smoke without tmux installed
- launch/team smoke with tmux-compatible backend installed (when available in CI/manual matrix)
- `ask` passthrough parity using Node script target

## WSL

Required support:

- WSL detection must rely on env markers and `/proc/version` fallback equivalent.
- WSL should be treated as tmux-capable Linux when tmux is installed.
- Mouse/XT override must only be applied in WSL-specific flow.
- Native-Windows degraded warnings must not appear inside WSL.

Required verification:

- unit tests for WSL detection logic
- launch bootstrap smoke inside WSL with tmux
- regression tests proving missing tmux in WSL degrades without throwing where current TS code already tolerates it

## Unsupported / constrained combinations

| Combination | Status | Required behavior |
|---|---|---|
| Native Windows + no tmux-compatible backend + team runtime | Unsupported | fail team command clearly; do not fake pane-based team runtime |
| Native Windows + missing tmux-compatible backend + HUD launch | Degraded | warn, skip HUD/tmux setup, continue direct launch |
| Any platform + missing provider-advisor script | Unsupported/config error | fail `omx ask` with explicit advisor-script-not-found error |
| Any platform + blocked executable permissions | Error | classify as blocked, surface actionable error/warning depending on caller |

## Release gate checklist

The Rust cutover for launch/team/process features is blocked until the following are all green:

1. **Linux:** compat harness + tmux launch smoke + team smoke.
2. **macOS:** compat harness + tmux launch smoke + shell-wrapper verification.
3. **Windows native:** command-resolution unit suite + no-tmux degraded launch smoke + tmux-compatible backend smoke if available.
4. **WSL:** WSL detection regression suite + XT override gate verification + tmux launch smoke.
5. Every row above has a named owner during migration and explicit PASS/FAIL evidence in the migration tracker.

## Per-capability acceptance declarations

### Launch command family

- Rust may become default only after Linux/macOS/WSL tmux launch behavior matches approved parity fixtures.
- Native Windows must preserve the current warning-and-continue path when tmux is absent.

### Team command family

- Rust may become default only after tmux-backed lifecycle smoke tests pass on Linux/macOS/WSL and native Windows support is explicitly gated by actual backend availability.

### Ask/provider command family

- Rust may become default only after `ask` passthrough fixtures are byte-exact and exit-code mapping matches Node baseline.

## Notes for migration tracking

- This matrix is intentionally capability-based rather than crate-based so it stays valid as Rust module boundaries evolve.
- If CI cannot cover every OS/runtime combination, manual smoke evidence must still be attached before default cutover.
