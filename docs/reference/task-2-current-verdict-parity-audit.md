# Task 2 — Current TS↔Rust verdict-parity audit

Date: 2026-03-15
Worker: worker-2 (verifier)

## Lane 1 — startup contract / runtime parity
- Verdict: PARTIAL
- Strongest proven Rust-owned slice:
  - `src/mcp/team-server.ts:351-356` spawns native `runtime-run`.
  - `crates/omx-runtime/src/runtime_run.rs:1029-1146` reclaims expired claims, rebalances work, gates terminal success on structured verification evidence, delivers mailbox notifications, writes phase/monitor snapshots, and syncs linked-Ralph terminal state.
- Highest-confidence remaining gap:
  - TS still owns leader-session conflict and worktree / role-instruction bootstrap semantics: `src/team/runtime.ts:751-798, 855-906` vs Rust startup templates pinned to `"workspace_mode":"single"` in `crates/omx-runtime/src/runtime_run.rs:445,464,553,573`.
- Docs status: stale-conservative
  - `docs/reference/ts-rust-parity-lanes.md:32-38` still says Rust lacks mailbox delivery, rebalance, structured verification gate, and linked-Ralph shutdown/event parity, but live Rust monitor code now implements bounded versions of those behaviors.

## Lane 2 — team runtime / tmux control-plane parity
- Verdict: PARTIAL
- Strongest proven Rust-owned slice:
  - `crates/omx-runtime/src/main.rs:39-45` exposes native `capture-pane` / `hud-watch` / `runtime-run` boundaries.
  - `crates/omx-runtime/src/tmux.rs:50,86` owns direct `send_to_pane` and `capture_pane` helpers.
- Highest-confidence remaining gap:
  - TS still owns session topology, HUD-pane restoration, readiness polling, trust-prompt dismissal, and teardown policy in `src/team/tmux-session.ts:760,975,1182,1244,1555,1565,1592,1610`.
- Docs status: truthful

## Lane 3 — HUD behavior parity
- Verdict: PARTIAL
- Strongest proven Rust-owned slice:
  - `src/cli/runtime-native.ts:43-58` can route guarded HUD launch to native `hud-watch`.
  - `crates/omx-runtime/src/hud.rs:56-77` provides a minimal native watch loop / preset parser.
- Highest-confidence remaining gap:
  - TS still owns state loading and real rendering semantics: `src/hud/state.ts:226` (`readAllState`), `src/hud/render.ts:190` (`renderHud`), `src/hud/index.ts:31-55, 74-164, 227-260` (TTY/cursor/SIGINT/non-overlap loop).
- Docs status: truthful

## Lane 4 — watcher / reply-listener / notifications parity
- Verdict: PARTIAL
- Strongest proven Rust-owned slice:
  - `crates/omx-runtime/src/reply_listener.rs:46-55, 145-223, 338-370, 514-529, 1069-1077` covers start/status/stop subcommands, Discord fetch/cursor progression, registry lookup, sanitized pane injection, and state/log persistence.
- Highest-confidence remaining gap:
  - Watcher behavior is still minimal in Rust: `crates/omx-runtime/src/watchers.rs:16-35, 143-161` mostly parses args, writes pid files, and sleeps, while richer JS semantics remain in `scripts/notify-fallback-watcher.js` and `scripts/hook-derived-watcher.js`; TS also still normalizes reply-listener start/status/stop at `src/notifications/reply-listener.ts:446-625`.
- Docs status: truthful

## Lane 5 — MCP / CLI boundary mapping and truthfulness
- Verdict: PARTIAL
- Strongest proven Rust-owned slice:
  - `src/mcp/team-server.ts:351-356` maps `omx_run_team_start` to native `runtime-run`.
  - `src/cli/runtime-native.ts:87-153` owns runtime-binary resolution / hydration.
  - `src/team/api-interop.ts:89-125` defines claim-safe worker lifecycle operations used by the worker protocol.
- Highest-confidence remaining gap:
  - Boundary mapping is clear, but truthfulness is mixed: `crates/omx-runtime/src/topology.rs:13-25` still says Rust owns team lifecycle / watcher loops broadly, which overstates actual behavior relative to live parity gaps in lanes 1-4.
- Docs status: stale-overclaiming

## Verification
- `npm run build -- --pretty false` — PASS
- `npx tsc --noEmit --pretty false` — PASS
- `node --test dist/verification/__tests__/phase1-runtime-surface-parity.test.js dist/verification/__tests__/ts-rust-parity-lanes-doc.test.js` — PASS (6 tests)
- `cargo test -p omx-runtime` — PASS (71 tests)

## Overall verdict
- Current parity state is **PARTIAL**.
- Safe claim: Rust owns multiple launch / helper / bounded runtime slices.
- Unsafe claim: full behavioral parity across team lifecycle, tmux orchestration, HUD behavior, and watcher semantics.
