# GpuTerm 1.3.5-beta

## Highlights

- **GpuTerm-managed SSH keys.** Open **SSH key → Set up key** on a saved remote profile to generate an Ed25519 key, install its public half in the server's authorized-key file, and verify a sign-in with the new key before saving it.
- **Encrypted storage and in-memory sign-in.** The private key stays in the existing Argon2id / AES-256-GCM credential vault and is passed to libssh2 from memory. Profile JSON contains only the public key, fingerprint and timestamps. Normal terminal, SFTP, monitoring, forwarding and LLM tunnel connections can use managed keys, including keys belonging to ProxyJump hops.
- **Key lifecycle controls.** Inspect/copy the public key and SHA-256 fingerprint, replace or remove a key, and export a backup as a passphrase-encrypted OpenSSH private key with an adjacent `.pub` file. Export requires a repeated passphrase of at least 8 characters.
- **Platform-aware installation.** POSIX hosts use `~/.ssh/authorized_keys` with directory/file modes 700/600; Windows OpenSSH administrators use `administrators_authorized_keys`, with ACL adjustment, and other users use their profile's `.ssh` directory.

## Fixes

- Verify a new key with a strict authentication policy: SSH agent, password, keyboard-interactive and a successful unauthenticated sign-in cannot disguise rejection of the new key. ProxyJump hosts keep their own normal authentication.
- Stop Windows key installation/removal on PowerShell errors, and check the native `icacls` exit code before reporting successful permission changes.
- Report failed rollback/revocation with the new key's fingerprint instead of claiming that server cleanup succeeded.
- Keep managed-key secrets separate from profile passwords, preserve key metadata when editing a profile, and clear the profile's key vault entry on profile deletion. Ordinary connections retain their existing credential fallback when a managed key is rejected.
- Document the distinction between deleting local credentials and revoking the server's public key, along with backup and vault-reset recovery steps. Remove links to maintenance drafts that are not yet tracked for publication.

## Validation

- **272 frontend tests pass** across 37 files, including managed-key setup failure/success, replacement confirmation, encrypted-export passphrase rules and cancellation, removal options, and remote/local profile panel visibility.
- **417 Rust tests pass**, with **5 environment-dependent/developer tests ignored**. Key coverage includes distinct Ed25519 generation, fingerprints, safe comments/public-key validation, export encryption/decryption and wrong-passphrase rejection, secret redaction, per-hop vault lookup, private export permissions, authentication fallback, and strict new-key-only verification.
- TypeScript checking and the Vite production build pass. ESLint reports 0 errors and 14 existing React Hook warnings. Clippy passes across all targets/features with warnings denied; `git diff --check` passes. GitHub CI also checks the declared Rust 1.88 minimum.
- GitHub Actions builds native Windows NSIS `.exe`, Debian `.deb` and Apple Silicon `.dmg` installers, then generates `SHA256SUMS.txt`. Windows-specific local telemetry and Claude status-line tests run on the Windows build host.
- The macOS job signs nested Mach-O files and code containers inside-out, applies the final ad-hoc bundle signature, verifies with `codesign --verify --deep --strict`, then creates/mounts the DMG and verifies the enclosed app before upload.

## Notes

- Setup explicitly changes a remote authorized-key file and requires working initial credentials, a trusted host key, write permissions, and an unlocked vault. The new key must support key-only authentication on the final host; a server requiring a second factor will fail this verification.
- Live managed-key installation, sign-in and revocation against macOS/Linux/Windows SSH servers were not rerun in this release verification. The live-server tests remain ignored and require a deliberately configured test account/key directory. Automated scripts/parsers are not a guarantee for every OpenSSH configuration.
- Remote changes and local vault/profile writes are separate operations. Network/permission errors can prevent rollback or old-key cleanup; verify server-side revocation after replacement/removal. Replacement cleanup errors are logged. **Remove** can be set to forget locally only; deleting a profile or resetting the vault leaves the public key on the server.
- Resetting/losing the vault loses managed private keys as well as passwords. Export an encrypted backup beforehand or use another credential to set up a new key. Exported files remain protected by the chosen passphrase; Unix exports are written through a mode-600 temporary file. Windows export access follows the chosen directory's ACL.
- Existing 1.3.4-beta profiles and encrypted vaults remain compatible; managed-key metadata is optional and private keys use separate vault entries. No automatic remote key installation or credential migration runs on upgrade.
- The previously documented OTP/background-connection, hardware telemetry and notification limits still apply. This beta's Windows installer lacks a trusted publisher signature. The macOS bundle is fully ad-hoc signed for integrity, but not Developer ID signed or Apple-notarized; the DMG is Apple Silicon (`aarch64`) only.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.4-beta...v1.3.5-beta
