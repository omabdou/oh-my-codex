# oh-my-codex v0.12.1

**Patch release for team status JSON hygiene, interactive worker PID metadata integrity, and release collateral alignment**

`0.12.1` follows `0.12.0` with a narrow patch train focused on the findings resolved during the release branch review: clean machine-readable team status output, correct interactive worker PID metadata, and synchronized `0.12.1` release collateral.

## Highlights

- `omx team status --json` no longer risks stray mailbox-delivery stderr noise during stale leader-mail pruning.
- Interactive team workers now record the PID of their actual tmux pane, not a pane-index approximation.
- Release metadata and collateral are aligned to `0.12.1`.

## What’s Changed

### Fixes
- avoid duplicate bridge `MarkMailboxDelivered` calls for already-delivered leader system mail
- persist interactive worker PID metadata from the resolved pane id

### Changed
- bump release metadata from `0.12.0` to `0.12.1` across Node/Cargo manifests, changelog, and release collateral

## Verification

- `npm run build`
- `node --test dist/team/__tests__/state.test.js`
- `node --test --test-name-pattern="startTeam captures interactive worker pid from the resolved pane id" dist/team/__tests__/runtime.test.js`
- `npx biome lint src/team/state/mailbox.ts src/team/__tests__/state.test.ts src/team/runtime.ts src/team/__tests__/runtime.test.ts`
- `node --test dist/cli/__tests__/version-sync-contract.test.js`

## Remaining risk

- This patch verification is still local and targeted; it is not a full GitHub Actions matrix rerun.
- The release still touches live team/runtime surfaces, so post-release monitoring should watch team status output and interactive worker lifecycle telemetry.

## Contributors

- [@Yeachan-Heo](https://github.com/Yeachan-Heo) (Bellman)

**Full Changelog**: [`v0.12.0...v0.12.1`](https://github.com/Yeachan-Heo/oh-my-codex/compare/v0.12.0...v0.12.1)
