# Native Release Transition

This directory captures the release-path contract for the OMX Rust migration.

## Goals

- make native binaries the primary release artifact
- keep any npm package strictly transitional (downloader/launcher only)
- define the platform matrix and smoke checks before cutover

## Transition phases

1. **Baseline**
   - existing Node package/bin contract stays green
   - compatibility harness records current CLI behavior
2. **Transition**
   - native bundles are produced for every supported platform
   - npm, if still published, only locates/downloads/launches the native binary
   - no new CLI logic runs through Node in published install flows
3. **Cutover**
   - release approval requires native-only smoke coverage on each supported platform
   - install docs point to the native bundles as the supported path
4. **Cleanup**
   - remove `bin/omx.js`, `dist/`, and TypeScript release-path CI only after verifier sign-off

## Source-of-truth artifacts

- `release/native-bundle-contract.json` — expected bundle names, archive layout, and smoke commands
- `release/platform-capability-matrix.md` — supported/degraded behavior by platform

## Release gates

A native cutover is blocked unless all of the following are true:

- every bundle in `native-bundle-contract.json` exists
- each bundle contains exactly one `omx` executable at the documented path
- platform smoke commands pass on Linux, macOS, Windows native, and WSL2
- any npm package still in circulation behaves only as a native launcher/downloader shim
