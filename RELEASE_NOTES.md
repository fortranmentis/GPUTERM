# GpuTerm 1.2.2-beta

## Highlights

- **Sortable remote SFTP listings.** Click Name, Type, Size, or Modified to sort the remote directory. Repeated clicks reverse the direction, folders remain first, missing metadata stays last, and the selected order survives navigation and refreshes.
- **Clear decimal file sizes throughout SFTP.** Remote files, local files, and transfer progress now use SI `B / KB / MB / GB / TB` units, while RAM and process telemetry retain their binary IEC units.
- **Claude limits on macOS no longer require Python.** The one-click setup installs a privacy-filtering JavaScript for Automation helper powered by macOS's built-in `osascript`.
- **More reliable local Windows AI quota reads.** Codex and AGY probes reuse the native executable path observed in the running process and fall back through `cmd.exe` for npm `.cmd` shims.
- **Long macOS disk paths stay inside the telemetry card.** Mount paths such as CoreSimulator volumes are ellipsized without hiding the usage percentage; hover text exposes the full path.

## Fixes

- Fixed macOS Claude Code cards detecting the process but showing no 5-hour or weekly balance when Python was unavailable. The new JXA status-line helper atomically publishes the same allow-listed snapshot fields and prunes snapshots older than seven days.
- Fixed local Windows Codex and AGY cards detecting a running agent but failing to start the quota probe because a GUI process may not inherit terminal-profile PATH changes and Windows `CreateProcess` does not resolve npm `.cmd` shims.
- Fixed Windows Claude setup and metadata collection disagreeing about the user's home directory. Setup, the PowerShell helper, session logs, and quota snapshots now consistently use the Windows User Profile folder.
- Fixed failed Codex live reads collapsing into a generic `n/a`. AI DASH now preserves the actionable provider error unless a valid session-log fallback is available.
- Fixed remote SFTP ordering being permanently locked to folder-first/name-ascending despite displaying sortable-looking columns.
- Fixed SFTP file sizes using `KiB / MiB / GiB / TiB` when the file browser was expected to show decimal `KB / MB / GB / TB`.
- Fixed long mount points overflowing the fixed-width DISK summary card. The path alone now shrinks with an ellipsis while the percentage and hidden-disk count remain visible.
- Existing custom Claude status lines are still never overwritten; setup reports the conflict and leaves a manual integration command.

## Validation

- Frontend: 125 Vitest tests pass across SFTP sorting and SI thresholds, telemetry overflow behavior, AI DASH, terminal sessions, drag-and-drop, saved profiles, and credential-vault flows.
- Backend: 147 Rust tests pass with 1 host-permission-dependent telemetry test ignored. Coverage includes macOS JXA execution and privacy filtering, Windows native executable selection, Claude PowerShell snapshots, Codex account normalization, and AGY PTY parsing.
- Static/build checks: the TypeScript/Vite production build succeeds, Clippy passes with warnings denied, ESLint reports no errors, and `git diff --check` passes.
- Release packaging runs on GitHub-hosted Windows, Ubuntu, and macOS runners and publishes a native NSIS `.exe`, Debian `.deb`, Apple Silicon `.dmg`, and `SHA256SUMS.txt`.
- The macOS job signs every nested Mach-O and code container inside-out, applies a final deep ad-hoc signature to `GpuTerm.app`, verifies it with `codesign --deep --strict`, builds the DMG from that verified bundle, mounts the DMG, and verifies the enclosed app again before upload.

## Notes

- No profile, known-host, app-settings, or credential-vault migration is required when upgrading from 1.2.1-beta.
- Existing macOS Claude users should press **AI DASH → Claude Code → Set up** once, restart Claude Code, accept workspace trust if prompted, and send one message so the new `osascript` helper can publish subscription limits.
- Windows Claude users can also rerun **Set up** to refresh the PowerShell helper and normalized User Profile paths. Codex and AGY quota probes update automatically while their CLIs are running.
- Claude subscription limits remain provider-reported and appear after the first response. GpuTerm stores only the allow-listed session, model, context, cost, reset, and quota fields; prompts, responses, tool data, credentials, and environment contents are not copied.
- This is a beta prerelease. Windows installers do not have a trusted publisher signature. The macOS bundle is fully ad-hoc signed for integrity but is not Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.2.1-beta...v1.2.2-beta
