# Ralph Persistence Release Gate

This checklist is a hard gate for Ralph persistence rollout.
CI/release validation MUST fail when any required scenario below is missing or failing.

## Rollout policy (fixed for this port)

- Release N: behind explicit opt-in flag `OMX_RALPH_PERSISTENCE_PORT=1`.
- Release N+1 default enablement decision only after:
  - parity drift remains clean,
  - cancellation metrics show no cross-session corruption,
  - gate scenarios V1–V10 stay green in CI.

## Verification matrix gate

| ID | Scenario | Required evidence | Status |
|---|---|---|---|
| V1 | Session-scoped Ralph lifecycle | `src/cli/__tests__/session-scoped-runtime.test.ts` + `src/mcp/__tests__/trace-server.test.ts` | [x] |
| V2 | Root fallback compatibility (HUD) | `src/hud/__tests__/state.test.ts` | [x] |
| V3 | Canonical PRD/progress precedence + migration | `src/ralph/__tests__/persistence.test.ts` | [x] |
| V4 | Phase vocabulary enforcement | `src/mcp/__tests__/state-server-ralph-phase.test.ts` | [x] |
| V5 | Cancel standalone Ralph terminalization | `src/cli/__tests__/session-scoped-runtime.test.ts` | [x] |
| V6 | Cancel Ralph linked mode behavior | `src/cli/__tests__/session-scoped-runtime.test.ts` | [x] |
| V7 | Team-linked terminal propagation | `src/hooks/__tests__/notify-hook-linked-sync.test.ts` | [x] |
| V8 | Cross-session safety | `src/cli/__tests__/session-scoped-runtime.test.ts` + `src/mcp/__tests__/trace-server.test.ts` | [x] |
| V9 | Upstream parity evidence | `docs/reference/ralph-upstream-baseline.md` + `docs/reference/ralph-parity-matrix.md` | [x] |
| V10 | CI/release gate enforcement | `.github/workflows/ci.yml` + `src/verification/__tests__/ralph-persistence-gate.test.ts` | [x] |

### Explicit scenario checklist

- [x] V1 Session-scoped Ralph lifecycle
- [x] V2 Root fallback compatibility (HUD)
- [x] V3 Canonical PRD/progress precedence + migration
- [x] V4 Phase vocabulary enforcement
- [x] V5 Cancel standalone Ralph terminalization
- [x] V6 Cancel Ralph linked mode behavior
- [x] V7 Team-linked terminal propagation
- [x] V8 Cross-session safety
- [x] V9 Upstream parity evidence
- [x] V10 CI/release gate enforcement

## Required docs

- `docs/contracts/ralph-state-contract.md`
- `docs/contracts/ralph-cancel-contract.md`
- `docs/reference/ralph-upstream-baseline.md`
- `docs/reference/ralph-parity-matrix.md`

## Release note requirements

Every release touching Ralph persistence MUST mention:

1. session-authoritative scope policy,
2. legacy compatibility window (`.omx/prd.json` and `.omx/progress.txt`),
3. opt-in flag behavior for the current release.


## PRD policy proof requirements

The release gate MUST include explicit proof for the following contract boundaries:

- Policy normalization marker: `src/mcp/__tests__/state-server-ralph-phase.test.ts` — `normalizes invalid prd_policy to required and records ralph_prd_policy_normalized_from`
- Missing-policy default: `src/mcp/__tests__/state-server-ralph-phase.test.ts` — `defaults missing prd_policy to required`
- Collision suffix behavior: `src/ralph/__tests__/persistence.test.ts` — `adds -1 and -2 suffixes when scaffold filename collisions occur`
- Opt-out scaffold suppression: `src/ralph/__tests__/persistence.test.ts` — `does not scaffold PRD when prd_policy is opt_out`
- CLI boundary arg stripping proof: `src/cli/__tests__/ralph-command.test.ts` — `strips --no-prd before launchWithHud command invocation`
- Opt-out state persistence proof: `src/cli/__tests__/ralph-command.test.ts` — `persists prd_policy=opt_out when --no-prd is provided`
- Opt-out non-bypass gate proof: `src/hooks/__tests__/agents-overlay.test.ts` — `remains BLOCKED when test-spec is missing even with prd_policy opt_out`
- Guidance text proof: `src/hooks/__tests__/keyword-detector.test.ts` — `asserts explicit non-bypass text appears in generated guidance`
