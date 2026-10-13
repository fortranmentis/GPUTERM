# GpuTerm 1.3.4-beta

## Highlights

- **Resource history across the desktop.** CPU, memory/swap, fullest-disk usage and per-GPU utilization, VRAM, temperature and power are retained in memory for one hour in 5-second buckets. Detached resource and LLM charts switch between 15 minutes and one hour.
- **Threshold alerts and desktop notifications.** The monitoring bar's bell opens GPU temperature/VRAM, idle-GPU, memory, disk and LLM-critical rules. Sustain times, hysteresis and cooldowns prevent brief spikes or repeated alerts; notification delivery falls back to in-app messages.
- **Process controls with identity checks.** CPU, memory and GPU process tables offer graceful termination and optional force termination. GpuTerm rechecks the process start marker immediately before signalling and uses the current account's permissions without sudo.
- **Keyboard-interactive / OTP sign-in.** User-started SSH connections show questions in a dialog, including separate questions for each ProxyJump hop. One-time codes are not stored as passwords; background connections report when user interaction is required.
- **Profile editing and persistent monitoring settings.** Edit saved profiles with the pencil button and keep monitoring interval, display mode and ignored filesystem types across restarts.
- **Broader AMD and SSH config support.** Use the probed `amd-smi` path when `rocm-smi` is unavailable or fails, and follow bounded, loop-safe `Include` files/globs in OpenSSH config import.

## Fixes

- Preserve the currently viewed local/remote SFTP folder when queued transfers complete, and keep canceled transfers canceled instead of showing them as errors.
- Report failed LLM refreshes, synchronize detached LLM windows when instances change, and prevent delayed forwarding status replies from overwriting another profile's state.
- Serialize the complete retry sequence for SSH channel opens so concurrent SOCKS/tunnel requests cannot receive each other's destination channels; back off idle forwarding loops rather than polling every 5 ms.
- Probe idle SSH links for a server reply and probe after wake; retain a keepalive fallback for servers that never answer the initial probe.
- Save the password entered on New → Save into the encrypted vault before writing the profile; keep the long profile form's title visible.
- Keep 15m/1h chart controls from collapsing the chart grid and keep alert checkboxes at their intended size.

## Validation

- **264 frontend tests pass** across 36 files, covering SFTP folder preservation/cancellation, profile editing, forwarding status races, detached LLM changes, history charts, alerts and authentication/process dialogs.
- **407 Rust tests pass**, with 2 environment-dependent tests ignored. Coverage includes AMD JSON layouts, SSH config Includes, probe outcomes, channel-open/idle behavior, vault-first profile saves, history bucketing/retention, alert transitions, OTP prompt classification and process identity/signalling scripts.
- TypeScript checking and the Vite production build pass. ESLint reports 0 errors and 14 existing React Hook warnings. Clippy passes across all targets/features with warnings denied; `git diff --check` passes.
- GitHub Actions builds native Windows NSIS `.exe`, Debian `.deb`, Apple Silicon `.dmg`, and publishes `SHA256SUMS.txt` from the installer assets. Windows-specific local telemetry and Claude status-line tests run on the Windows build host.
- The macOS job signs nested Mach-O files and code containers inside-out, applies the final deep ad-hoc signature, verifies with `codesign --verify --deep --strict`, then creates and mounts the DMG and verifies its enclosed app before upload.

## Notes

- Live SSH OTP and dead-link detection remain unverified against an actual test server. The reply probe starts after 15 seconds of inactivity and waits up to 8 seconds (approximately 23 seconds total); initial probe timeout falls back to keepalives/TCP detection, so detection timing depends on server behavior.
- `amd-smi metric/static/process --json` support is fixture-tested; real AMD server output is still needed for hardware validation. JSON layouts vary by ROCm version, unavailable fields remain unavailable, and fan speed/clock are not collected.
- macOS process signalling was exercised by the feature author using a real `sleep` process, including mismatched identity and permission refusal; this release verification ran the automated suite. Windows/Linux process controls remain unit-tested. Rechecking the marker reduces PID-reuse risk but does not make checking and signalling atomic.
- OTP-required hosts cannot always serve background telemetry, SFTP, forwarding or LLM tunnels because those create separate connections without a dialog. Reconnect stops when user interaction is required instead of guessing codes.
- OS notification delivery requires a suitable installed build and system permission/settings. In-app messages remain available when system delivery fails. Resource history/alert state is session-scoped and resets on reconnect/restart; thresholds are global and history is never written to disk.
- Existing profiles and encrypted credentials remain compatible with 1.3.3-beta. Newly persisted monitoring/alert settings use defaults when fields are absent; the existing encrypted vault must be unlocked to save a password.
- This is a beta prerelease. Windows is not signed by a trusted publisher. macOS is fully ad-hoc signed for integrity, but not Developer ID signed or notarized. The DMG is Apple Silicon (`aarch64`) only; Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.3-beta...v1.3.4-beta
