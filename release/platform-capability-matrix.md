# Platform Capability Matrix

| Platform | Primary artifact | Required runtime deps | Supported launch mode | Degraded / unsupported notes | Release smoke coverage |
| --- | --- | --- | --- | --- | --- |
| Linux x64 | `omx-x86_64-unknown-linux-gnu.tar.gz` | none beyond glibc-compatible userland | native binary + tmux flows | none planned | `omx --help`, `omx version`, `omx doctor`, minimal `omx team` lifecycle |
| macOS arm64 | `omx-aarch64-apple-darwin.tar.gz` | none | native binary + tmux flows | none planned | `omx --help`, `omx version`, `omx doctor`, `omx ask` passthrough |
| macOS x64 | `omx-x86_64-apple-darwin.tar.gz` | none | native binary + tmux flows | may share smoke runner with translated macOS CI if native Intel runner unavailable | `omx --help`, `omx version`, `omx doctor` |
| Windows x64 (native) | `omx-x86_64-pc-windows-msvc.zip` | none | native binary + psmux-backed team flows | tmux-dependent flows are unsupported; docs must point users to psmux expectations | `omx.exe --help`, `omx.exe version`, `omx.exe doctor`, minimal team bootstrap without tmux |
| WSL2 x64 | linux tarball inside WSL | tmux installed in distro | native binary + tmux flows | treat as Linux runtime, but keep WSL-specific smoke note for tmux/session behavior | `omx --help`, `omx version`, minimal `omx team` lifecycle under tmux |

## Contract notes

- Every archive must unpack to a single top-level directory named after the artifact stem.
- The executable path inside each bundle is `./<bundle-stem>/omx[.exe]`.
- Any transitional npm package must only detect platform, fetch the matching bundle, and exec the bundled native binary.
- Native Windows support follows the current README split: native Windows uses psmux-oriented flows, while tmux-dependent workflows remain WSL-first.
