# Notify-hook / tmux compatibility quarantine

Goal: make tmux/Node notify-hook behavior explicitly compatibility-only for the cargo/native-first milestone. Nothing in this inventory is required for normal product verification.

Classification legend:
- PORT: port to Rust/native if still needed for product authority
- COMPAT-ONLY: keep for optional tmux/Node flows; not part of product proof
- DELETE: remove after cutover when no longer referenced

## Inventory and provisional classification (v0)

scripts/notify-hook/
- team-worker.js — COMPAT-ONLY
- team-leader-nudge.js — COMPAT-ONLY
- tmux-injection.js — COMPAT-ONLY
- team-dispatch.js — COMPAT-ONLY
- auto-nudge.js — COMPAT-ONLY
- payload-parser.js — COMPAT-ONLY
- process-runner.js — COMPAT-ONLY
- state-io.js — COMPAT-ONLY
- operational-events.js — COMPAT-ONLY
- linked-sync.js — COMPAT-ONLY
- log.js — COMPAT-ONLY
- utils.js — COMPAT-ONLY
- visual-verdict.js — COMPAT-ONLY

src/notifications/
- tmux.ts — COMPAT-ONLY
- tmux-detector.ts — COMPAT-ONLY
- (other modules: dispatcher.ts, notifier.ts, formatter.ts, reply-listener.ts, config.ts, etc.) — COMPAT-ONLY for 0.x cargo-only milestone; candidate PORT or DELETE later based on actual native runtime needs

src/hooks/extensibility/
- sdk.ts — COMPAT-ONLY (dev tooling; not product-authoritative for the migration milestone)

## Fencing approach (initial)
- Documentation: this file + README mark tmux/notify surfaces as compatibility-only.
- Runtime env gates:
  - DISABLED by default. Enable explicitly with `OMX_COMPAT_TMUX=1|true|yes`.
  - Force-disable with `OMX_NO_TMUX=1` (overrides any opt-in).
  - Injection path: `scripts/notify-hook/tmux-injection.js` short-circuits unless opt-in is present; logs `injection_skipped` with reason.
  - SDK path: `src/hooks/extensibility/sdk.ts`'s `tmux.sendKeys()` requires opt-in and returns `no_backend` when fenced.
  - Detector path: `src/notifications/tmux-detector.ts` returns `false` from `isTmuxAvailable()` unless `OMX_COMPAT_TMUX` is truthy.
- Tests: any JS tests for these surfaces live under a compat-only lane and are never a product gate.

### Quick enable/disable

```bash
# Enable tmux compatibility for this shell
export OMX_COMPAT_TMUX=1

# Force-disable (takes precedence)
export OMX_NO_TMUX=1
```

### Sample .omx/tmux-hook.json

```json
{
  "enabled": true,
  "target": { "type": "pane", "value": "%42" },
  "allowed_modes": ["ralph", "ultrawork", "team"],
  "prompt_template": "Continue from current mode state. [OMX_TMUX_INJECT]",
  "marker": "[OMX_TMUX_INJECT]",
  "skip_if_scrolling": true
}
```

Logs are written under `.omx/logs/` with `notify-hook-YYYY-MM-DD.jsonl` entries.

## Exit criteria for this lane
- No tmux/notify JS surface is ambiguously product-authoritative.
- Native-first product behavior and verification do not require the above JS files.
- When a file becomes unused for compat, mark as DELETE and remove in a follow-up PR.
