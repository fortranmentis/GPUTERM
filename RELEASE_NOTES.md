# GpuTerm 1.1.4-beta

## Highlights

- Added recursive SFTP upload and download for complete directory trees. Each top-level file or folder stays in one transfer queue item with aggregate byte progress and cancellation.
- Added reliable native desktop drag-and-drop from Explorer, Finder, and Nautilus on Windows, macOS, and Debian/Linux, while retaining URI-list paste uploads from compatible file managers.
- Added native SFTP drag-out from GpuTerm to Finder, Explorer, and Linux file managers. Remote files, folders, and multi-selections are materialized into an application-owned temporary export before the operating-system drag begins.
- Added a draggable, keyboard-accessible divider between the remote and local SFTP lists so their vertical ratio can be adjusted to fit the current task.
- Refined the remote toolbar: Open and mkdir are compact icon buttons, and the folder-name editor appears only after mkdir is selected.
- Increased selected-item contrast in both SFTP lists and made local folders selectable with one click and openable with a double-click.

## Fixes

- Fixed external SFTP drops showing `No local file payload found` on Debian/WebKitGTK when the browser drag payload omitted absolute paths.
- Fixed Windows and macOS external drops passing only a filename, which caused `Failed to inspect local file ... No such file or directory (os error 2)`.
- Fixed remote SFTP items stopping at the GpuTerm window boundary because the previous pointer-only transfer model never created a native operating-system file drag.
- Fixed cross-platform drag-out failures for remote names that collide after sanitization or use Windows-reserved characters by reserving safe, unique absolute staging paths.
- Fixed dropped directories being detected but skipped instead of enqueued for transfer.
- Fixed local folder clicks navigating immediately, which prevented folders from being selected for upload.
- Fixed directory transfer progress being reset after the backend had reported the recursively calculated total.
- Fixed empty-directory transfers finishing without a visible 100% completion state.
- Hardened recursive traversal against symbolic-link cycles and unexpected paths outside the selected tree.

## Validation

- Frontend: 101 Vitest tests passed across SFTP, native file drag, terminal split/input, saved-session, ProxyJump, monitoring, and encrypted credential-vault coverage. The SFTP suite includes 22 tests, plus two focused native drag IPC tests.
- Backend: 102 Rust tests passed (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including recursive local tree sizing, platform-independent remote path joining, and safe unique drag-out staging paths.
- Static/build checks: Clippy passed for all targets and features with warnings denied, and the TypeScript/Vite production build completed successfully.
- Local macOS packaging: the 1.1.4 application bundle built successfully with native drag enabled and passed `codesign --verify --deep --strict` after ad-hoc signing.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS: every nested Mach-O file and code bundle is ad-hoc signed inside-out; the final `GpuTerm.app` is deep-signed and verified with `codesign --verify --deep --strict`. The DMG is verified, mounted, and its enclosed app is verified again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No credential format or vault migration is required when upgrading from 1.1.3-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- When a destination folder already exists, confirming the prompt merges transferred contents into that folder and replaces colliding files; unrelated existing files remain in place.
- Symbolic links are intentionally not followed during recursive folder transfer. Transfer the resolved file or directory directly when that content is needed.
- Dragging a remote item outside GpuTerm first downloads it to an application-owned temporary export. Keep holding while small items prepare; for a large file or folder, wait for the transfer to finish and drag it again. Prepared paths are briefly cached for retry, and exports older than 24 hours are pruned during later drag-outs.
- Interrupted transfer resume is not implemented; canceled or failed items must be started again.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.3-beta...v1.1.4-beta
