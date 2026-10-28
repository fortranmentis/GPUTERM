# GpuTerm 1.3.8-beta

## Highlights

- **In-app updates.**
  - GpuTerm checks GitHub for a newer release shortly after launch and every six hours, then shows the release notes with **Update**, **Later** and **Skip this version**.
  - **Update** downloads a signed update bundle, verifies it against the public key built into the app, installs it and offers **Restart now**.
  - The bottom of the sidebar shows the installed version, and **Check for updates** runs the same check by hand.
- **Built-in MCP server for AI assistants.**
  - Click **MCP** at the bottom of the sidebar to let Claude Code, Claude Desktop, Codex, Cursor and other stdio MCP clients read the hosts you have connected.
  - Eleven read-only tools cover:
    - live telemetry and the last hour of history;
    - free GPUs across hosts;
    - GPU and top processes;
    - containers, tmux sessions and directory listings;
    - LLM runtimes and active alerts.
  - The dialog gives a ready-to-paste setup for each client.
- **Actions only with your approval.** With **Allow actions** on, `run_command`, `read_file` and `kill_process` each open a confirmation in GpuTerm. Deny is the default, and no answer within two minutes counts as a denial.
- **Linux AppImage.** Releases now include `GpuTerm_x.y.z_amd64.AppImage`, which can update itself. The `.deb` remains for package-manager installs.

## Fixes

- The backend now tracks which alerts are currently firing and the latest telemetry reading per session. Before, only the window did, so the MCP tools could not see them.
- The update dialog sits above the credential-vault lock screen, so a pending update is visible before you unlock. MCP approval requests sit above both.
- The release workflow signs every update bundle:
  - It repacks the macOS `.app.tar.gz` from the fully ad-hoc signed app, rather than the pre-signing build output.
  - It publishes `latest.json` to a fixed `updater` release, because GitHub's "latest" link skips pre-releases.
  - `SHA256SUMS.txt` now leaves out signature files.

## Validation

- **339 frontend tests pass across 48 files.** New coverage includes:
  - update states (available, install, restart, skip, later, package-managed builds, failed checks);
  - the MCP approval queue (Deny by default, Escape denies, expired requests are dropped);
  - MCP settings and the per-client setup snippets, including Windows paths.
- **484 macOS Rust tests pass; 6 are ignored.** New coverage includes:
  - JSON-RPC handling: version negotiation, hidden action tools, malformed input, notifications;
  - token matching and over-long line limits;
  - host lookup by id or name;
  - free-GPU filtering, history downsampling and output caps;
  - binary-file detection;
  - approval delivery, audit-log rotation and the active-alert lifecycle.
- **Other checks:** TypeScript, ESLint (0 errors, 14 existing React Hook warnings) and Clippy with all targets and features and warnings denied pass. The new updater crates are pinned to versions that keep the declared Rust 1.88 minimum.
- **Manual check on macOS (updater):** a 1.3.8 test build found a locally served 1.3.9 manifest. It downloaded and verified the bundle, which was signed the same way the release workflow signs it, installed it and relaunched as 1.3.9.
- **Manual check on macOS (MCP):** through `gputerm --mcp-stdio` against the running app with two connected hosts (macOS and WSL Ubuntu with an RTX 3060 Ti):
  - `list_hosts`, `get_host_status`, `get_usage_history`, `find_free_gpus`, `list_tmux_sessions` and `get_active_alerts` returned live data;
  - `run_command` ran only after the in-app approval and was recorded in the audit log;
  - with the app closed, the relay explained how to turn MCP on.
- **Release workflow:** builds the Windows NSIS EXE, Debian DEB, Linux AppImage and Apple Silicon DMG.
  - The macOS app is ad-hoc signed inside-out, checked with `codesign --verify --deep --strict` and verified again from the mounted DMG.

## Notes

- **Install this version by hand once.** 1.3.7-beta and earlier have no updater. From 1.3.8-beta on, new releases can be installed from inside the app.
- **Debian/Ubuntu `.deb` installs are not updated in place.** The update dialog opens the release page instead. The AppImage updates itself.
- **Update signing is separate from OS code signing.** Updates are signed with GpuTerm's own minisign key, and a bundle whose signature does not match is refused. The Windows installer is still not trusted-publisher signed. The macOS app is fully ad-hoc signed for integrity, not Developer ID signed or notarized; Intel Mac users can build from source.
- **MCP is off by default.**
  - The server listens only on `127.0.0.1`, with a token regenerated on every start and kept in a file only your user can read.
  - Tools see only hosts connected in GpuTerm and never return saved passwords or keys.
  - Command output and file contents are marked as untrusted data.
  - Every call is logged to `mcp-audit.log` in the GpuTerm data folder.
- **No migrations.** This release introduces no credential-format, profile-schema or server-side migration. `app_settings.json` gains an optional `mcp` section.
- **Untested:** Windows stdio inheritance for `--mcp-stdio`, and Claude Desktop/Codex/Cursor sessions, are not yet exercised on real installs.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.7-beta...v1.3.8-beta
