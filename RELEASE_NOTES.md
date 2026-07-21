# GpuTerm 1.1.1-beta

## Highlights

- Build flexible terminal layouts with up to four panes. New panes can be placed left, right, above, or below the focused pane, with a selectable initial ratio and draggable dividers.
- Mix independent shells from the same host and saved sessions from different hosts in one layout while keeping each session's SFTP and monitoring context available.
- Close and restore the host selector, SFTP browser, and monitoring bar independently. Panel visibility and SFTP width are restored on the next launch, and the terminal automatically expands into freed space.

## Fixes

- Fixed a connection race that could leave a newly connected terminal blank when xterm was initialized inside a hidden pane before the visible split layout mounted.
- Preserved the existing xterm DOM, scrollback, pending MOTD, and prompt output when a pane moves to another split branch.
- Prevented terminal header controls and the bottom input area from being clipped after splitting or resizing the workspace.
- Replaced generic SFTP and monitoring close icons with directional panel controls matching the host selector.

## Validation

- Frontend: 76 Vitest tests, including early terminal output replay, visible-pane mounting, split-layout DOM preservation, and panel visibility persistence.
- Backend: 85 Rust tests and Clippy on all targets with warnings denied.
- Packaging: production frontend build plus native Windows NSIS `.exe`, Debian/Ubuntu `.deb`, and Apple Silicon `.dmg` builds on GitHub-hosted runners.
- macOS: every nested Mach-O and code bundle is ad-hoc signed inside-out; the final `GpuTerm.app` is deep-signed and checked with `codesign --verify --deep --strict`. The generated DMG is verified, mounted, and the enclosed app signature is checked again.
- Release assets include `SHA256SUMS.txt` for all three installers.

## Notes

- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.
- Passwords and key passphrases remain memory-only and are never written to disk.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.0-beta...v1.1.1-beta
