# Rust setup integration handoff

## Purpose

This handoff isolates the current **setup integration blocker** for the Rust CLI port, gives the merge checklist needed to land the existing doctor-focused work safely, and defines the next artifact-parity steps for `omx setup`.

Scope is intentionally limited to `docs/rust/**`.

## Current blocker

The Rust CLI already advertises `omx setup` in top-level help, and Rust `doctor` already consumes the persisted setup-scope contract, but the Rust entrypoint still has **no setup dispatch path**.

### Evidence

- `crates/omx-cli/src/lib.rs:15-16` — help text still promises `omx setup`
- `crates/omx-cli/src/lib.rs:77-97` — `parse_args(...)` returns `CliAction::Unsupported` for `setup`
- `crates/omx-cli/src/doctor.rs:147-150` — Rust doctor already reports resolved setup scope
- `src/cli/__tests__/setup-scope.test.ts:40-260` — Node contract defines the missing setup behavior that Rust still needs

### Why this blocks integration

Merging doctor parity without a clear setup handoff is safe, but wiring `setup` carelessly would create an inconsistent CLI surface:

1. help text claims setup support
2. doctor reads/warns about setup-managed artifacts
3. Rust cannot yet create or refresh those artifacts
4. artifact parity needs semantic file-tree assertions, not just byte-exact stdout fixtures

## Safe merge boundary for the current Rust work

The current merge is safe **only if it is treated as doctor-first parity**, not full setup parity.

### Merge assumptions

- Rust `doctor` remains the only setup-aware command with implemented behavior.
- Rust `setup` does **not** get partially wired behind unstable file-writing logic.
- follow-up setup work reuses the same persisted-scope contract already implemented for doctor.
- semantic setup parity remains tracked in docs/planning artifacts until the compat harness can assert file trees.

## Merge checklist for the setup integration blocker

- [ ] Keep the merge description explicit that Rust doctor parity landed **without** Rust setup parity.
- [ ] Preserve the existing doctor persisted-scope behavior as the source of truth for setup/doctor shared scope resolution.
- [ ] Do not add a partial Rust `setup` command unless the parser, persistence, and artifact roots land together.
- [ ] Confirm the next setup owner starts from `src/cli/__tests__/setup-scope.test.ts`, not from ad hoc manual behavior.
- [ ] Confirm artifact parity planning still points to normalized file-tree assertions (`docs/testing/rust-compat-next-slices.md`, `docs/testing/rust-parity-next-slices.json`).
- [ ] Keep config-writing parity (`src/cli/setup.ts:960-1028`) out of the first setup PR unless earlier scope/file-tree slices are already green.
- [ ] Treat AGENTS.md refresh behavior as part of setup parity, not as optional cleanup.
- [ ] Preserve the legacy `project-local` → `project` migration contract in any shared scope helper.

## Recommended next implementation slice

The next Rust PR should be **setup scope + artifact-root parity**, not full setup mutation parity.

### Target behavior to land next

1. parse `setup --scope project|user` and `--scope=...`
2. reuse persisted `.omx/setup-scope.json` when `--scope` is omitted
3. keep `--dry-run` from persisting the scope file
4. preserve legacy `project-local` migration warning
5. write artifacts to the correct root for `project` vs `user`
6. keep doctor reading the exact same resolved scope/path logic

## Artifact parity steps after the blocker is cleared

### Step 1 — scope parser + persisted-scope reuse

Anchor tests:

- `src/cli/__tests__/setup-scope.test.ts:40-76`
- `src/cli/__tests__/setup-scope.test.ts:206-223`

Rust acceptance targets:

- `setup --scope project`
- `setup --scope=user`
- persisted scope reuse when `--scope` is omitted
- migration warning for legacy `project-local`
- no persistence on `--dry-run`

### Step 2 — project/user artifact-root parity

Anchor tests:

- `src/cli/__tests__/setup-scope.test.ts:78-204`
- `src/cli/__tests__/setup-skills-overwrite.test.ts`

Rust acceptance targets:

- project scope writes under repo-local `.codex/`, `.agents/`, `.omx/`
- user scope writes under `CODEX_HOME` / user home
- user scope leaves project `AGENTS.md` unchanged
- project scope refreshes `AGENTS.md` with current OMX template behavior
- doctor resolves project-vs-user artifact locations from the shared scope helper

### Step 3 — semantic compat-harness promotion

Planning anchors:

- `docs/testing/rust-compat-next-slices.md`
- `docs/testing/rust-parity-next-slices.json`

Needed runner capabilities before declaring setup parity:

- temp-path normalization
- path-separator normalization
- targeted file-tree assertions
- stable checks for `.omx/setup-scope.json`, `.codex/config.toml`, `.omx/agents/*.toml`, prompt/skill trees

### Step 4 — config merge / destructive refresh behavior

High-risk anchors:

- `src/cli/setup.ts:720-1028`
- `src/cli/__tests__/setup-skills-overwrite.test.ts`

Land only after steps 1-3 are green:

- stale prompt cleanup
- native-agent refresh
- skill overwrite/refresh behavior
- `config.toml` merge/update flow
- notify-hook reporting
- force-mode destructive maintenance wording

## Handoff notes for the next owner

### Reuse, do not re-invent

Use one shared Rust scope helper for both commands:

- resolve persisted scope
- migrate legacy scope values
- map scope to codex/skills/agents/config roots
- expose whether the scope came from CLI, persisted state, or default resolution

### Avoid these failure modes

- parsing `setup` separately from doctor scope logic
- persisting scope during `--dry-run`
- implementing project artifacts before user-scope roots are tested
- refactoring file installers before output/file-tree parity is locked
- mixing config-merge work into the first setup-scope PR

## Ready-to-hand-off summary

The blocker is not missing design information; it is missing a **single coherent Rust setup parity slice** that joins parser + persisted scope + artifact roots. Merge the current doctor work as doctor-first parity, then hand the next owner a setup PR scoped to `setup-scope.test.ts` plus semantic artifact assertions before any config-merge expansion.
