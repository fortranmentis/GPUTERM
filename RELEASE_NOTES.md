# GpuTerm 1.3.9-beta

## Highlights

- **Open remote web UIs without exposing a public port.** The sidebar's **Web services** lists TensorBoard, Jupyter, MLflow, Ray Dashboard, Streamlit, Gradio and code-server. **Open** creates/reuses a loopback-only SSH forward and launches the default browser; Jupyter runtime-file base URLs and login tokens are used when available.
- **Watch every connected SSH host.** Discovery runs every 20 seconds on Linux, macOS and Windows. Newly detected known services can show an **Open** toast after the initial scan; notifications can be disabled. Other listening ports are listed separately, and Linux additionally inspects exposed TCP ports from up to 50 accessible Docker containers.
- **Terminal links and saved forwards.** Cmd/Ctrl-click HTTP(S) URLs, including wrapped links. Plain HTTP URLs pointing to localhost, wildcard addresses or the SSH host's own name are opened through a temporary SSH forward. Pin a discovered service to save a forward for future connections.
- **Web services for AI assistants.** The read-only MCP `list_web_services` tool returns discovered service metadata with Jupyter tokens removed from returned paths. MCP settings now use switches and compact host-visibility rows.
- **Refreshed project website.** The bilingual GitHub Pages landing page now covers desktop/native mobile apps, external-display workspaces and Siri Shortcuts, with responsive download tables and Korean line-wrapping improvements. These website changes do not add new desktop mobile features.

## Fixes

- Stop global text-field styles from stretching checkbox and radio controls to text-input dimensions; restore horizontal checkbox labels across the app.
- Declare the landing-page JavaScript's browser environment in ESLint so its DOM APIs are checked without false undefined-global errors.
- Handle wrapped terminal URLs and surrounding punctuation, retain Jupyter base paths/token queries when opening, and reuse live forwards or choose another free local port when needed.
- Keep unrelated kernels/editor backends/agents out of named-service classification; expose unknown listening ports separately rather than labelling every port a recognized web UI.
- Report scan/open failures in the service panel and browser-opening failures as in-app messages. Keep the first discovery scan quiet instead of announcing already-running services.
- Update the English and Korean READMEs with discovery commands, browser-opening behavior, notification/pin controls and operational limits.

## Validation

- **348 frontend tests pass across 51 files**, including the two updater-manifest tests (also rerun separately). **497 macOS Rust tests pass; 6 are ignored.** Platform-specific counts can differ.
- TypeScript, ESLint (0 errors, 14 existing React Hook warnings), the Vite production build and `git diff --check` pass. Strict Clippy checks all targets/features with warnings denied. Existing React test `act` and Vite chunk-size warnings are non-fatal.
- New automated coverage includes Linux/macOS/Windows listener fixtures, Docker port parsing, Jupyter token/base-path handling, forward-target/path validation, terminal URL wrapping, service-panel opening/pinning/errors and actionable notifications.
- GitHub CI checks the declared Rust 1.88 minimum. Native release jobs build Windows NSIS EXE, Debian DEB, Linux AppImage and Apple Silicon DMG; signed update bundles, signatures, SHA-256 checksums and `latest.json` are published through the existing updater channel.
- macOS packaging signs every nested code object inside-out and the final app ad-hoc, verifies with `codesign --verify --deep --strict`, then verifies the app again from the mounted DMG. Published assets are downloaded again for checksum, package-version and manifest checks.

## Notes

- Compared with 1.3.8-beta this release adds web-service discovery/opening and UI refinements, not a credential or profile-format migration. Existing profiles, vaults, MCP approvals and updater behavior remain compatible. A pinned forward is saved only when explicitly selected.
- Discovery is best-effort, not a health check: tool availability, account permissions, process visibility, Jupyter runtime paths and Docker networking affect results. Container image/port heuristics can misclassify a service; exposed ports may not be listening, and separate authentication may still be required. Docker discovery requires existing Docker access and never adds sudo.
- Automatic discovery targets remote SSH hosts, not local terminal sessions. The watch announces only newly seen host/port pairs after a baseline scan. HTTP localhost/wildcard/own-host links are tunneled; HTTPS and unrelated addresses open unchanged. Saving a forward does not save its Jupyter token.
- Tests use fixtures and mocked UI calls. Live remote Linux/macOS/Windows discovery, container web UIs and installed-platform browser interactions were not exercised by this release verification; the 1.3.8 live updater/MCP checks are historical, not rerun claims for 1.3.9.
- Update bundles use the existing updater signing key; private signing material is read only from GitHub Secrets. The Windows installer has no trusted-publisher signature. The macOS app is fully ad-hoc signed, not Developer ID signed or Apple-notarized. The DMG targets Apple Silicon; DEB installs update through their package workflow/manual download, while AppImage supports in-app updates.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.8-beta...v1.3.9-beta
