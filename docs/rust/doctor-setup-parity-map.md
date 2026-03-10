# Doctor / Setup Rust Parity Command Map

## Purpose

This note maps the current TypeScript `doctor` / `setup` command contract to the Rust CLI migration surface, with exact source anchors, proposed Rust module boundaries, the next low-flake parity slices to port, and the merge checklist needed before cutover.

Scope is intentionally limited to `docs/rust/**`.

## Source anchors

### CLI routing

- `src/cli/index.ts:92-95` — help text contract for `omx setup`, `omx doctor`, and `omx doctor --team`
- `src/cli/index.ts:420-425` — `setup(...)` dispatch with parsed `--scope`
- `src/cli/index.ts:443-444` — `doctor(options)` dispatch

### Doctor command

#### Install-health flow

- `src/cli/doctor.ts:47-69` — persisted/default setup-scope resolution including legacy `project-local` migration
- `src/cli/doctor.ts:72-90` — scope-aware path resolution for user vs project installs
- `src/cli/doctor.ts:93-160` — top-level `doctor` execution, nine checks, result counting, and summary text

#### Team diagnostics flow

- `src/cli/doctor.ts:169-190` — `doctor --team` header, output shape, and non-zero failure exit
- `src/cli/doctor.ts:193-346` — team issue collection and severity handling
- `src/cli/doctor.ts:244-251` — `resume_blocker`
- `src/cli/doctor.ts:253-301` — `delayed_status_lag` and `slow_shutdown`
- `src/cli/doctor.ts:305-331` — `stale_leader`
- `src/cli/doctor.ts:333-346` — `orphan_tmux_session`
- `src/cli/doctor.ts:349-359` — duplicate suppression

### Setup command

#### Scope selection + persisted state

- `src/cli/setup.ts:308-324` — requested/persisted/prompt/default scope selection
- `src/cli/setup.ts:326-339` — persisting `.omx/setup-scope.json`
- `src/cli/setup.ts:342-388` — top-level setup banner, resolved scope message, directory creation, and run summary bootstrap

#### File-tree refresh boundaries

- `src/cli/setup.ts:720-792` — prompt installation + stale prompt cleanup
- `src/cli/setup.ts:795-872` — native agent config refresh + stale native-agent cleanup
- `src/cli/setup.ts:874-910` — skill installation entry boundary
- `src/cli/setup.ts:960-1028` — `config.toml` merge/update and root-model upgrade path
- `src/cli/setup.ts:1030-1042` — notify-hook setup visibility contract

### Test anchors defining the lowest-flake parity surface

- `src/cli/__tests__/doctor-warning-copy.test.ts:29-61` — onboarding warning copy for config + MCP first-setup expectations
- `src/cli/__tests__/doctor-team.test.ts:29-225` — `doctor --team` low-flake diagnostics cases
- `src/cli/__tests__/setup-scope.test.ts:40-58` — `setup --scope` flag forms
- `src/cli/__tests__/setup-scope.test.ts:60-76` — persisted scope reuse
- `src/cli/__tests__/setup-scope.test.ts:78-102` — doctor honoring persisted project scope paths
- `src/cli/__tests__/setup-scope.test.ts:104-116` — dry-run does not persist scope
- `src/cli/__tests__/setup-scope.test.ts:118-155` — project-scope file-tree creation contract
- `src/cli/__tests__/setup-scope.test.ts:157-180` — non-interactive default to user scope
- `src/cli/__tests__/setup-scope.test.ts:182-204` — doctor does not warn about missing project AGENTS.md in user scope
- `src/cli/__tests__/setup-scope.test.ts:206-223` — legacy `project-local` scope migration

### Planning/backlog anchors

- `docs/testing/compat-harness-todo.md:16-21` — next semantic/file-tree parity slices (`setup --dry-run`, `doctor --team`, scope file-tree assertions)
- `docs/testing/rust-compat-expansion-notes.md:44-52` — setup/doctor/uninstall parity cases to compare semantically in temp dirs

## Recommended Rust command/module boundaries

### Current/near-term command ownership

| Command family | Rust entrypoint | Proposed module boundary | Notes |
|---|---|---|---|
| CLI routing | `crates/omx-cli/src/main.rs` + `crates/omx-cli/src/lib.rs` | keep subcommand parsing in `lib.rs`, dispatch in `main.rs` | Preserve help text lines from `src/cli/index.ts:92-95`. |
| doctor install health | `crates/omx-cli/src/doctor.rs` | `parse_doctor_args`, `run_doctor`, `run_install_doctor` | Already a good parity unit; keep output assembly self-contained. |
| doctor team diagnostics | `crates/omx-cli/src/doctor.rs` | nested collector helpers for issue discovery + dedupe | Reuse `omx-process` for tmux/codex/node probing. |
| setup scope + file-tree refresh | **proposed** `crates/omx-cli/src/setup.rs` | split into `scope`, `install`, `config`, and `summary` helper sections or submodules | This is the next natural port after doctor parity. |

### Concrete module split for `setup`

Recommended shape inside `crates/omx-cli/src/setup.rs`:

1. `parse_setup_args`  
   - own `--scope`, `--dry-run`, `--force`, `--verbose`
