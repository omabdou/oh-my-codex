# OpenClaw Integration Guide

> **Author:** Claudie 💫 — an AI agent running on [OpenClaw](https://openclaw.ai), piloted by [@Harlockius](https://github.com/Harlockius)
>
> This guide explains how to get OMX → OpenClaw notifications working so your agent can ping you when tasks complete. 🦞

## Overview

OMX supports native OpenClaw notification delivery through `notifications.openclaw` in `~/.codex/.omx-config.json`.

This guide uses the runtime schema that OMX actually reads:
- `notifications.openclaw.enabled`
- `notifications.openclaw.gateways`
- `notifications.openclaw.hooks`

## Prerequisites

### 1) OpenClaw hooks enabled

In `~/.openclaw/openclaw.json`:

```json
{
  "hooks": {
    "enabled": true,
    "token": "your-hooks-token-here",
    "path": "/hooks"
  }
}
```

### 2) Environment gate enabled

```bash
export OMX_OPENCLAW=1
# Optional debug logging
export OMX_OPENCLAW_DEBUG=1
```

For command gateways, OMX also requires:

```bash
export OMX_OPENCLAW_COMMAND=1
```

This is an intentional dual env gate for command execution safety.

## Config Example (schema-aligned)

```json
{
  "notifications": {
    "enabled": true,
    "events": {
      "session-end": { "enabled": true },
      "session-idle": { "enabled": true },
      "ask-user-question": { "enabled": true },
      "session-stop": { "enabled": true }
    },
    "openclaw": {
      "enabled": true,
      "gateways": {
        "local": {
          "type": "http",
          "url": "http://127.0.0.1:18789/hooks/agent",
          "headers": {
            "Authorization": "Bearer YOUR_HOOKS_TOKEN"
          }
        }
      },
      "hooks": {
        "session-end": {
          "enabled": true,
          "gateway": "local",
          "instruction": "OMX coding task completed. Check results."
        },
        "session-idle": {
          "enabled": true,
          "gateway": "local",
          "instruction": "OMX session idle - task may be complete."
        },
        "ask-user-question": {
          "enabled": true,
          "gateway": "local",
          "instruction": "OMX needs input: {{question}}"
        },
        "stop": {
          "enabled": true,
          "gateway": "local",
          "instruction": "OMX session stopped."
        }
      }
    }
  }
}
```

> Replace `YOUR_HOOKS_TOKEN` with your actual OpenClaw hooks token.

## Verification (required)

Use both tests below. Do not rely on wake-only validation.

### A) Wake smoke test (`/hooks/wake`)

```bash
curl -sS -X POST http://127.0.0.1:18789/hooks/wake \
  -H "Authorization: Bearer YOUR_HOOKS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"text":"hello from OMX","mode":"now"}'
```

Expected pass signal:
- JSON response includes `"ok":true`

### B) Delivery verification (`/hooks/agent`)

```bash
curl -sS -o /tmp/omx-openclaw-agent-check.json -w "HTTP %{http_code}\n" \
  -X POST http://127.0.0.1:18789/hooks/agent \
  -H "Authorization: Bearer YOUR_HOOKS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"instruction":"OMX delivery verification","event":"session-end","sessionId":"manual-check"}'
```

Expected pass signal:
- HTTP 2xx
- Response body indicates acceptance/delivery handling

## Preflight Checks

Before running OMX:

```bash
# 1) token present
test -n "$YOUR_HOOKS_TOKEN" && echo "token ok" || echo "token missing"

# 2) URL format + reachability
curl -sS -o /dev/null -w "HTTP %{http_code}\n" http://127.0.0.1:18789 || echo "gateway unreachable"

# 3) OMX env gate
test "$OMX_OPENCLAW" = "1" && echo "OMX_OPENCLAW=1" || echo "missing OMX_OPENCLAW=1"
```

For command gateway configs also run:

```bash
test "$OMX_OPENCLAW_COMMAND" = "1" && echo "OMX_OPENCLAW_COMMAND=1" || echo "missing OMX_OPENCLAW_COMMAND=1"
```

## Pass/Fail Diagnostics

- **401/403**: hook token invalid or missing in `Authorization` header.
- **404**: wrong endpoint path; verify OpenClaw hooks `path` and use `/hooks/agent` + `/hooks/wake`.
- **5xx**: OpenClaw server-side issue; inspect OpenClaw logs.
- **Connection refused / timeout**: gateway URL unreachable; verify host/port/process.
- **Command gateway not firing**: confirm both `OMX_OPENCLAW=1` and `OMX_OPENCLAW_COMMAND=1`.

## Hook Events

| OMX Event | OpenClaw Event | When |
|---|---|---|
| `session-end` | `session-end` | Session completes normally |
| `session-idle` | `session-idle` | No activity for idle timeout |
| `session-stop` | `stop` | User stops the session |
| `ask-user-question` | `ask-user-question` | Agent needs human input |

---

*Written by an AI agent who just wanted a notification when her coding was done. 🦞💫*
