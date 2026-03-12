# Native Surface Runtime Migration

Status: in-progress (2026-03-12)
Owner: Team broader-native-tmux-identical

## Objective
Deliver a tmux-identical operator experience using a native runtime surface while keeping tmux as a compatibility backend — not the default runtime authority.

This migration aligns launch, HUD, team layout/runtime metadata, hooks/notifications targeting, and docs/help so they tell one consistent truth.

## Inputs (authoritative)
- Context: `.omx/context/broader-native-tmux-identical-migration-20260312T043751Z.md`
- Context: `.omx/context/native-pane-runtime-omx-20260312T035817Z.md`
- PRD: `.omx/plans/prd-native-pane-runtime-omx.md`
- Test Spec: `.omx/plans/test-spec-native-pane-runtime-omx.md`
- Parity gap PRD/spec: `.omx/plans/prd-omx-shell-parity-gap-closure.md`, `.omx/plans/test-spec-omx-shell-parity-gap-closure.md`
- Implementation plan: `.omx/plans/impl-omx-shell-parity-gap-closure.md`

## Scope for this slice (docs-first)
- Review current help/doc surfaces for truthfulness vs. implemented behavior.
- Update/author docs to reflect the broader native migration and compatibility fence for tmux-backed flows.
- Call out what is “surface-parity” (visibility/UX) vs. “operational parity” (live orchestration semantics).

Non-goals for this slice: landing the remaining detached-session live orchestration in Rust (tracked separately in code/PRD).

## Operator experience contract
- Normal path: native prompt-mode workers (`omx team …`) and HUD (`omx hud --watch`) — works with or without tmux.
- Compatibility path: tmux-specific flows (e.g., `omx tmux-hook …`) when explicitly requested or integrated.
- Help output and fixtures must match shipped behavior. Keep `crates/omx-cli/src/lib.rs` and `src/compat/fixtures/help.stdout.txt` in lockstep.
- `omx sparkshell --tmux-pane …` remains an explicit operator-only compatibility inspection aid until Phase 4. It must not regain product-control-plane authority.

## Frozen UX parity checklist (Phases 1-3)

| Capability | Native-first authority | Compat-only / supporting surfaces | Phase 1-3 status |
| --- | --- | --- | --- |
| Launch team workers | `omx team …`, `src/team/runtime.ts`, persisted team state under `.omx/state/team/**` | tmux session helpers in `src/team/tmux-session.ts` only when explicitly selected | Native-first |
| Inspect worker progress | `omx team status …`, heartbeat/status files, team monitor snapshots | `omx sparkshell --tmux-pane …` for operator inspection only | Native-first with compat inspection |
| Nudge / recover | team API + dispatch/mailbox/task state, monitor/rebalance logic in `src/team/runtime.ts` | `scripts/notify-hook/team-leader-nudge.js` and related notify-hook scripts are compat-only | Native-first authority; compat helpers fenced |
| Idle / stall detection | worker heartbeat files, monitor snapshots, team status summaries | pane-tail review can assist humans but is not authoritative | Native-first |
| Shutdown / cleanup | state-backed lifecycle + native cleanup path | tmux teardown helpers only for compat sessions | Native-first with compat cleanup |

Definition of done for documentation truthfulness in this slice:
- launch, inspect, nudge, recover, and shutdown clearly describe a native-first authority path;
- tmux surfaces are called out as opt-in compatibility helpers;
- operator-facing docs do not imply tmux is required for normal product runtime.

## Runtime authority map (current review snapshot)

| Behavior | Authoritative files / surfaces | Review note |
| --- | --- | --- |
| Team lifecycle, monitor snapshots, worker/task/mailbox orchestration | `src/team/runtime.ts`, `src/team/state.ts`, `src/team/api-interop.ts` | Product authority lives in persisted state + team API, not pane ids |
| Worker launch transport / compat pane control | `src/team/tmux-session.ts` | Keep isolated behind backend/compat selection; no new direct product imports |
| Hook/plugin send/submit compatibility injection | `src/hooks/extensibility/sdk.ts` | Compat-only; already fenced with `no_backend` when tmux is disabled |
| Notify/session tmux detection + pane capture | `src/notifications/tmux-detector.ts`, `src/notifications/tmux.ts` | Compat-only observability helpers; not product-authoritative |
| Notify-hook watcher / nudge scripts | `scripts/notify-fallback-watcher.js`, `scripts/notify-hook/*.js` | Compat-only operational aids until native event/state replacements fully absorb the behavior |
| Operator documentation boundary | `README.md`, `docs/hooks-extension.md`, `docs/compat/notify-tmux-quarantine.md`, this file | Must consistently describe native-first runtime and tmux opt-in quarantine |

