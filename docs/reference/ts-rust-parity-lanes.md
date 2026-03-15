# Parallel TS → Rust parity lanes (TypeScript as SSOT)

## Purpose
A review-only parity map for the current hybrid runtime. TypeScript remains the source of truth for behavior and state contracts. Rust ownership is tracked per surface so cutover claims stay evidence-based.

## Current native entry surfaces
- `crates/omx-runtime/src/main.rs` exposes the current native boundary commands: `phase1-topology`, `capture-pane`, `hud-watch`, `reply-listener`, `notify-fallback`, `hook-derived`, and `runtime-run`.
- `docs/reference/rust-runtime-phase1-cutover-order.md` already records that the spawn boundary is native for HUD, watcher, reply-listener, and MCP `runtime-run`; the remaining risk is behavioral parity rather than a hidden Node launcher.

## Lane 1 — startup contract / runtime parity
### TypeScript SSOT contracts
- `src/team/runtime.ts:725` — `startTeam()` owns the canonical team bootstrap flow.
- `src/team/runtime.ts:1226` — `monitorTeam()` owns team-state reconciliation, expired-claim recovery, mailbox delivery, rebalance, verification gating, and monitor snapshot writes.
- `src/team/runtime.ts:1587` — `shutdownTeam()` owns shutdown requests, worker ACK handling, cleanup, and linked-Ralph terminal sync.
- Supporting stateful TS-only behaviors still wired into runtime parity:
  - `src/team/runtime.ts:1244` — expired claim reclaim.
  - `src/team/runtime.ts:1320` — rebalance policy.
  - `src/team/runtime.ts:1364` — structured verification evidence gate.
  - `src/team/runtime.ts:1386,1425` — phase + monitor snapshot persistence.
  - `src/team/runtime.ts:2160,2176,2191,2609,2676` — inbox/mailbox dispatch paths.

### Rust ownership today
- `crates/omx-runtime/src/runtime_run.rs:255` — native team-state initialization.
- `crates/omx-runtime/src/runtime_run.rs:381` — native post-session state finalization.
- `crates/omx-runtime/src/runtime_run.rs:772` — native `monitor_team()` loop.
- `crates/omx-runtime/src/runtime_run.rs:834,867` — native shutdown/result emission.
- `crates/omx-runtime/src/runtime_run.rs` also now persists parity-adjacent monitor/phase metadata (`mailboxNotifiedByMessageId`, `completedEventTaskIds`, `phase-state.json`), performs bounded root-team / linked-Ralph terminal mode-state sync, and records limited shutdown request/ACK events, but that is still narrower than full TS lifecycle semantics.
- `src/mcp/team-server.ts:327-333` — MCP now spawns `omx-runtime runtime-run` at the runtime boundary.

### Verified gap summary
Rust owns the launch seam, but does **not** yet match TS lifecycle semantics. The native startup/config path already persists `team_state_root`, `workspace_mode`, worker `role`, lifecycle profile, and bounded monitor metadata; however the native monitor/shutdown path is still materially narrower than `src/team/runtime.ts`:
- no leader-session conflict parity,
- no full worktree provisioning / detached-trigger parity,
- no worker instruction-file / model-instruction parity,
- no mailbox delivery / dispatch receipt parity,
- no rebalance parity,
- no structured verification evidence gate parity,
- no full linked-Ralph shutdown/event parity beyond bounded terminal-state sync.

### Cutover guidance
Treat `runtime-run` as **native-owned but parity-incomplete**. Safe claim: launch boundary migrated. Unsafe claim: full team lifecycle parity.

## Lane 2 — team runtime / tmux control-plane parity
### TypeScript SSOT contracts
- `src/team/tmux-session.ts:760` — `createTeamSession()` owns pane/session topology.
- `src/team/tmux-session.ts:975` — `restoreStandaloneHudPane()` owns HUD-pane restoration behavior.
- `src/team/tmux-session.ts:1182,1244` — readiness polling + trust-prompt dismissal.
- `src/team/tmux-session.ts:1290,1592` — worker/leader send-key injection.
- `src/team/tmux-session.ts:1386,1455,1525,1565` — pane PID lookup, kill, teardown, and session destroy flows.

### Rust ownership today
- `crates/omx-runtime/src/main.rs` exposes `capture-pane` as the native pane-observation seam.
- `crates/omx-runtime/src/tmux.rs` and reply-listener tests already cover pane analysis and send-path helpers, as summarized in `docs/reference/rust-runtime-phase1-cutover-order.md`.

### Verified gap summary
Rust has bounded tmux primitives, but TS still owns the higher-order tmux control plane:
- team session creation/layout,
- worker readiness polling,
- trust-prompt handling,
- worker/leader pane delivery retries,
- teardown policy and exclusion rules.

### Cutover guidance
Native tmux parity is **primitive-complete enough for helpers**, not **session-orchestrator complete**.


## Lane 3 — HUD behavior parity
### TypeScript SSOT contracts
- `src/hud/index.ts` owns the watch-loop TTY, cursor, SIGINT, and non-overlap behavior.
- `src/hud/state.ts` owns `readAllState()` and scoped state loading.
- `src/hud/render.ts` owns preset/token rendering behavior.

