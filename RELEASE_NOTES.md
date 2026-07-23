# GpuTerm 1.1.6-beta

## Highlights

- Stabilized native local-host monitoring on Windows without changing the local terminal or saved-session workflow.
- Local Windows telemetry now launches PowerShell with the Windows `CREATE_NO_WINDOW` process flag, so CPU, memory, disk, user, GPU, and detail polling stays inside GpuTerm instead of flashing console windows.
- The collector resolves the installed system Windows PowerShell directly, with a normal PATH lookup fallback for nonstandard installations.
- PowerShell collector output is explicitly emitted as UTF-8 text, preserving localized CPU, device, volume, user, and process names for the existing JSON parsers.

## Fixes

- Fixed one or more external terminal windows appearing repeatedly after connecting to the Windows local host.
- Fixed monitoring cards remaining unavailable when installed-app environment differences prevented a bare `powershell.exe` lookup.
- Fixed localized Windows collector output being decoded through the active OEM code page and potentially invalidating telemetry JSON.
- Applied the hidden, noninteractive collector path consistently to regular telemetry polls, GPU probing/counters, and on-demand CPU, memory, and GPU detail queries.

## Validation

- Frontend: 103 Vitest tests pass across terminal sessions/splits, SFTP, monitoring, saved profiles, ProxyJump, native drag-and-drop, and credential-vault behavior.
- Backend: 105 Rust tests pass (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including the new Windows local collector command/UTF-8 configuration coverage.
- Static/build checks: Rust `cargo check`, Clippy with warnings denied, and the TypeScript/Vite production build complete successfully.
- Windows-specific regression coverage verifies the system PowerShell command, noninteractive text arguments, UTF-8 preamble, and a native Windows UTF-8 collector round trip.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS release packaging fully ad-hoc signs nested Mach-O files and code containers inside-out, deep-signs and strictly verifies the final app, creates the DMG from that verified bundle, then mounts it and verifies the enclosed app again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No profile, known-host, or credential-vault migration is required when upgrading from 1.1.5-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- Local Windows monitoring uses Windows PowerShell 5.1, which is preinstalled on supported Windows 10/11 systems; no OpenSSH server is required for a local session.
- The first CPU sample may show usage as unavailable until the next poll because usage is calculated from two counter samples. Memory and disk data can appear immediately.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.5-beta...v1.1.6-beta
