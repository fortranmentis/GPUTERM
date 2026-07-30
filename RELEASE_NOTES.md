# GpuTerm 1.2.3-beta

## Highlights

- **Reliable AI DASH metadata on Windows SSH hosts.** Large PowerShell collectors are uploaded over SFTP under a content-addressed name and executed with `-File`, keeping the OpenSSH command line short even when Codex, Claude Code, and AGY are monitored together.
- **Fast repeat polling without repeated uploads.** Unchanged Windows collectors are reused from `~/.gputerm/scripts`; only changed content is transferred, and scripts not touched for seven days are pruned automatically.
- **Actionable collection errors.** If an agent metadata scrape fails, AI DASH now reports the transport or execution reason instead of showing the same generic state as a provider that has not published data yet.

## Fixes

- Fixed Windows agent model and quota metadata disappearing when the UTF-16LE base64 `-EncodedCommand` exceeded the 8,191-character `cmd.exe` limit used by the default Windows OpenSSH shell. The all-provider form previously reached roughly 18,954 characters.
- Fixed Claude Code setup failing on existing macOS and Windows SSH installations with `[SFTP(4)] failure`. libssh2 negotiates SFTP v3, whose standard rename cannot overwrite an existing destination; GpuTerm now uploads the complete temporary file, unlinks the destination only when required, and retries the rename.
- Fixed failed metadata collection being silently discarded. A specific Claude setup-state hint still takes precedence when it provides a more useful remediation.
- Preserved short Windows PowerShell collectors on the lower-latency inline path while moving only commands above a conservative safety threshold to SFTP.

## Validation

- Frontend: 125 Vitest tests cover AI DASH, SFTP sorting and transfers, terminal sessions, telemetry layout, saved profiles, and credential-vault flows.
- Backend: 160 Rust tests pass with 1 host-permission-dependent telemetry test ignored. New regression coverage checks every Windows provider combination against the `cmd.exe` limit, uploaded-script selection, SFTP v3 replacement behavior, and propagation of metadata errors to AI DASH.
- Static/build checks: TypeScript and the Vite production build succeed, ESLint reports no errors, Clippy passes with warnings denied, and `git diff --check` passes.
- Release packaging runs natively on GitHub-hosted Windows, Ubuntu, and macOS runners and publishes an NSIS `.exe`, Debian `.deb`, Apple Silicon `.dmg`, and `SHA256SUMS.txt`.
- The macOS job signs nested Mach-O code inside-out, applies a final deep ad-hoc signature to `GpuTerm.app`, verifies it with `codesign --deep --strict`, creates the DMG from that verified bundle, mounts it, and verifies the enclosed app again before upload.

## Notes

- No profile, known-host, app-settings, or encrypted credential-vault migration is required when upgrading from 1.2.2-beta.
- Windows SSH monitoring may create content-addressed PowerShell files under the connected user's `~/.gputerm/scripts` directory. They contain only GpuTerm's collector code, do not contain credentials or collected output, and are removed after seven days without use.
- Claude subscription limits remain provider-reported and appear after the first response. GpuTerm stores only the documented allow-listed snapshot fields; prompts, responses, tool data, credentials, and environment contents are not copied.
- This is a beta prerelease. The Windows installer is not signed by a trusted publisher. The macOS bundle is fully ad-hoc signed for integrity but is not Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.2.2-beta...v1.2.3-beta
