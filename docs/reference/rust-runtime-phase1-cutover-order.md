# Phase 1 Rust Runtime Cutover Order

## Purpose
Safe retirement order for remaining JS/TS runtime owners on the phase-1 control-plane path.

## Current cutover status
- **Completed in this worktree:** HUD/runtime command construction is centralized in `src/cli/runtime-native.ts`, and `crates/omx-runtime/` is now a workspace member with `hud-watch`, `phase1-topology`, and `capture-pane` subcommands.
- The following call sites no longer hand-build their own HUD/runtime commands:
  - `src/cli/index.ts`
  - `src/hud/index.ts`
  - `src/team/tmux-session.ts`
  - `src/cli/team.ts` (native pane-inspection command generation)
- **What live HUD/runtime owner is now replaced under native mode:** when `OMX_RUNTIME_HUD_NATIVE=1`, the live HUD launch selected by `src/cli/runtime-native.ts` is `omx-runtime hud-watch`, so the Node-owned `hud --watch` launch path is bypassed for the shared HUD launch sites above.
- **Fresh HUD truth guard:** this is a launch-seam cutover, not a full behavior-parity claim. `crates/omx-runtime/src/hud.rs` still provides a minimal watch/render loop rather than TS-equivalent `readAllState()` + render-token parity from `src/hud/index.ts`, `src/hud/state.ts`, and `src/hud/render.ts`.
- **Strict evidence for that bypass:** `crates/omx-runtime/src/main.rs` now exposes `hud-watch`; `src/cli/runtime-native.ts` emits `omx-runtime hud-watch`; and `src/team/__tests__/runtime.test.ts:2075-2134` asserts tmux logs contain `'/tmp/crates/omx-runtime' hud-watch` and do **not** contain `node ... hud --watch` during HUD-pane restoration.
- **What MCP/team-runner owner is now replaced/bypassed:** `src/mcp/team-server.ts` no longer spawns `node runtime-cli.js` directly; it resolves the native runtime binary and launches `runtime-run`, and `crates/omx-runtime/src/runtime_run.rs` now owns native startup/bootstrap, pane metadata, monitor glue, and shutdown glue for that seam without invoking Node startup helpers.
- **Exact remaining blocker for a truthful 100% native claim:** the remaining risk is now parity, not a live Node startup dependency. `runtime-run` has native startup ownership, but its monitor/shutdown/bootstrap behavior is still a bounded subset of `src/team/runtime.ts`: it does not yet match worktree provisioning, richer worker launch-arg/model selection, startup dispatch retry/readiness evidence, mailbox delivery/rebalance, or shutdown ACK/event/linked-Ralph parity. The cut is now natively owned but still needs process-level proof plus behavior-parity review before any strong completion claim.
- **What guarded watcher path is now replaced/bypassed:** `src/cli/index.ts:1833-1847` now bypasses `node scripts/notify-fallback-watcher.js --once` when `OMX_RUNTIME_WATCHERS_NATIVE=1` and instead resolves the native runtime binary and launches `notify-fallback --once`.
- **What guarded hook-derived watcher path is now replaced/bypassed:** `src/cli/index.ts:1849-1860` now bypasses `node scripts/hook-derived-watcher.js --once` when `OMX_RUNTIME_WATCHERS_NATIVE=1` and instead resolves the native runtime binary to launch `hook-derived --once`.
- **What guarded long-lived notify-fallback watcher owner is now replaced/bypassed:** `src/cli/index.ts:1661-1727` now bypasses detached `node scripts/notify-fallback-watcher.js` startup when `OMX_RUNTIME_WATCHERS_NATIVE=1` and instead resolves the native runtime binary to launch detached `notify-fallback` with the same pid-file / lifetime inputs.
- **What guarded long-lived hook-derived watcher owner is now replaced/bypassed:** `src/cli/index.ts:1755-1784` now bypasses detached `node scripts/hook-derived-watcher.js` startup when `OMX_RUNTIME_WATCHERS_NATIVE=1` and instead resolves the native runtime binary to launch detached `hook-derived` for the same cwd-scoped state path.
- **What guarded reply-listener owner is now replaced/bypassed:** `src/notifications/reply-listener.ts` now resolves the native runtime binary to launch `reply-listener` on the default/guarded path; the legacy Node daemon no longer remains available as a phase-1 runtime fallback.
- **What prompt-mode control is now replaced/bypassed on the phase-1 native path:** `src/team/runtime.ts:2056-2063` now centralizes the phase-1 prompt-mode gate and disables prompt transport whenever `OMX_RUNTIME_HUD_NATIVE=1`, `OMX_RUNTIME_WATCHERS_NATIVE=1`, or `OMX_RUNTIME_REPLY_LISTENER_NATIVE=1`, so prompt-mode branches are short-circuited on the native path rather than remaining active runtime control-plane owners.
- **What bounded reply-listener stop/status seam is now native-aware:** worker-2's verified built-path verdict confirms the guarded stop/status seam is now native-aware on the guarded path, so reply-listener no longer remains the next highest-value live Node owner solely because of that bounded seam.
- **What reply-listener fallback slice is now reduced:** the guarded/default reply-listener path no longer launches the legacy `node -e` daemon. The remaining JS surface is limited to config/state normalization plus native dispatch into `omx-runtime reply-listener`.
- **What native parity slices are now verified:** worker-2 confirms native inject-reply core/state/log behavior is real and verified via `cargo test -p omx-runtime` passing `22/22`; the verified bridge claim adds native `lookup-message` parity for the raw registry-correlation sub-step (`reply message ID -> mapping lookup`) with `cargo test -p omx-runtime` at `23/23`; the pane-send claim adds native `send_to_pane` parity in `crates/omx-runtime/src/tmux.rs` plus the reply-listener hook, removing the final JS-owned `sendToPane` step on the guarded live-send path with `cargo test -p omx-runtime` at `25/25`; the pane-validation claim adds native `PaneAnalysis` + `analyze_pane_content()`, removing `analyzePaneContent(content)` plus the confidence-based decision from the guarded path, verified by `cargo test -p omx-runtime` at `26/26`; the live-path claim adds native Discord HTTP fetch parity, removing the `channel-messages` GET/auth/after-cursor request-formation step inside `pollDiscord`; the next verified claim adds native `discordLastMessageId` progression parity, removing the JS cursor/state progression blocker by taking over after-cursor construction and `discordLastMessageId` state evolution; the parsing/helper claim adds native `parse_first_discord_message()` plus low-level JSON string decoding/unescaping, removing the JS `message_reference.message_id` extraction blocker and raw Discord content-string decode ownership first; and the latest live Discord chain claim verifies that `reply_listener.rs` now does `parse_first_discord_message -> update_discord_last_message_id -> lookup_message_mapping -> inject_reply` for the first fetched Discord reply, which moves the JS `lookupByMessageId` correlation-usage blocker first.
- **What still remains live:** the JS `src/notifications/reply-listener.ts` module is now reduced to a bounded control seam that normalizes config/state and dispatches to native `omx-runtime reply-listener`; the guarded live daemon/poll loop is no longer Node-owned on the phase-1 path. Likewise, the watcher launch sites in `src/cli/index.ts` now require native `omx-runtime` on the phase-1 path instead of falling back to Node watcher daemons.
- **Next remaining review focus after the verified native watcher/reply cutovers:** the biggest remaining control-plane risk is still the MCP team-runner lifecycle seam, but now the risk is behavioral parity inside native `runtime_run.rs`, not a live Node bridge. Review should focus on startup/bootstrap completeness, dispatch/retry handoff boundaries, and shutdown/linked-Ralph semantics.
- **Fresh truth guard (2026-03-13):** direct code-boundary review now records the new cut line in one place: `src/mcp/team-server.ts` spawns `omx-runtime runtime-run`; `crates/omx-runtime/src/runtime_run.rs` no longer references `runtime-cli.js`, `START_TEAM_SCRIPT`, or `execute_node_json()`; and native `start_team()` now owns initial state/session/bootstrap creation. The remaining questions are parity and verification depth, not whether Node still owns the startup seam.
- **Boundary note:** the HUD owner is replaced only for launch paths that already route through the shared selector under native mode; full runtime/control-plane cutover still depends on retiring the remaining Node-owned lifecycle surfaces above.
- **PR #841 draft gate:** keep the PR in draft state until the blocker ledger above is either fully resolved or truthfully narrowed in PR-facing docs/review text, and only undraft after explicit owner instruction.


