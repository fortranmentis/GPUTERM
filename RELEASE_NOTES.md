# GpuTerm 1.3.2-beta

## Highlights

- **tmux sessions in the terminal toolbar.** The new terminal button lists sessions on the currently focused local or remote Unix host, including window count, attached state, and creation age.
- **Attach and create in a normal GpuTerm split pane.** Selecting a session opens a new pane and attaches by tmux's stable `$N` session ID. Detaching with `Ctrl-B d` returns to the pane's login shell instead of closing the pane. A named session can be created with attach-or-create semantics, or the name can be left blank for tmux to number it.
- **Session lifecycle controls.** Refresh the listing in place or terminate a session from its row after an explicit confirmation. Missing tmux, an empty server, unsupported Windows hosts, and transport errors are shown as distinct non-blocking states inside the picker.

## Fixes

- Kept tmux discovery bound to the focused split's host so changing the active session cannot list one server and attach on another.
- Addressed existing sessions by validated tmux IDs rather than remote-controlled display names, and added POSIX single-quote escaping for new names typed into a live login shell.
- Cleared the `TMUX` environment variable for attach/create commands so a profile that automatically enters tmux does not trigger tmux's nested-session guard.
- Probed common Homebrew, `/usr/local`, user-local, and Snap paths in addition to the login `PATH`, covering shells whose non-interactive profile omits the interactive tmux location.
- Treated tmux's normal “no server running” exit as an empty session list rather than a connection error, while preserving real list and kill failures in the picker.
- Refreshed the README release and download badge URLs so GitHub's image cache no longer leaves the published version count stale after a new release.

## Validation

- Frontend: 184 Vitest tests pass. The new coverage exercises listing, focused-host selection, attach, named and auto-numbered creation, name validation, kill confirmation and cancellation, refresh, empty/not-installed/unsupported/error states, picker interaction, and shell quoting against command separators.
- Backend: 303 Rust tests pass with 1 live-environment test ignored. The new coverage checks tagged listing parsing, window and attach metadata, legacy creation timestamps, hostile names and malformed rows, strict `$<digits>` ID validation, and safe kill-command construction.
- Static/build checks: TypeScript and the Vite production build succeed, ESLint reports 0 errors (14 existing React Hook warnings), the new Rust module passes rustfmt, Clippy passes with warnings denied across all targets, and `git diff --check` passes.
- GitHub-hosted Windows, Ubuntu, and macOS runners build a native NSIS `.exe`, Debian `.deb`, Apple Silicon `.dmg`, and `SHA256SUMS.txt`.
- The macOS job signs every nested Mach-O and code container inside-out, applies a final deep ad-hoc signature to `GpuTerm.app`, verifies it with `codesign --deep --strict`, creates the DMG from that verified bundle, mounts it, and verifies the enclosed app again before upload.

## Notes

- tmux integration is optional and does not install software. `tmux` must already be available on the target Unix host. Native Windows hosts are unsupported; WSL can be used by connecting to its SSH server as a Linux host.
- Killing a tmux session stops every program running inside it and therefore always requires confirmation.
- No profile, known-host, app-settings, or encrypted credential-vault migration is required when upgrading from 1.3.1-beta.
- This is a beta prerelease. The Windows installer is not signed by a trusted publisher. The macOS bundle is fully ad-hoc signed for integrity but is not Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.1-beta...v1.3.2-beta
