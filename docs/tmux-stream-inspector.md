# tmux stream inspector proposal

## Why

OMX currently issues a large number of tmux CLI calls in hot paths:
- capture-pane
- list-panes
- display-message
- send-keys guard checks around those reads

The expensive part is not only tmux itself, but the repeated Node child-process spawn + pane capture + string parsing loop.

## Current hot paths

### Team runtime
- src/team/tmux-session.ts
  - waitForWorkerReady() repeatedly polls capture-pane
  - capturePaneAsync()
  - multiple list-panes / display-message helpers

### Team idle / nudge
- src/team/idle-nudge.ts
  - capturePane() on each scan
  - isPaneIdle() = capture + parse loop

### Notifications / reply injection
- src/notifications/tmux-detector.ts
- src/notifications/tmux.ts
- scripts/tmux-hook-engine.js
- scripts/notify-hook/tmux-injection.js
- scripts/notify-hook/auto-nudge.js
- scripts/notify-hook/team-leader-nudge.js

## Proposal

Introduce a Rust helper / sidecar, tentatively omx-tmux-inspect.

### Mode A: snapshot
Return cached pane/session state as JSON.

Example:
- omx-tmux-inspect snapshot --session omx-team-foo
- omx-tmux-inspect pane --id %3

### Mode B: watch
Run one long-lived inspector loop that watches tmux sessions and maintains:
- pane current command
- pane dead/alive state
- recent tail buffer
- shell detection
- ready/idle heuristics
- marker presence / injection guard signals

Node should query this cached state rather than directly calling tmux capture-pane in every hot path.

## Expected wins

- fewer tmux child-process spawns
- lower latency in worker readiness polling
- cheaper multi-pane idle detection
- centralized shell/idle heuristics
- easier perf instrumentation

## Incremental adoption plan

1. add Rust helper binary + simple JSON snapshot output
2. switch waitForWorkerReady() to inspector-backed reads
3. switch idle-nudge.ts to inspector state
4. switch notify/reply helpers to inspector state
5. consider deeper event-driven integration later

## Non-goals

- replacing all tmux writes immediately
- changing team runtime semantics in the same PR
- full tmux control-mode migration in the first iteration
