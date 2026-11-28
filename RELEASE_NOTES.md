# GpuTerm 1.4.2

## Highlights

- **Lighter Linux and macOS telemetry.** Batch collector commands into non-login shell calls and cache static host information per connection. Coalesce terminal output into fewer UI updates.
- **More reliable terminal splits and SFTP.** Search the active pane only, keep link positions accurate after wide characters, and retain transfer progress while the SFTP pane is closed.
- **Cancelable folder-size scans.** Cancel an active scan or reuse cached results for up to five minutes. Local browsing follows directory symlinks and tolerates broken or unreadable entries.

## Fixes

- Clean up terminal link providers, resize listeners, and closed-pane state; keep Ctrl+F working as expected on non-macOS platforms.
- Limit concurrent SFTP drag-out work, stabilize drag listeners, and send terminal/transfer events only to the main window.
- Avoid stale web-service polling results after disconnect and duplicate container fetches per session. Recognize Podman and nerdctl in web-service discovery and redact credentials and query parameters in TensorBoard log-directory URLs.
- Move blocking connection/channel setup off forwarding accept loops and preserve client half-close behavior. Prevent simultaneous requests from opening duplicate shared operations connections on a cold cache.
- Apply the remote-only check to MCP process termination; expose a visibility toggle for connected local hosts. Prune obsolete hidden-profile entries and protect deletion of profiles used by database instances.
- Serialize saved LLM instance writes, check pinned forward port collisions, normalize trailing slashes for disk scans, and avoid exponential backtracking in SSH config wildcard matching.
- Split large SSH, agent monitoring, terminal, SFTP and stylesheet modules. Enforce the React hook dependency rule as an ESLint error and expand regression coverage.

## Validation

- The local frontend suite passed 403 tests. GitHub CI checks the frontend, Rust backend, TypeScript, ESLint, Clippy, and the declared Rust minimum. Installer builds also compile the production frontend.
- Added regression coverage for terminal links, folder-size cancellation/cache, session and web-service stores, container and database monitoring, updater behavior, snippets and SFTP panels.
- Correct platform-specific imports in the status-line and local terminal tests so that Linux lint and Windows test compilation can run.
- GitHub Actions builds Windows NSIS EXE, Apple Silicon DMG, Debian DEB and Linux AppImage. The Mac workflow ad-hoc signs nested code and the entire app bundle, then mounts the final DMG and verifies its enclosed app with `codesign --verify --deep --strict`.

## Notes

- Cancel discards the pending result; a collector that has already started may continue to its timeout. Folder-size scanning still does not support Windows; cached results can be stale until expiry or rescan.
- Existing installations continue using the signed updater channel; DEB users install updates manually. Ad-hoc signing verifies bundle integrity; it is not Apple Developer ID signing or notarization. The Windows installer does not carry a trusted publisher signature.
- Automated and packaging tests do not establish live SSH compatibility, native drag-and-drop behavior, or normal operation on every GPU/host combination.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.4.1...v1.4.2
