# oh-my-codex v0.10.4

**7 PRs + 1 direct commit in the release window**

`0.10.4` is a fast-follow patch release after `0.10.3`. The release window began with the `0.10.3` tag at `2026-03-18 02:53 UTC`; 8 non-merge commits landed afterward, capped by the `#941` merge commit (`ceccb94`) that disables implicit OMX cleanup on launch.

## Highlights

### Ralph / Ralphthon hardening

- Ralph now includes a mandatory deslop pass in its workflow
- Ralphthon state readers are more resilient and no longer perform a redundant bootstrap call

### Team tmux stability

- Team layouts now auto-reconcile after terminal resize events
- Detached tmux sessions are cleaned up when the leader pane exits

### Autoresearch + notification flow polish

- Deep-interview can now launch autoresearch through the split-pane run path
- Wrapped recent-output blocks are preserved in notifications

### Safer launch behavior

- Automatic stale-session cleanup is disabled during normal launch
- Destructive cleanup is now explicit through `omx cleanup`

## What's Changed

### Features
- feat: add mandatory deslop pass to ralph workflow ([#932](https://github.com/Yeachan-Heo/oh-my-codex/pull/932))
- feat(team): auto-reconcile tmux team layout on resize ([#934](https://github.com/Yeachan-Heo/oh-my-codex/pull/934))
- feat: launch autoresearch from interview via split-pane run path ([#933](https://github.com/Yeachan-Heo/oh-my-codex/pull/933))

### Fixes
- fix: harden ralphthon state readers and remove redundant bootstrap call ([#935](https://github.com/Yeachan-Heo/oh-my-codex/pull/935))
- fix: kill detached tmux session when leader pane exits ([#937](https://github.com/Yeachan-Heo/oh-my-codex/pull/937))
- fix(notifications): preserve wrapped recent-output blocks ([#939](https://github.com/Yeachan-Heo/oh-my-codex/pull/939))
- fix: disable implicit OMX cleanup on launch ([#941](https://github.com/Yeachan-Heo/oh-my-codex/pull/941))

### Internal
- chore: target dependabot PRs to dev branch (`ca33f8e`)

## Contributors

- Bellman
- [@Yeachan-Heo](https://github.com/Yeachan-Heo)
- [@lifrary](https://github.com/lifrary) (SEUNGWOO LEE)

## Local release verification checklist

Run before tagging / publishing:

- `node scripts/check-version-sync.mjs --tag v0.10.4`
- `npm run build`
- `npm run check:no-unused`
- `npm test`

**Full Changelog**: [`v0.10.3...v0.10.4`](https://github.com/Yeachan-Heo/oh-my-codex/compare/v0.10.3...v0.10.4)
