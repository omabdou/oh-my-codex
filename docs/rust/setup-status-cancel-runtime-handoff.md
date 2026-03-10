# Setup → Status/Cancel/Runtime Rust Migration Handoff

## Purpose

This note hands off the current Rust `setup` integration work into the next migration slices for `status`, `cancel`, and session/team runtime behavior. It focuses on exact TypeScript source anchors, likely Rust destination files, parity dependencies that must stay aligned across commands, and the main hotspots that can create hard-to-review regressions.

Scope is intentionally limited to `docs/rust/**`.

## Handoff summary

The safest next sequence after setup scope/dry-run parity is:

1. finish shared setup/doctor scope resolution
2. port `status` over the same session-scoped state model
3. port `cancel` on top of the same mode-discovery + state-mutation rules
4. wire later team/runtime status surfaces only after tmux/process helpers and session-state semantics are stable

The key migration rule is: **setup, doctor, status, and cancel must all agree on where state lives and which session scope is active**. If that contract drifts, the CLI will appear to work while reading and mutating different state files.

## Existing parity dependencies that status/cancel must inherit

### 1. Setup-scoped path resolution

The setup migration establishes which root should own OMX artifacts:

- project scope → repo-local `.omx/**`, `.codex/**`, `AGENTS.md`
- user scope → user home / `CODEX_HOME`
- persisted scope → `.omx/setup-scope.json`

Status/cancel do not directly install files, but they rely on setup parity because users expect:

- `omx setup --scope project` to prepare the same repo-local state that `omx status` later reads
- `omx doctor` to inspect the same roots that `omx cancel` mutates
- persisted scope selection to remain stable across all command families

### 2. Session-scoped state discovery

Current TypeScript runtime behavior is session-aware rather than only root-state aware.

Primary source anchors:

- `src/cli/index.ts:517` — status line output shape (`ACTIVE` vs inactive + phase)
- `src/cli/index.ts:1800-1853` — cancel-mode mutation flow and `Cancelled: <mode>` output
- `src/state/mode-state-context.ts:1-35` — runtime context enrichment (`TMUX_PANE`, timestamps)
- `src/cli/__tests__/session-scoped-runtime.test.ts:20-121` — low-flake contract for session-scoped status/cancel parity
- `src/mcp/__tests__/state-paths.test.ts:100-159` — state path expectations for root and session-scoped files

Rust status/cancel work must reuse the same state-root resolution discipline already needed by setup/doctor.

### 3. Process/tmux helper readiness

The Rust runtime layer already has reusable process helpers, and this worktree now includes tmux command builders under `crates/omx-process/**` for later runtime migration.

Relevant Rust-side anchors:

- `crates/omx-process/src/process_bridge.rs` — subprocess execution + error classification
- `crates/omx-process/src/process_plan.rs` — ordered step execution / rollback model
- `crates/omx-process/src/tmux_shell.rs` — pane shell payload construction
- `crates/omx-process/src/tmux_commands.rs` — tmux probe/capture/send-keys/kill builders

These should stay below the future Rust command/runtime layers; `status`/`cancel` should consume state logic first and only then call tmux/runtime helpers where parity requires it.

## Exact TypeScript behavior that must hand off cleanly

### Setup → status handoff

The setup migration should leave these later assumptions true:

- `src/cli/setup.ts:308-339` persists scope consistently
- `src/cli/doctor.ts:47-90` resolves paths from the same persisted scope
- `src/cli/index.ts:517` can discover state files in the same root without bespoke path rules

### Setup → cancel handoff

Cancel depends on the same state-discovery layer plus consistent session selection:

- `src/cli/index.ts:1800-1849` mutates only the current session-scoped active modes by default
- `src/cli/__tests__/session-scoped-runtime.test.ts:87-121` proves unrelated sessions must not be mutated
- linked mode rules such as Ralph → Ultrawork cancellation must remain explicit, not inferred from filename scanning alone

### Setup → runtime/team handoff

Later team/runtime migration depends on setup and session-path parity because runtime state spans:

- `.omx/state/team-state.json`
- `.omx/state/sessions/<session_id>/*-state.json`
- `.omx/state/team/<team-name>/**`

Relevant anchors:

- `src/team/state.ts`
- `src/team/state/monitor.ts`
- `src/team/runtime.ts`
- `src/team/phase-controller.ts`
- `src/team/runtime-cli.ts`
- `src/team/tmux-session.ts`

Do not port these higher-runtime layers before the setup/scope/session contract is stable.

## Likely Rust destination files

These are the most likely files to change in the next migration slices.

### Setup / shared path helpers

- `crates/omx-cli/src/setup.rs` *(new or expanded)*
- `crates/omx-cli/src/doctor.rs`
- `crates/omx-cli/src/lib.rs`
- `crates/omx-cli/src/main.rs`
- `crates/omx-cli/src/install_paths.rs` *(recommended shared helper once parity is stable)*
- `crates/omx-cli/src/session_state.rs` *(recommended shared helper for current-session state discovery)*

### Status / cancel command wiring

- `crates/omx-cli/src/status.rs` *(recommended new module)*
- `crates/omx-cli/src/cancel.rs` *(recommended new module)*
- `crates/omx-cli/src/lib.rs` — subcommand dispatch/help text
- `crates/omx-cli/src/main.rs` — exit-code plumbing if split remains thin

### Team/runtime support later

- `crates/omx-cli/src/team.rs` or future runtime modules
- `crates/omx-process/src/process_bridge.rs`
- `crates/omx-process/src/process_plan.rs`
- `crates/omx-process/src/tmux_commands.rs`
- `crates/omx-process/src/tmux_shell.rs`

## Recommended Rust module boundaries

### Shared helper boundary to introduce before status/cancel port

