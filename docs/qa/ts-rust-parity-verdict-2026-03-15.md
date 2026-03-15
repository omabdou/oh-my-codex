# TS↔Rust parity verdict audit (2026-03-15)

## Scope
Current truth-preserving parity audit across the five lanes in `docs/reference/ts-rust-parity-lanes.md`, with TypeScript treated as behavioral SSOT and Rust treated as native-boundary / partial-behavior owner where directly proven.

## Lane 1 — startup contract / runtime parity
- **Verdict:** PARTIAL
- **TS SSOT evidence:** `src/team/runtime.ts:725`, `:1238`, `:1599`, `:1256`, `:1332`, `:1376`, `:2172`, `:2219`, `:2621`, `:2741`
- **Rust evidence:** `crates/omx-runtime/src/runtime_run.rs:152`, `:348`, `:490`, `:1029`, `:1045-1047`, `:1079-1097`, `:1127`, `:1157`, `:1605-1672`, `:2266`
- **Strongest proven Rust-owned slice:** native `runtime-run` startup/monitor/shutdown seam plus monitor snapshot + linked-Ralph terminal sync.
- **Highest-confidence remaining gap:** TS still owns richer lifecycle semantics around worker launch/readiness/dispatched inbox flow and broader shutdown semantics.
- **Docs truthfulness:** truthful.

## Lane 2 — tmux control-plane parity
- **Verdict:** PARTIAL
- **TS SSOT evidence:** `src/team/tmux-session.ts:760`, `:975`, `:1182`, `:1244`, `:1290`, `:1386`, `:1455`, `:1525`, `:1565`, `:1592`
- **Rust evidence:** `crates/omx-runtime/src/tmux.rs:17`, `:50`, `:86`, `:118`, tests at `:155-196`; command exposure in `crates/omx-runtime/src/main.rs:39-44`
- **Strongest proven Rust-owned slice:** pane capture + send-to-pane helpers and bounded pane analysis.
- **Highest-confidence remaining gap:** Rust does not own team session topology, readiness polling, trust-prompt dismissal, or teardown orchestration.
- **Docs truthfulness:** truthful.

## Lane 3 — HUD behavior parity
- **Verdict:** PARTIAL
- **TS SSOT evidence:** `src/hud/index.ts:31`, `:74`, `:208`; `src/hud/state.ts:226`; `src/hud/render.ts:190`
- **Rust evidence:** `crates/omx-runtime/src/hud.rs:10-53`; guarded launch routing in `src/cli/runtime-native.ts:13`, `:51-55`
- **Strongest proven Rust-owned slice:** guarded native `hud-watch` launch + minimal native frame loop.
- **Highest-confidence remaining gap:** Rust lacks TS-equivalent `readAllState()` + render behavior; native HUD still renders a placeholder frame.
- **Docs truthfulness:** truthful.

## Lane 4 — watchers / reply-listener / notifications parity
- **Verdict:** PARTIAL
- **TS SSOT evidence:** `scripts/notify-fallback-watcher.js:80-111`, `:267`, `:396`, `:493`; `scripts/hook-derived-watcher.js:33`, `:87`, `:141`, `:155`, `:173`; `src/notifications/reply-listener.ts:430`, `:446`, `:480-529`
- **Rust evidence:** `crates/omx-runtime/src/watchers.rs:16-34`, `:143`, `:159`; `crates/omx-runtime/src/reply_listener.rs:46-54`, `:178-220`, `:375`, `:605`, `:657`, `:683`, `:820`, `:868`
- **Strongest proven Rust-owned slice:** reply-listener core flow: config parsing, Discord fetch path, registry lookup, inject-reply, status/stop/start.
- **Highest-confidence remaining gap:** watcher ports remain boundary-only/minimal while JS watcher scripts still own richer dispatch/nudge/derived-event behavior.
- **Docs truthfulness:** truthful.

## Lane 5 — MCP / CLI boundary mapping and truthfulness
- **Verdict:** PASS for boundary mapping, PARTIAL for behavioral cutover
- **TS SSOT evidence:** `src/mcp/team-server.ts:351-356`; `src/cli/runtime-native.ts:40`, `:51-55`, `:87`, `:117`; native watcher launch sites in `src/cli/index.ts:1676-1724`, `:1792-1794`, `:1880-1897`
- **Rust evidence:** `crates/omx-runtime/src/main.rs:30`, `:39-44`, `:109-124`; topology/cutover docs in `docs/reference/rust-runtime-phase1-cutover-order.md:16-29`, `:37-62`
- **Strongest proven Rust-owned slice:** native command boundary is real: MCP start path spawns `omx-runtime runtime-run`, and guarded HUD/watcher/reply-listener launch paths resolve the native binary.
- **Highest-confidence remaining gap:** docs must continue distinguishing launch-boundary migration from full behavioral parity.
- **Docs truthfulness:** truthful.

## Overall verdict
- **Current verdict:** Rust owns multiple native execution boundaries and several bounded behavior slices, but **full TS behavioral parity is not proven**.
- **Most mature Rust-owned lane:** runtime-run seam + reply-listener core behavior.
- **Least mature Rust-owned lane:** watcher behavior and HUD behavior beyond launch ownership.
- **Overclaim risk:** saying tmux/HUD/watchers/runtime are fully parity-complete would be false.

## Verification evidence
- `cargo test -p omx-runtime` — PASS (`71 passed; 0 failed`)
- `npm run build -- --pretty false` — PASS (`EXIT_CODE=0`)
- `node --test dist/verification/__tests__/phase1-runtime-surface-parity.test.js` — PASS (`5 passed; 0 failed`)
- `node --test dist/verification/__tests__/ts-rust-parity-lanes-doc.test.js` — PASS (`1 passed; 0 failed`)