2. `resolve_setup_scope` / `persist_setup_scope`  
   - mirror `src/cli/setup.ts:308-339`
3. `resolve_setup_paths`  
   - user/project directory map shared with doctor
4. `run_setup`  
   - banner, step ordering, summary accumulation
5. `install_prompts` / `install_skills` / `install_native_agents`  
   - mirror `src/cli/setup.ts:720-910`
6. `merge_config` / `update_config_file`  
   - mirror `src/cli/setup.ts:960-1028`
7. `print_setup_summary`  
   - own exact end-user output contract

### Shared boundary worth extracting later

If setup lands soon, it is worth introducing a small shared install-path helper after parity is stable:

- **proposed helper**: `crates/omx-cli/src/install_paths.rs`
- responsibilities:
  - user vs project scope directory resolution
  - persisted scope file path
  - `.omx/state`, `.omx/logs`, `.omx/plans` roots

Do **not** extract this before setup parity exists; premature factoring makes output-parity reviews harder.

## Low-flake parity cases to port next

Priority order is behavior-first and follows the existing backlog.

### P0 next after doctor parity: setup scope and dry-run behavior

1. **`setup --scope` parsing + banner text**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:40-58`
2. **persisted scope reuse**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:60-76`
3. **doctor/setup shared persisted project-scope path handling**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:78-102`
4. **dry-run must not write `.omx/setup-scope.json`**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:104-116`
5. **legacy `project-local` migration**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:206-223`

### P1 after setup scope: file-tree semantics

6. **project-scope installation tree creation**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:118-155`
7. **non-interactive user-scope default behavior**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:157-180`
8. **user-scope AGENTS.md doctor behavior remains non-warning**  
   Contract: `src/cli/__tests__/setup-scope.test.ts:182-204`

### P2 after setup parity slice exists

9. **capture normalized `setup --dry-run` artifacts**  
   Backlog: `docs/testing/compat-harness-todo.md:18-21`
10. **add semantic temp-dir comparisons for setup/doctor/uninstall**  
    Backlog: `docs/testing/rust-compat-expansion-notes.md:44-52`

## Integration risks

### 1. Shared scope behavior can drift between commands

`doctor` and `setup` both depend on the same persisted scope contract. If Rust ports setup independently without reusing the same scope logic already used by Rust doctor, these regressions are likely:

- doctor reads one scope source while setup writes another
- legacy `project-local` migration happens in one command but not the other
- user/project path roots diverge for prompts, skills, or config

**Mitigation:** keep one Rust scope-resolution implementation used by both commands.

### 2. Output-shape drift is easy in setup

`setup` prints ordered steps, per-category summaries, and contextual warnings. Refactoring too early into generic installers increases the chance of changing:

- step numbering
- dry-run wording
- force/stale-cleanup wording
- end-of-run summary order

**Mitigation:** port one output step at a time and lock user-visible text with targeted tests before cleanup.

### 3. Config merge logic is higher risk than doctor checks

Unlike doctor, setup mutates files. The highest-risk area is `config.toml` update behavior from `src/cli/setup.ts:960-1028`, especially around:

- root model upgrade prompts
- preserving unrelated config
- inserting OMX MCP server config deterministically
- notify hook visibility without overclaiming installation success

**Mitigation:** defer broad config refactors until scope + dry-run + file-tree parity are green.

### 4. Existing Rust doctor worktree changes are ahead of docs scope

This worktree currently already contains Rust-side doctor implementation changes under `crates/omx-cli/**`. Those are outside this docs-only task, but they affect the recommended next-step ordering.

**Mitigation:** treat this note as the merge map for reconciling that existing doctor work with the next setup parity slice, not as approval to expand scope in this task.

### 5. Harness normalization is required before artifact comparisons

`setup --dry-run` and file-tree assertions involve temp paths, timestamps, and generated names.

**Mitigation:** follow `docs/testing/compat-harness-todo.md:18-21` before declaring semantic parity complete.

## Merge checklist

- [ ] Help text contract for `setup` / `doctor` remains aligned with `src/cli/index.ts:92-95`
- [ ] One Rust setup parser owns `--scope`, `--dry-run`, `--force`, and `--verbose`
- [ ] Rust setup and Rust doctor share the same persisted scope logic
- [ ] `setup --scope` forms from `src/cli/__tests__/setup-scope.test.ts:40-58` are covered in Rust tests
- [ ] persisted scope reuse from `src/cli/__tests__/setup-scope.test.ts:60-76` is covered
- [ ] `doctor` respects persisted project scope paths as in `src/cli/__tests__/setup-scope.test.ts:78-102`
- [ ] dry-run non-persistence from `src/cli/__tests__/setup-scope.test.ts:104-116` is covered
- [ ] project/user file-tree semantics from `src/cli/__tests__/setup-scope.test.ts:118-204` are covered before cutover
- [ ] legacy `project-local` migration from `src/cli/__tests__/setup-scope.test.ts:206-223` is covered
- [ ] any setup config-writing path is verified against `src/cli/setup.ts:960-1028`
- [ ] normalized compat-harness cases for `setup --dry-run` are planned before default cutover

## Recommended immediate follow-up

The next Rust implementation PR should target **setup scope + dry-run parity only**, using the test anchors above, before attempting the full setup file-tree refresh and config merge path.