## Phase 3 native-cutover acceptance gate

Phase 3 should not be called complete unless all of the following are true at the same time:

1. **Help / docs truthfulness**
   - `README.md`, `src/compat/fixtures/help.stdout.txt`, and native help output all describe `omx team` as the default no-tmux path.
   - `omx tmux-hook` and `omx sparkshell --tmux-pane …` are described as explicit compatibility/operator paths, not normal runtime requirements.
2. **State-backed inspection authority**
   - team inspection guidance prefers heartbeat/status/task/mailbox/monitor-snapshot paths before pane-tail inspection.
   - pane-oriented output remains optional operator evidence only.
3. **Doctor / recovery expectations**
   - `omx doctor --team` continues to flag `slow_shutdown`, `stale_leader`, and `orphan_tmux_session` so Phase 3 does not hide cleanup or split-brain regressions.
   - native-first guidance explains that these diagnostics are primarily guarding compatibility or mixed-mode leftovers, not defining the normal product path.
4. **No hidden product-authoritative tmux dependency**
   - leader/worker progress, idle/stall detection, dispatch, and cleanup are explainable from persisted state + team APIs without requiring pane ids.
   - any remaining tmux-only surface is either compat-only or explicitly deferred to a later deletion/removal phase.

## Hidden tmux-authority risks to keep visible

- **Status output still emits pane inspection hints** (`src/cli/__tests__/team.test.ts`): useful for operators, but risky if reviewers mistake these hints for the authority path. Keep heartbeat/status/task/mailbox paths prominent in the same output.
- **Compat notify-hook scripts still model stall/nudge behavior** (`scripts/notify-hook/team-leader-nudge.js`, `scripts/notify-fallback-watcher.js`): Phase 3 must treat them as quarantine tooling, not the runtime source of truth.
- **Shutdown/orphan diagnostics still mention tmux sessions** (`src/cli/doctor.ts`, `crates/omx-cli/src/doctor.rs`): this is acceptable only as mixed-mode/compat cleanup evidence, not as proof that tmux remains required.
- **Launch parity docs still track detached-session tmux behavior** (`docs/rust/native-launch-legacy-parity-review.md`): keep this explicitly scoped as launch-compat work so it does not re-expand product authority for team runtime.

## Product-authoritative vs compat/docs-only tmux references

**Not product-authoritative (must stay compat/docs-only in Phase 3):**
- `src/notifications/tmux.ts`
- `src/notifications/tmux-detector.ts`
- `src/hooks/extensibility/sdk.ts` tmux send-keys path
- `scripts/notify-hook/*.js`
- `scripts/notify-fallback-watcher.js`
- `omx tmux-hook …`
- `omx sparkshell --tmux-pane …`

**Still allowed as bounded compatibility substrate, but not normal-path authority:**
- `src/team/tmux-session.ts`
- detached-session / HUD launch parity work tracked in `docs/rust/native-launch-legacy-parity-review.md`

**Product-authoritative for Phase 3 reasoning and acceptance:**
- `src/team/runtime.ts`
- `src/team/state.ts`
- `src/team/api-interop.ts`
- team-state/heartbeat/status/task/mailbox/monitor snapshot files under `.omx/state/team/**`
- native help/docs/release-gate surfaces describing the default path

## Failure-mode review before Phase 3 cutover

