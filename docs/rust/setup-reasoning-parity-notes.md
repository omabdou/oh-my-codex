# Rust CLI parity notes: setup scope + reasoning set/get

## Why this note exists

Task 2 allowed either:

- `crates/omx-cli/**` changes that do not overlap worker-1, or
- isolated notes under `docs/rust/**` when overlap risk exists.

Worker-1 owns `omx doctor` parity in `crates/omx-cli/**`. The current Rust CLI entrypoint surface for any future `setup` and `doctor` handling is shared (`src/lib.rs`, `src/main.rs`), so this lane records the non-overlapping design/test plan instead of creating merge-conflicting code changes.

## Current Rust status

### Reasoning command

Already implemented in `crates/omx-cli/src/reasoning.rs`:

- `omx reasoning` with no value reads `CODEX_HOME/config.toml`
- missing config prints the "not set" message
- `omx reasoning <mode>` writes `model_reasoning_effort = "<mode>"`
- repeated reads return `Current model_reasoning_effort: <mode>`
- top-level TOML insertion happens before the first table when needed

Backed today by:

- crate unit tests in `crates/omx-cli/src/reasoning.rs`
- TS compat contract in `src/compat/__tests__/reasoning-contract.test.ts`

### Setup command

Not yet implemented in the Rust scaffold.

Node baseline behavior is currently defined by:

- `src/cli/setup.ts`
- `src/cli/__tests__/setup-scope.test.ts`

## Required parity slices

## 1) Reasoning set/get contract must stay byte-compatible

Keep the existing Rust behavior aligned with these baseline flows:

1. `omx reasoning` when `<CODEX_HOME>/config.toml` does not exist
2. `omx reasoning high` writes the config file
3. `omx reasoning` after the write reports the current value
4. invalid mode exits non-zero with the expected validation message
5. `omx reasoning --help` returns top-level help output

### Suggested Rust verification additions

When the crate entrypoint work resumes, add/keep these checks:

- byte-exact parity against `src/compat/fixtures/reasoning/*.txt`
- a temp-dir test proving insert-before-first-table behavior
- a temp-dir test proving existing unrelated top-level keys are preserved
- a temp-dir test proving `CODEX_HOME` overrides `HOME/.codex`

## 2) Setup scope parity is the next safe Rust expansion after doctor ownership settles

The setup scope contract to preserve from Node is:

- `--scope project` and `--scope=project` are accepted
- `--scope user` and `--scope=user` are accepted
- omitted scope reuses `.omx/setup-scope.json` when present
- legacy persisted `project-local` migrates to `project` with a warning
- `--dry-run` must not persist `.omx/setup-scope.json`
- project scope writes prompts/skills/config/native-agents under the repo cwd
- user scope writes under the user home / `CODEX_HOME`
- user scope leaves project `AGENTS.md` unchanged
- `doctor` must resolve paths using the persisted setup scope

### Recommended Rust implementation order

1. Pure parser/state helpers for persisted scope resolution
2. dry-run-safe scope persistence behavior
3. path resolution helpers for user vs project scope
4. artifact-writing setup implementation
5. doctor/setup shared scope-resolution wiring
6. compat harness coverage for setup output plus filesystem artifacts

### Recommended Rust tests

Mirror the Node cases from `src/cli/__tests__/setup-scope.test.ts` with Rust-focused coverage for:

- separate-arg and equals-arg parsing
- persisted scope reuse
- legacy scope migration warning
- no persistence on dry-run
- project-scope artifact roots
- user-scope artifact roots
- doctor using persisted scope paths once the doctor port lands

## Merge-safe handoff

Once worker-1 lands the doctor entrypoint shape, the next Rust owner can use this note to add:

- shared scope-resolution helpers usable by both `setup` and `doctor`
- parity tests first, then command wiring
- compat-harness cases for semantic setup verification (stdout + created files)
