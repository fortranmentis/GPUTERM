# GpuTerm 1.1.5-beta

## Highlights

- Completed bidirectional native SFTP desktop drag-and-drop across Debian/Linux, macOS, and Windows, addressing the asymmetric platform failures that remained after 1.1.4-beta.
- Replaced the generic Linux drag plugin path with a GpuTerm-owned native GTK file drag. File URIs are percent-encoded and remain available until `drag-end`, allowing Nautilus and other GTK file managers to finish their asynchronous selection request.
- Added platform-aware native drop coordinate normalization: AppKit and GTK logical coordinates are used directly, while Windows physical pixels are converted with the display scale factor.
- Added a responsive SFTP layout driven by the pane's actual width. Remote metadata columns collapse progressively, local rows tighten, and toolbar and transfer controls remain usable at the minimum pane width.

## Fixes

- Fixed Debian/Linux drag-out showing `Drop the prepared items in Finder, Explorer, or Files` but creating no file in Nautilus or another external destination.
- Fixed the Linux drag source being disconnected immediately on `drop-performed`, before the destination requested the `text/uri-list` payload.
- Fixed spaces, non-ASCII characters, and other URI-sensitive characters in Linux drag-out paths by generating valid file URLs.
- Fixed macOS Finder-to-SFTP drops being ignored on Retina displays because native AppKit coordinates were divided by `devicePixelRatio` a second time.
- Fixed narrow SFTP panes clipping the Modified/Type columns, local Browse control, Download/Upload/Delete actions, and transfer cards.
- Fixed long remote paths and filenames forcing horizontal overflow instead of using ellipsis and responsive columns.
- Removed the no-longer-used drag plugin permission and routed native file drag startup through a validated application command.

## Validation

- Frontend: 103 Vitest tests passed across SFTP, native drag IPC and coordinate scaling, terminal split/input, saved sessions, ProxyJump, monitoring, and credential-vault coverage. The SFTP suite contains 24 tests, plus two focused native drag command tests.
- Backend: 104 Rust tests passed (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including native drag path/image validation and the existing recursive SFTP, terminal, telemetry, and encrypted-vault coverage.
- Static/build checks: Clippy passed with warnings denied, and the TypeScript/Vite production build completed successfully.
- Responsive visual QA: at a 300 px SFTP region (280 px content width), the remote panel, path controls, file rows, local panel, transfer actions, queue, and transfer card all reported `scrollWidth <= clientWidth` with no horizontal clipping.
- Local macOS packaging: the 1.1.5 application bundle is built, fully ad-hoc signed inside-out, and verified with `codesign --verify --deep --strict` before release publication.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS release packaging signs nested Mach-O files and code containers before deep-signing the final app. The DMG is verified, mounted, and its enclosed app is verified again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No credential format or vault migration is required when upgrading from 1.1.4-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- Dragging a remote item outside GpuTerm still materializes it in an application-owned temporary export first. Keep holding while small items prepare; for a large file or folder, wait for preparation to finish and drag it again.
- Native desktop drag-and-drop depends on the destination application's operating-system drag support. Browser-only file payloads are not used as absolute paths.
- Interrupted transfer resume is not implemented; canceled or failed items must be started again.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.4-beta...v1.1.5-beta
