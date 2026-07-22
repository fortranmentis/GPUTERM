# GpuTerm 1.1.3-beta

## Highlights

- Hardened interactive SSH terminal input so simultaneous keys, modifier chords, and key-repeat bursts remain responsive without terminating the terminal session.
- Removed the per-input SSH channel flush that could discard queued remote output instead of flushing outgoing key data.
- Serialized nonblocking terminal writes, PTY resizes, and keepalive packets so a libssh2 `EAGAIN` operation is always retried before another SSH packet is submitted.
- Added regression coverage for burst input that temporarily returns `WouldBlock`, including a guard that ensures terminal input never invokes the destructive channel flush path.

## Fixes

- Fixed the remaining `Terminal stream failed: transport read` disconnect path that could occur while pressing two or more keys or entering rapid keyboard input.
- Fixed `Channel::flush()` being called after every terminal write. In libssh2 this operation discards queued incoming channel data and adjusts the receive window; it is not an outgoing TCP flush.
- Fixed keepalive and PTY resize packets interleaving with a partially sent terminal input packet after libssh2 returned `EAGAIN`.
- Fixed a blocking TCP socket remaining underneath a nonblocking libssh2 session. The mismatch could hold the shared channel lock until the socket receive timeout and surface that timeout as a fatal transport error, particularly on Windows and ProxyJump connections.
- Fixed transient `Interrupted` reads and writes terminating the ProxyJump forwarding loop; they are now retried alongside `WouldBlock` operations.
- Fixed terminal keepalive checks running once per second even though the configured interval is 30 seconds. The reader now follows the actual keepalive interval.

## Validation

- Frontend: 90 Vitest tests passed, including the new simultaneous-input serialization regression and the existing terminal split, saved-session, ProxyJump, SFTP, monitoring, and credential-vault coverage.
- Backend: 99 Rust tests passed (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including burst-write retry, no-flush, and libssh2 `EAGAIN` classification coverage.
- Static/build checks: Clippy passed for all targets with warnings denied, and the TypeScript/Vite production build completed successfully.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS: every nested Mach-O and code bundle is ad-hoc signed inside-out; the final `GpuTerm.app` is deep-signed and verified with `codesign --verify --deep --strict`. The DMG is then verified, mounted, and the enclosed application signature is verified again before upload.
- Release assets include `SHA256SUMS.txt` covering all three installers.

## Notes

- No credential format or vault migration is required when upgrading from 1.1.2-beta; saved profiles and the Argon2id + AES-256-GCM credential vault remain unchanged.
- The `v1.1.3-beta` installers were rebuilt after the initial publication to include the complete simultaneous-key fix; re-download the installer if it was obtained before this update.
- A genuine network or server-side disconnect can still close a terminal and report a transport error; this release fixes the false disconnects caused by local SSH transport state handling.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.2-beta...v1.1.3-beta