## Runtime-run review notes (2026-03-13)
- **Truthfulness:** `src/mcp/team-server.ts` is native at the spawn boundary and `runtime_run.rs` now owns startup/bootstrap natively. The seam should no longer be described as Node-backed at startup; remaining caution should focus on parity gaps inside the native implementation before any broader completion claim.
- **Code-quality caution:** `crates/omx-runtime/src/runtime_run.rs` still parses stdin JSON with ad-hoc string extraction helpers rather than a structured JSON decoder. That is acceptable for today's wrapper tests, but it is not yet a strong foundation for the eventual native lifecycle owner.
- **Verification guard:** keep process-boundary proof separate from launch-boundary proof. A passing `team-server.ts -> omx-runtime runtime-run` spawn assertion plus `runtime-cli.js` removal is no longer enough on its own; evidence now needs to show the native startup path behaves correctly end-to-end and that remaining monitor/shutdown/bootstrap parity gaps are understood.
- **Porting implication:** the eventual Rust cutover needs more than command replacement; it must re-home startup/bootstrap ownership plus preserve pane sidecar refresh, dead-worker heuristics, signal/shutdown fallback, and linked-Ralph cleanup semantics.
- **Code review finding (2026-03-13):** `runtime_run.rs::start_team()` is now natively owned and covers team-state initialization, tmux session creation, identity/config persistence, and worker bootstrap prompting. The remaining startup review gaps are narrower: no worktree provisioning, reduced worker launch-arg/model-selection parity, and simpler bootstrap prompting/transport evidence than `src/team/runtime.ts`.
- **Code review finding (2026-03-13):** the native `monitor_team()` wrapper is materially simpler than `src/team/runtime.ts::monitorTeam()` — it counts task files and dead panes, but it does not reclaim expired task claims, rebalance assignments, deliver mailbox messages, persist monitor snapshots, or enforce structured verification evidence before terminal success.
- **Code review finding (2026-03-13):** the native `shutdown_team()` wrapper is also materially simpler than `src/team/runtime.ts::shutdownTeam()` — it force-kills panes and deletes state, but it does not issue shutdown requests, wait for worker ACKs, append shutdown/ralph events, restore linked Ralph terminal state, or run prompt-worker teardown parity.
- **Code review finding (2026-03-13):** `src/team/runtime.ts` now centralizes `shouldUsePromptTransport()` / `isPhase1PromptModeDisabled()` for native phase-1 paths, so prompt-transport gating is still an active TS-owned behavior. That makes `dispatchCriticalInboxInstruction()` the safest first-cut handoff boundary: native code can own state-init + tmux/bootstrap first, while TS keeps transport selection/retry semantics until they stabilize.