### Hidden tmux authority still present
- `scripts/notify-fallback-watcher.js` still performs direct `tmux send-keys` recovery/dispatch/nudge actions (`send-keys` at lines 205-218, dispatch/nudge integration at 634 and 676). This is the clearest remaining compat-side authority that must stay fenced until a native event/state replacement exists.
- `scripts/notify-hook/team-leader-nudge.js`, `scripts/notify-hook/team-worker.js`, `scripts/notify-hook/auto-nudge.js`, and `scripts/notify-hook/team-dispatch.js` still own pane-targeted nudges/injections. These are useful operator aids today, but Phase 3 docs must treat them as compat-only, never default runtime truth.
- `src/hooks/extensibility/sdk.ts` still exposes tmux injection for plugins, but it now hard-fences that path with `OMX_NO_TMUX=1`, `no_tmux`, and `layout_mode === native_equivalent`, returning `no_backend` instead of silently taking back authority.

### Split-brain risk ledger
- **State authority vs pane authority:** `src/team/runtime.ts` treats `.omx/state/team/**` plus mailbox/task/heartbeat files as canonical, but compat scripts can still push progress via tmux panes. If docs do not call this out, operators may trust pane activity over persisted team state.
- **Cross-worktree heartbeat routing:** tests around cross-worktree heartbeats show that worker liveness must resolve to `OMX_TEAM_STATE_ROOT`, not the worker cwd. Any fallback script or plugin still writing local-worktree state can create false idle/stall or false-dead worker readings.
- **Compat inspection vs product inspection:** `omx sparkshell --tmux-pane` and raw `capture-pane` reads can help humans debug, but those views can lag or diverge from mailbox/task status. Treat them as evidence helpers, not control-plane truth.

### Failure modes that need native absorption
- **Stalled worker recovery:** today a stalled worker can still be nudged by tmux-pane inspection + `send-keys`; native runtime must own this through heartbeat/task/event-state rules so recovery works without pane reachability.
- **Cleanup / orphan processes:** `src/team/runtime.ts` already contains explicit cleanup/rollback logic (`killWorkerByPaneIdAsync`, `teardownWorkerPanes`, `cleanupTeamState`), but compat cleanup still assumes pane knowledge. Native cleanup must finish from process/session metadata alone so dead panes do not strand workers.
- **Leader stale / dispatch drain:** `notify-fallback-watcher` currently bridges leader-stale and dispatch receipt gaps. Before Phase 3 is considered cut over, these loops need equivalent native event delivery semantics or must remain explicitly quarantined as compat-only.

### JS hooks that still own runtime-adjacent behavior
1. `scripts/notify-fallback-watcher.js` — fallback dispatch drain, leader stale nudges, Ralph continue steering.
2. `scripts/notify-hook/team-dispatch.js` — queued dispatch injection into tmux targets.
3. `scripts/notify-hook/team-leader-nudge.js` — leader-idle/stale detection and compatibility nudges.
4. `scripts/notify-hook/auto-nudge.js` / `team-worker.js` — pane capture + idle/stall nudges.
5. `scripts/notify-hook/tmux-injection.js` — generic compat send-keys plumbing.
6. `src/hooks/extensibility/sdk.ts` — plugin-facing tmux send/submit surface (correctly fenced, but still a surviving hook surface).

**Phase 3 documentation gate:** native-first docs can claim cutover only while these JS hooks are described as quarantined compatibility helpers. If any of them remain necessary for default launch, inspect, nudge, recover, or shutdown semantics, Phase 3 is not truly complete.

## Verification
- Rust: `cargo test --workspace`
- TypeScript: `npx tsc --noEmit`
- Lint (biome): `npm run -s lint`
- Focused parity tests: native launch parity tests under `crates/omx-cli/tests/*` (inside-tmux vs detached-session vs no-tmux)

## Follow-ups (next slices)
- Broaden validation of the detached-session parity path (more scenarios, failure-rollbacks under load).
- Hook/notification targeting against abstract surfaces (not tmux panes).
- Team runtime metadata refinements and docs/screens for native layouts.

## Quick usage
```bash
# Native team run (no tmux required)
omx team 3:executor "short scoped task"

# HUD statusline (inline or in a split when inside tmux)
omx hud --watch

# Optional compatibility workflow
omx tmux-hook status
```