### Rust ownership today
- `crates/omx-runtime/src/hud.rs` exposes `hud-watch` and a minimal native watch loop / render surface.
- `src/cli/runtime-native.ts` already routes the guarded native HUD launch to `omx-runtime hud-watch`.

### Verified gap summary
HUD launch ownership is native on the guarded path, but behavior is still parity-incomplete:
- no TS-equivalent `readAllState()` implementation,
- no TS-equivalent render token/preset behavior,
- no verified TTY/cursor/SIGINT/non-overlap parity beyond a minimal watch loop.

### Cutover guidance
Safe claim: guarded HUD launch is native. Unsafe claim: HUD parity is complete.

## Lane 4 — watcher / reply-listener parity

### TypeScript SSOT contracts
- `scripts/notify-fallback-watcher.js` still defines the richer fallback watcher behavior, including tmux send-key injection and rollout-derived nudges.
- `scripts/hook-derived-watcher.js` still defines derived-event watcher behavior and state/log file conventions.
- `src/notifications/reply-listener.ts:446` — TS start/status/stop orchestration still normalizes config/state and dispatches native runtime.
- `src/notifications/reply-listener.ts` still documents the canonical guarded behavior: sanitize input, verify pane content, inject via sendToPane, and persist `discordLastMessageId` / daemon state.

### Rust ownership today
- `crates/omx-runtime/src/watchers.rs` provides native `notify-fallback` and `hook-derived` command surfaces, but current behavior is intentionally minimal: parse args, optionally write pid files, and sleep.
- `crates/omx-runtime/src/reply_listener.rs` now owns a meaningful bounded slice:
  - config parsing,
  - Discord fetch command construction,
  - `discordLastMessageId` progression,
  - registry lookup,
  - pane injection/log/state updates,
  - `status` / `stop` commands.
- `cargo test -p omx-runtime` currently passes `49` tests, including reply-listener and watcher tests.

### Verified gap summary
- Reply-listener parity is the strongest native lane, but TS still owns outer config/state normalization and launch orchestration.
- Watcher parity is **boundary-only** today: native commands exist, but the richer JS watcher semantics are not yet ported.

### Cutover guidance
Safe claim: guarded watcher/reply launch path is native-aware. Unsafe claim: watcher behavior is fully parity-complete.

## Lane 5 — MCP / CLI boundary mapping and truthfulness
### TypeScript SSOT contracts
- `src/mcp/team-server.ts` remains the canonical MCP tool surface for start/status/wait/cleanup semantics.
- `src/cli/runtime-native.ts` remains the canonical TS selector/hydration layer for resolving packaged `omx-runtime` binaries.
- `src/cli/index.ts`, `src/cli/team.ts`, and HUD/tmux call sites still define the user-facing CLI semantics even where the spawned command is native.

### Rust ownership today
- `src/mcp/team-server.ts:327-333` maps MCP `omx_run_team_start` into `omx-runtime runtime-run`.
- `src/cli/runtime-native.ts:40,62,84,87,117,135,147` maps packaged/runtime binary resolution for `omx-runtime` command use.
- `crates/omx-runtime/src/topology.rs` explicitly documents the intended Phase 1 ownership model: one native launcher for launcher boundary, HUD, team supervision, pane observation, watcher loops, and `.omx/state` writing.

### Verified gap summary
The boundary map is clear: **CLI/MCP entry semantics are still TS-defined**, while selected execution surfaces are already native. This means parity review must separate:
1. spawn-boundary migration, from
2. behavior migration.

## Practical verdict
### Safe statements
- TypeScript is still the behavioral SSOT for team runtime, tmux orchestration, and watcher semantics.
- Rust now owns multiple runtime entrypoints and bounded helper/runtime slices.
- The largest remaining parity risk is `runtime_run.rs` vs `src/team/runtime.ts`, not a hidden Node launch seam.
- PR #841 should stay draft unless the remaining blocker ledger is fully resolved or truthfully narrowed **and** the owner explicitly authorizes undraft/merge.

### Unsafe statements
- "Rust fully matches TS team-runtime behavior."
- "HUD parity is complete."
- "Watcher parity is complete."
- "tmux control-plane parity is complete."
- "Cutover is ready without additional lifecycle verification."
- "PR #841 is ready to undraft without explicit owner instruction."

## Verification snapshot
- `npm run build -- --pretty false` → PASS.
- `node --test dist/verification/__tests__/phase1-runtime-surface-parity.test.js` → PASS (`5 passed; 0 failed`).
- `node --test dist/verification/__tests__/ts-rust-parity-lanes-doc.test.js` → PASS (`1 passed; 0 failed`).
- Code-boundary review confirms native command surfaces in `crates/omx-runtime/src/main.rs` and MCP spawn via `src/mcp/team-server.ts:327-333`.

## Recommended next review order
1. `runtime_run.rs` vs `src/team/runtime.ts` startup/monitor/shutdown parity.
2. `src/team/tmux-session.ts` session/topology/retry/teardown parity.
3. HUD behavior parity beyond the native launch seam.
4. watcher behavior parity beyond boundary-only native shims.
5. final MCP/CLI truthfulness pass so docs never over-claim cutover readiness.