## Verification snapshot (2026-03-13)
- `cargo build --workspace` ✅ passes for all Rust workspace members.
- `cargo test --workspace` ✅ passes, including `omx-runtime` coverage for the current `runtime-run` wrapper behavior.
- `npm run build -- --pretty false` ✅ passes.
- Fresh code-boundary review confirms `src/mcp/team-server.ts` launches `omx-runtime runtime-run`; `crates/omx-runtime/src/runtime_run.rs` no longer references `runtime-cli.js`, `START_TEAM_SCRIPT`, or `execute_node_json()`; and native `start_team()` now owns initial state/session/bootstrap creation while Rust still keeps simplified monitor/shutdown wrappers.
- Targeted Node runtime suites ✅ pass:
  - `dist/cli/__tests__/runtime-native.test.js`
  - `dist/team/__tests__/runtime.test.js`
  - `dist/mcp/__tests__/team-server-runtime-deps.test.js`
  - `dist/hud/__tests__/hud-tmux-injection.test.js`
  - `dist/notifications/__tests__/reply-listener.test.js`
  - `dist/team/__tests__/api-interop.test.js`
- Process-boundary evidence remains assertion-backed in tests:
  - HUD native path checks for `omx-runtime hud-watch` and rejects `node ... hud --watch`.
  - Parity truthfulness doc guards now also require explicit HUD launch-vs-behavior language, so docs fail if they over-claim full HUD parity on the native path.
  - Team MCP runtime seam now checks for `runtime-run` at the spawn boundary and for removal of `runtime-cli.js` references, and review now confirms the startup seam no longer routes through `START_TEAM_SCRIPT` or `dist/team/runtime.js`.
  - The remaining risk is structural parity, not a hidden Node bridge: `runtime_run.rs` now owns startup/bootstrap, but it still does not match all of `src/team/runtime.ts` for launch-arg selection, worktrees, dispatch/retry evidence, mailbox/rebalance behavior, and shutdown/linked-Ralph semantics.
  - Guarded reply-listener path checks for native `reply-listener` status/stop/start routing.
  - Direct guarded daemon probe shows `startReplyListener()` launching `bin/rust/linux-x64/omx-runtime reply-listener`, followed by a clean native stop, with no Node-owned guarded reply-listener daemon command line.