Recommended minimal shared helpers:

1. `resolve_setup_scope(...)`
   - owns requested scope, persisted scope, legacy migration, and dry-run persistence rules
2. `resolve_state_root(...)`
   - returns the canonical `.omx/state` root for the selected install scope
3. `read_current_session_id(...)`
   - reads `.omx/state/session.json` when present
4. `list_mode_state_files(...)`
   - enumerates root + session-scoped state files in the same order as TS behavior
5. `mutate_mode_state(...)`
   - applies active/current_phase/completed_at updates consistently for cancel flows

These should be shared by setup, doctor, status, and cancel instead of copied per command.

### Suggested status port boundary

Recommended module responsibilities for `crates/omx-cli/src/status.rs`:

- discover active session scope
- load root + session-scoped mode states
- render one line per mode with TS-compatible output
- keep status read-only
- keep team/runtime deep inspection out of scope for the initial port

### Suggested cancel port boundary

Recommended module responsibilities for `crates/omx-cli/src/cancel.rs`:

- discover current session scope
- locate cancellable active modes for the current session/root context
- apply explicit linked-mode rules (Ralph → Ultrawork; team-related flows as parity requires)
- persist `active=false`, `current_phase=cancelled`, `completed_at=<iso>`
- print one `Cancelled: <mode>` line per affected mode

## Parity dependencies to lock first

Before Rust `status`/`cancel` implementation begins, the following should be locked by tests or explicit fixtures:

### Setup/doctor parity prerequisites

- `src/cli/__tests__/setup-scope.test.ts:40-223`
- `src/cli/__tests__/doctor-warning-copy.test.ts:29-61`
- `src/cli/__tests__/doctor-team.test.ts:29-225`

### Status/cancel parity prerequisites

- `src/cli/__tests__/session-scoped-runtime.test.ts:20-121`
- `src/state/__tests__/mode-state-context.test.ts`
- `src/mcp/__tests__/state-paths.test.ts:100-159`

### Runtime follow-on prerequisites

- `docs/rust/process-bridge-design.md`
- `docs/rust/platform-capability-matrix.md`
- `crates/omx-process` unit coverage for tmux/process helpers

## Risk hotspots

### 1. Scope root drift across commands

If setup writes project-scoped files but status/cancel read user-scoped state roots, the CLI will look healthy while mutating the wrong location.

Highest-risk files when this happens:

- `crates/omx-cli/src/setup.rs`
- `crates/omx-cli/src/doctor.rs`
- future `crates/omx-cli/src/status.rs`
- future `crates/omx-cli/src/cancel.rs`

**Mitigation:** one shared path/scope resolver used everywhere.

### 2. Session-scoped state drift

TypeScript status/cancel semantics are not “all states everywhere”; they are tied to the current session unless the command explicitly operates globally.

Regression shape:

- status omits session-scoped active modes
- cancel mutates root state but ignores session state
- cancel mutates unrelated sessions

**Mitigation:** port `src/cli/__tests__/session-scoped-runtime.test.ts` semantics before broad runtime migration.

### 3. Linked-mode cancellation rules becoming implicit

Ralph-linked Ultrawork cancellation is explicit in TS behavior. A generic “cancel all active states” Rust port would be wrong.

Relevant anchor:

- `src/cli/__tests__/session-scoped-runtime.test.ts:48-85`

**Mitigation:** encode linked-mode transitions directly in the cancel module and test them independently.

### 4. Team runtime overreach too early

It is tempting to use the new Rust tmux/process helpers immediately for full team-runtime porting. That would widen scope before state semantics are stable.

**Mitigation:** keep the next port focused on read/write state parity first; treat tmux/process helpers as enabling infrastructure, not proof that runtime parity is ready.

### 5. Output-shape drift

Status/cancel are user-facing and script-observed commands. Small wording drift can break parity fixtures or downstream automation.

Critical output anchors:

- `team: ACTIVE` from `status`
- `Cancelled: team`
- `Cancelled: ralph`
- `Cancelled: ultrawork`

**Mitigation:** lock output with targeted Rust tests before refactoring helper boundaries.

## Recommended execution order after current setup slice

### Phase 1 — finish setup scope contract

- land setup scope parsing/dry-run/persistence parity
- share persisted scope logic with doctor
- avoid status/cancel wiring until the shared root resolver exists

### Phase 2 — port status only

- implement read-only mode discovery
- cover current session-scoped state visibility
- preserve TS output wording

### Phase 3 — port cancel on top of status discovery

- reuse the same state enumeration rules
- implement explicit linked-mode cancellation
- prove unrelated sessions are untouched

### Phase 4 — widen into team/runtime migration

- only after state-root/session rules are green
- consume `omx-process` helpers for tmux/process steps as needed
- verify detached runtime/state lifecycles against the platform matrix and process-bridge design notes

## Merge checklist for the next owner

- [ ] Shared Rust scope/path resolution is used by setup + doctor
- [ ] A single Rust session-state helper determines current session id
- [ ] Rust status reads both root and current-session mode states where TS does
- [ ] Rust cancel mutates only the intended current-session/root states
- [ ] Rust cancel preserves explicit linked-mode behavior (Ralph → Ultrawork)
- [ ] Rust cancel does not mutate unrelated sessions
- [ ] Status/cancel output matches current TS wording for the low-flake cases
- [ ] Team/runtime migration is deferred until session-state parity is green
- [ ] New runtime/tmux integration reuses `crates/omx-process/**` helpers instead of ad hoc subprocess logic

## Immediate follow-up recommendation

The next implementation PR after setup scope parity should be a narrow **status-first** Rust port, with tests derived from `src/cli/__tests__/session-scoped-runtime.test.ts`, followed by a separate **cancel** PR that reuses the same session-state discovery layer.
