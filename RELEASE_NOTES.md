# GpuTerm 1.4.0

## Highlights

- **First official desktop release.** 1.4.0 leaves beta: the version has no prerelease suffix, GitHub publishes it as a non-prerelease and Latest, and existing signed-updater installations retain their update endpoint.
- **See what fills a disk.** Click a mount in the disk details to list child folders largest first, drill down one level at a time, return with breadcrumbs, rescan, or open the current folder in SFTP where available. Measurements are requested on demand and cached in memory, not added to periodic monitoring.
- **One workspace for remote operations.** The established terminal splits, SSH/SFTP, encrypted credential vault and key manager, GPU/system history and alerts, container/SQL monitoring, AI DASH, LLM-runtime monitoring, remote web-service opening and local MCP integration remain available with their documented platform limits.
- **Windows, macOS and Linux distribution.** EXE, Apple Silicon DMG and DEB remain available, alongside AppImage and signed updater bundles. English/Korean READMEs and the desktop website status now identify the official release.

## Fixes

- Distinguish stable version tags from prerelease tags in both GitHub release creation and installer upload, so packaging cannot turn 1.4.0 back into a prerelease. Keep only the infrastructure `updater` channel as a prerelease for backward compatibility.
- Run remote folder scans on a separate SSH connection and admit one scan per session, avoiding a long `du` blocking shared operations. Surface incomplete results, unreadable paths, errors and omitted smaller folders instead of inventing totals.
- Use a private `mktemp` file with exit cleanup for folder-scan diagnostics rather than a predictable temporary filename.
- Count only `du` diagnostics as unreadable-path warnings, not unrelated priority-wrapper warnings; a controlled local scan caught this distinction during validation.
- Retry transient DMG verification/mount service errors up to five times. Verification is never skipped: persistent image errors fail packaging, and an invalid app signature fails immediately with mount cleanup.
- Preserve both the local folder-size feature and the incoming website documentation update; leave unrelated working documents/assets out of release commits.

## Validation

- **358 frontend tests pass across 54 files; 505 macOS Rust tests pass, with 6 ignored.** Platform-specific counts can differ. TypeScript, the Vite production build, strict Clippy across all targets/features, Bash syntax, release-workflow YAML parsing and `git diff --check` pass.
- ESLint reports 0 errors and 14 existing React Hook warnings. Existing React test `act` and Vite chunk-size warnings remain non-fatal; unrelated repository-wide formatting differences are not rewritten.
- New coverage exercises folder sorting/navigation/cache/rescan/SFTP actions/errors, complete and partial scan parsing, unreadable counts, bounded output and shell quoting. A controlled local filesystem fixture executes `du` and checks private diagnostic-file cleanup; it does not scan user data or a real SSH server.
- Mock-command packaging tests check transient DMG retries, persistent verification failure, signature failure and cleanup without mounting an actual image. Published macOS artifacts are separately verified using the real platform tools.
- Stable-version regression tests cover 1.4.0 release-page URLs and updater bundle names, website preference for an official release over a newer beta, historical beta fallback, and exclusion of drafts/infrastructure releases.
- Release CI checks the declared Rust minimum and native Windows/Linux/macOS packaging. Final verification downloads the published files, checks SHA-256, inspects package versions, verifies the mounted DMG and signed update bundles, and confirms the official/Latest flag and updater-channel manifest.

## Notes

- Official release status is a distribution milestone, not a claim that every host/GPU/server configuration was tested. Live remote folder scans, large production trees, Windows/Linux installed GUI behavior and biometric hardware remain outside this release's automated verification.
- Folder sizes currently support Linux/macOS SSH hosts and local Linux/macOS terminals, not Windows. They use allocated disk usage from `du -x -k -d 1`, not logical file sizes, and stay on one filesystem. Only the top 200 folders are displayed; cached values can be stale. Permission errors, unusual tab/newline filenames, hard links and concurrent filesystem changes can limit interpretation.
- Folder scans use low CPU priority and idle I/O priority when available, without sudo. A host-side 25-second limit depends on `timeout` or Perl; the command budget is 35 seconds, not an unconditional guarantee that a remote process is killed when neither helper exists. Truncated measurements are lower bounds.
- Existing beta profiles and encrypted vaults remain compatible; upgrading does not automatically run a scan, install server keys or perform a remote action. The fixed updater channel continues serving the signed 1.4.0 manifest to existing installations. DEB installations update through package workflows/manual installation; AppImage supports in-app updates.
- The macOS app is fully ad-hoc integrity-signed, not Developer ID signed or Apple-notarized; the DMG targets Apple Silicon. Windows installers lack a trusted-publisher signature. The updater's public-key signatures are separate from platform publisher trust; private signing keys remain in GitHub Secrets.
- Existing repository-wide formatting differences, React Hook/test warnings and bundle-size warnings are not broadened into unrelated rewrites. Consult the README for feature-specific permissions, authentication and platform support.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.9-beta...v1.4.0
