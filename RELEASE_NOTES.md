# GpuTerm 1.1.2-beta

## Highlights

- Added a cross-platform local credential vault. A GpuTerm master password is processed with Argon2id (64 MiB, 3 iterations) to derive a 256-bit key, and the complete credential payload is encrypted and authenticated with AES-256-GCM.
- Replaced macOS Keychain, Windows Credential Manager, and Linux Secret Service/libsecret access. The master password and derived key remain in memory only for the current app run, while saved SSH passwords and key passphrases are never written in plaintext.
- Added native local-host terminals with the same terminal splitting and live CPU, memory, disk, user, and GPU monitoring experience as remote sessions.
- Saved profiles now show when a credential exists and can reconnect by double-clicking without exposing the stored secret to the webview. ProxyJump target and jump-host credentials are restored independently.

## Fixes

- Fixed local-host monitoring so local terminals collect metrics directly on Linux, macOS, and Windows instead of attempting to use an SSH operations connection.
- Fixed saved-session reconnects that previously opened without a usable password prompt or could leave the terminal blank while credentials were unavailable.
- Added authenticated-vault corruption and wrong-password handling, fresh nonces for every write, fixed v1 KDF parameter validation, atomic replacement, rollback on failed persistence, and owner-only `0600` vault files on Unix platforms.
- Added a confirmed **Reset vault** recovery path that deletes saved credentials while preserving session profiles when the master password is forgotten.

## Validation

- Frontend: 89 Vitest tests, including vault creation, unlock retry, reset confirmation, credential masks, saved-session reconnects, ProxyJump prompts, local profiles, terminal splits, and panel persistence.
- Backend: 97 Rust tests passed (1 ignored), including AES-GCM authentication failure, ciphertext tampering, fresh nonce generation, KDF parameter tampering, Unix file permissions, local telemetry parsers, and local PTY behavior.
- Static/build checks: Clippy passed on all targets with warnings denied; the TypeScript production build completed successfully.
- Packaging: native Windows NSIS `.exe`, Debian/Ubuntu `.deb`, and Apple Silicon `.dmg` builds run on GitHub-hosted runners.
- macOS: every nested Mach-O and code bundle is ad-hoc signed inside-out; the final `GpuTerm.app` is deep-signed and checked with `codesign --verify --deep --strict`. The generated DMG is verified, mounted, and the enclosed app signature is checked again.
- Release assets include `SHA256SUMS.txt` covering all three installers.

## Notes

- Existing passwords in macOS Keychain, Windows Credential Manager, or Linux Secret Service/libsecret are neither imported nor deleted. Create a GpuTerm master password and enter SSH passwords again after upgrading.
- The GpuTerm master password cannot be recovered. **Reset vault** keeps session profiles but permanently deletes all credentials saved in `credentials.enc`.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.1-beta...v1.1.2-beta
