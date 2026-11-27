# GpuTerm 1.4.1

## Highlights

- **Official maintenance release.** 1.4.1 remains a stable, non-prerelease GitHub release and Latest, with the existing signed-updater endpoint preserved.
- **More consistent MCP host privacy and approvals.** Host-linked LLM runtimes and active alerts respect hidden-host settings. Actions re-check current settings after approval; disabling MCP/actions cancels pending requests. Audit argument size is bounded and `read_file` requires a regular file.
- **More resilient SFTP transfers.** Private app-owned drag-out staging, safer local file replacement, fewer redundant remote checks during folder overwrites, and sanitized downloaded folder segments.
- **Smoother lifecycle and monitoring.** MCP commands use a dedicated SSH session with interleaved stdout/stderr reads; local collectors drain both pipes concurrently. Disconnect/settings work no longer blocks the UI, profile deletion asks for confirmation, and app exit requests session/forward teardown.

## Fixes

- Apply hidden-host filtering to LLM summaries and active alerts, and revalidate action permissions after approval rather than relying only on the original request state.
- Avoid pipe-buffer stalls in local commands and approved MCP commands with large stdout/stderr. Keep command work off shared operations connections.
- Stage SFTP drag-out exports under the application configuration directory with Unix 0700/0600 permissions and best-effort pruning of exports older than 24 hours at startup, export preparation and exit. Use platform atomic replacement for local downloads/vault updates, including `MoveFileExW` on Windows.
- Avoid redundant remote `lstat` calls for overwrite folder transfers; sanitize nested download names and preserve clearer errors/part-file recovery information on remote finalize failure.
- Validate Claude status-line session IDs, remove `GPUTERM_PAYLOAD` environment transport and reduce predictable temporary-file races across the platform helpers.
- Serialize remote-forward listener setup with the connection's channel-open lock; strengthen DB temporary-file cleanup traps and confirmation before saved-host deletion.
- Document access-control changes, local drag-out staging and status-line helper updates in the English and Korean READMEs.

## Validation

- GitHub CI checks the frontend and Rust test suites, TypeScript, ESLint, Clippy and the declared Rust minimum. Native packaging builds the production frontend on each platform.
- Automated coverage includes MCP filtering/approval rechecks and stream draining, SFTP path/cache/permission behavior, status-line validation and snapshot writes, and profile-deletion confirmation.
- Native GitHub runners build Windows EXE, Debian DEB, Linux AppImage and Apple Silicon DMG. Checksums and signed update bundles are published with `latest.json`; the macOS bundle is fully ad-hoc signed inside-out and verified again from its mounted DMG.
- Final verification downloads published assets, checks SHA-256 and package versions, validates the DMG/app signatures and updater signatures/trusted comments, rejects a tampered digest, and compares release/updater manifests and source notes. Stable/Latest flags and source/tag/workflow revisions are checked separately.

## Notes

- Existing 1.4.0 profiles and encrypted vaults retain their format. This patch does not silently install keys, enable MCP actions, change DB configuration or run user-approved remote operations during upgrade.
- SFTP drag-out copies are plaintext local files, not encrypted vault entries. Unix mode restrictions do not protect against other processes under the same OS account; Windows uses inherited OS/account permissions. Cleanup is best-effort and prunes aged entries, not every fresh export immediately on exit.
- Local atomic replacement does not guarantee atomic remote overwrites: SFTP-server capabilities may require unlink/rename fallback. On failed finalize after replacing a destination, inspect the reported remote part-file location before retrying.
- Direct-URL LLM instances with no hidden SSH-profile association remain visible to MCP. Approval and regular-file checks are access-control refinements, not a command sandbox or a guarantee against every filesystem race.
- Re-run Claude integration setup or reinstall the helper on each monitored host to deploy helper changes. Updating the desktop alone does not replace remote status-line files; existing custom status-line conflicts still need manual integration.
- Automated tests use fixtures/mocks and limited controlled local processes. Live SSH/SFTP interoperability, Windows/Linux installed GUI behavior, native file-manager drag/drop, real DB/cluster operations and biometric hardware are not established by these tests. Platform-specific test counts can differ.
- The Apple Silicon app is completely ad-hoc integrity-signed, not Developer ID signed or Apple-notarized; Windows installers lack a trusted publisher signature. Updater public-key signatures are separate from platform publisher trust. Private signing material remains in GitHub Secrets.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.4.0...v1.4.1