## Recommended deletion / retirement order
1. **`src/team/runtime.ts` prompt-mode branch**
   - Remove first because the ADR already invalidates prompt-mode workers for phase 1.
   - Lowest tmux-regression risk because the target runtime is tmux-backed only.
   - Key surfaces: prompt launch mode setup, prompt transport checks, prompt resume/shutdown branches.

2. **`crates/omx-runtime/src/runtime_run.rs` inline Node lifecycle helpers**
   - Replace next with native `startTeam` / `monitorTeam` / `shutdownTeam` ownership so MCP/team-runner lifecycle ownership actually moves under a single native owner.
   - `src/mcp/team-server.ts` has already switched to the native binary; the remaining split-brain risk is inside `runtime-run` itself.

3. **`src/cli/index.ts` watcher ownership (`notify-fallback-watcher.js`, `hook-derived-watcher.js`)**
   - Retire detached watcher lifecycle after the native supervisor exists.
   - These are long-lived control-plane loops and should move under the new owner before UI/HUD cleanup finishes.

4. **`src/notifications/reply-listener.ts` daemon ownership**
   - Migrate only the retained phase-1 reply behavior after watcher ownership is native.
   - Keeps notification/reply scope constrained while eliminating another Node daemon.

5. **`src/hud/index.ts` Node HUD tmux launch**
   - Replace once a native HUD entrypoint exists.
   - This is a user-visible path, so it should follow establishment of the native supervisor/runtime contract.
   - Transitional note: command construction is already centralized; the remaining work is swapping the selected live command from Node to the native HUD entrypoint.

6. **`src/team/tmux-session.ts` HUD create/restore launch paths**
   - Retire immediately after the HUD entrypoint is native.
   - Must stay aligned with the standalone HUD launch contract to avoid split launch behavior.

7. **`src/cli/index.ts` HUD launch in `runCodex`**
   - Remove last among HUD launchers after both the HUD entrypoint and tmux-session HUD restore/create flow are native.
   - This prevents the CLI from being left without a working HUD launch target during transition.
   - Transitional note: `runCodex` already routes through the shared runtime-native selector; the remaining Node ownership is the selected command, not per-site string assembly.

## Evidence-backed runtime owners
- `src/team/runtime.ts:947, 963, 1027, 1253, 1362, 1706, 1716, 1849, 2026, 2131, 2504, 2645, 2697`
- `src/mcp/team-server.ts:327-333`
- `src/cli/index.ts:1666-1858`
- `src/notifications/reply-listener.ts:744-879`
- `src/hud/index.ts:276-301`
- `src/team/tmux-session.ts:847, 944`
- `src/cli/index.ts:1215-1216`

## Rationale
- Establish the **single native lifecycle owner** before deleting long-lived loops.
- Delete the **ADR-invalid prompt path** first because it is pure scope reduction.
- Move **watchers/reply daemons** under native ownership before removing the last Node HUD launchers.
- Retire **HUD launch sites together** only after the native HUD path exists, to preserve tmux parity during cutover.
