# Compatibility Harness TODO

## Guiding rule

Expand parity coverage in behavior-first order: keep the current byte-exact baseline green, then add the next lowest-flake command families before any cutover work.

## Priority queue

### P0 — extend the existing byte-exact baseline

1. Add `reasoning --help` / invalid-mode fixtures called out in the test spec baseline-capture section.
2. Add representative invalid-arg / missing-value fixtures for top-level CLI routing.
3. Add one stable `ask --print` / `ask --prompt` fixture so provider flag parsing is covered at the same black-box layer as passthrough.
4. Add a fixture-backed package/bootstrap failure case that captures the current JS-launcher error path before native packaging work changes it.

### P1 — first semantic/file-tree parity slices

1. Capture `setup --dry-run` and `doctor --team` baseline artifacts in temp directories.
2. Define normalization rules for timestamps, temp paths, and randomized team/session names before any semantic diff harness is added.
3. Add file-tree assertions for setup/user-vs-project scope behavior.
4. Add uninstall cleanup snapshots once setup file-tree parity exists.

### P2 — platform + runtime harnesses

1. Publish a platform capability inventory mapped to existing contract sources for Linux, macOS, Windows native, and WSL.
2. Add a fixture/stub strategy for tmux vs psmux availability and degraded-mode expectations.
3. Build black-box team lifecycle smoke cases around start/resume/shutdown plus state/event file assertions.
4. Add platform-command parity cases that run at the process boundary rather than only through source-level tests.

### P3 — native release and cutover gates

1. Add release-artifact smoke checks for a direct binary target with no Node runtime.
2. Split transitional npm-wrapper checks from native-primary release checks so phase boundaries stay explicit.
3. Require a per-command-family parity mode declaration (byte-exact / normalized / semantic) before each new Rust cutover.
4. Keep JS launcher fixtures only until native artifact checks are green and verifier-approved.

## Definition of done for the next harness expansion PR

- The new slice is explicitly mapped to a PRD/test-spec contract family.
- The parity mode is declared up front.
- The Node baseline stays green under `npm run test:compat:node`.
- Any new normalization rule is checked in and justified; no implicit “close enough” diffs.
