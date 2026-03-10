# PR Draft: Bootstrap Rust CLI parity harness and initial omx command-family port

## Target branch
`dev`

## Summary
This draft PR starts the brownfield refactor of `omx` from the current TypeScript/Node runtime toward a Rust-native CLI.

It does **not** complete the full CLI port. Instead, it establishes the migration foundation and ports the first low-risk, behavior-locked slices behind executable parity checks.

Ported and parity-verified in this slice:
- `omx --help`
- `omx version`
- `omx ask ...`
- `omx reasoning`
- `omx reasoning <mode>`
- `omx reasoning --help`
- invalid reasoning-mode handling

This PR also adds the first Rust process/platform scaffolding needed for later migration of `doctor`, `setup`, `launch`, `team`, and other runtime-heavy command families.

## Changes
- add a Rust workspace and initial crates:
  - `crates/omx-cli`
  - `crates/omx-process`
- add a black-box compatibility harness that can target either:
  - current Node/JS CLI, or
  - Rust binary via `OMX_COMPAT_TARGET`
- add checked-in parity fixtures for:
  - help
  - version
  - ask passthrough
  - reasoning no-config / set / current-value behavior
- port first Rust CLI slices with parity:
  - top-level help output
  - version output
  - ask provider-advisor passthrough semantics
  - reasoning command baseline behavior
- add Rust process/platform scaffolding for future migration:
  - process bridge abstraction
  - Windows command resolution / wrapper logic
  - tmux/process planning helpers
- add migration/reference docs:
  - process bridge design
  - platform capability matrix
  - setup/doctor parity notes
  - release/native transition notes
  - parity expansion backlog

## Validation
- [x] `cargo fmt --check`
- [x] `cargo test`
- [x] `cargo check`
- [x] `cargo clippy --all-targets --all-features -- -D warnings`
- [x] `npm run build`
- [x] `npm run lint`
- [x] `npm run test:compat:node`
- [x] `npm run test:compat:rust`
- [x] `node scripts/compat/run-rust-parity.js --case reasoning-help --case reasoning-invalid`

## Scope of parity achieved in this PR

### Ported and verified now
- `help`
- `version`
- `ask`
- `reasoning`

### Not yet ported
- `setup`
- `doctor` / `doctor --team` final integrated parity
- `agents-init` / `deepinit`
- `team`
- `ralph`
- `hud`
- `hooks`
- `tmux-hook`
- `status`
- `cancel`
- launch/runtime/tmux orchestration
- notifications
- MCP servers
- uninstall/update/star flows

## Why this is good
- locks a real executable parity harness before broad refactoring
- proves the Rust binary can already match multiple existing CLI contracts
- creates a reusable process/platform layer for later runtime migration
- keeps the migration behavior-first instead of attempting a risky big-bang rewrite

## Risks / remaining follow-up
- artifact authority is still somewhat duplicated across `docs/rust/*` and `release/*`
- broader command families remain Node-only
- runtime-heavy surfaces (`team`, launch/tmux, notifications, MCP) are still the highest-risk migration areas
- current branch also has follow-up WIP for `doctor`/`setup` parity that may be better split into later PRs

## Backward compatibility
This PR is intended to be backward-compatible for the currently shipped CLI:
- the existing Node/JS entrypoint remains intact
- Rust is introduced as an incremental migration path, not a release-path replacement
- current user-visible behavior is preserved through parity checks rather than changed in place

## Related
Closes #
