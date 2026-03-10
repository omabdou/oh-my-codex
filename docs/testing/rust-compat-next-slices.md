# Rust parity next slices: setup, doctor, status/cancel

This note turns the next command families into merge-ready parity backlog items without touching the leader-owned harness files.

## Seed planning fixture

- `docs/testing/rust-parity-next-slices.json`

That JSON is intentionally a planning fixture, not a runnable suite for `scripts/compat/run-rust-parity.js` yet. The current runner only supports byte-exact command comparison, while the next slices need normalized text and file-tree assertions.

Checked-in fixture manifests for the first promotion step:

- `scripts/compat/fixtures/setup-scope/project-dry-run.fixture.json`
- `scripts/compat/fixtures/setup-scope/project-install.fixture.json`
- `scripts/compat/fixtures/setup-scope/user-default.fixture.json`
- `scripts/compat/fixtures/doctor-team/resume-blocker.fixture.json`
- `scripts/compat/fixtures/doctor-team/delayed-status-lag.fixture.json`
- `scripts/compat/fixtures/session-runtime/status-active.fixture.json`
- `scripts/compat/fixtures/session-runtime/cancel-team.fixture.json`

## Slice definitions

### 1. `setup-dry-run`

- **Parity mode:** normalized text plus artifact assertions
- **Primary contracts:**
  - `src/cli/__tests__/setup-scope.test.ts`
  - `src/cli/__tests__/setup-skills-overwrite.test.ts`
- **Stable command targets:**
  - `omx setup --dry-run --scope project`
  - `omx setup --scope project` in a temp repo
  - `omx setup` with no persisted scope in a temp repo
- **Checked-in planning fixtures:**
  - `scripts/compat/fixtures/setup-scope/project-dry-run.fixture.json`
  - `scripts/compat/fixtures/setup-scope/project-install.fixture.json`
  - `scripts/compat/fixtures/setup-scope/user-default.fixture.json`
- **What to compare:**
  - stable scope-selection lines in stdout
  - project-local file tree under `.codex/`, `.agents/`, and `.omx/` for project scope
  - user-home file tree under `<HOME>/.codex`, `<HOME>/.agents`, and `<HOME>/.omx` for user scope
  - persisted `.omx/setup-scope.json` plus whether project `AGENTS.md` is generated vs preserved
- **What to normalize:** temp paths and path separators only

### 2. `doctor-team`

- **Parity mode:** normalized text
- **Primary contracts:**
  - `src/cli/__tests__/doctor-team.test.ts`
- **Stable issue families:**
  - `resume_blocker`
  - `delayed_status_lag`
  - `orphan_tmux_session`
- **Fixture strategy:**
  - create a temp `.omx/state/team/<name>` tree
  - inject fake `tmux` binaries through `PATH`
  - preserve targeted worker `status.json` / `heartbeat.json` files as the semantic source of truth
- **What to normalize:** temp paths, path separators, and generated session ids

### 3. `team-status-cancel`

- **Parity mode:** normalized text plus artifact assertions
- **Primary contracts:**
  - `src/cli/__tests__/session-scoped-runtime.test.ts`
  - `src/cli/__tests__/team.test.ts`
  - `src/cli/team.ts`
- **Stable command targets:**
  - `omx status`
  - `omx cancel`
  - `omx team status <name>`
- **What to compare:**
  - stable summary lines such as `team: ACTIVE`, `Cancelled: team`, and `team=<name> phase=<phase>`
  - session-scoped state transitions in `.omx/state/sessions/<id>/team-state.json`
  - deterministic worker/task counts in team status fixtures
- **What to avoid:** direct parity against ad-hoc tmux log noise or volatile runtime timestamps

## Recommended implementation order

1. Keep `docs/testing/rust-parity-suite.json` as the only runnable byte-exact suite.
2. Add a second-layer semantic runner only when the leader-owned harness is ready to consume planning fixtures.
3. Implement the next slices in this order:
   1. `setup-dry-run`
   2. `doctor-team`
   3. `team-status-cancel`
4. Promote each slice from this plan file into an executable suite only after its normalization rules are explicit and reviewed.

## Runner impact for a future PR

`scripts/compat/run-rust-parity.js` already provides the right low-level command execution pieces for the next layer:

- baseline/candidate command selection
- per-case cwd/env/timeout overrides
- stdout/stderr/exit/signal capture
- shared normalizer plumbing

The missing piece is not raw process launching; it is an explicit semantic assertion layer for file trees and targeted normalized text fragments. Keep that future runner work separate from this planning artifact so the current byte-exact runner remains stable.
