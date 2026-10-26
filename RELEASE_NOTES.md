# GpuTerm 1.3.6-beta

## Highlights

- **SSH key manager.** Open **SSH keys** in the sidebar to generate Ed25519, RSA 3072/4096 and ECDSA P-256/P-384/P-521 keys. Copy/save public keys, rename/delete local keys, and export private keys with a passphrase or by explicitly choosing an unencrypted export. Private keys remain in the master-password vault during normal use.
- **Assign and install keys per profile.** Pick **Sign in with key**, then **Install on server** to register and verify the selected key before assignment. Normal authentication tries the profile's dedicated managed key, then its selected key-manager key, then the existing file/password/agent flow; installation verification remains key-only on the final host.
- **Containers and Kubernetes.** The new **Containers** card discovers Docker, Podman, nerdctl, kubectl, k3s and microk8s on connected remote Linux hosts. Inspect container CPU/memory and node/deployment/pod state; confirmed actions start/restart/stop/kill containers or restart deployments without adding sudo.
- **SQL monitoring.** Register PostgreSQL or MySQL/MariaDB instances reached through connected Linux/macOS SSH servers and their native clients. View engine-supported connection/activity counters, lock waits, longest queries, commit/query rates, cache hit and replication lag. Passwords use the encrypted vault and remote stdin/private temporary files.
- **Opt-in Touch ID / Windows Hello unlock.** Enable the sidebar toggle after unlocking the vault; startup tries once and returns to master-password entry on cancellation/failure. Linux remains password-only. The vault file retains its existing Argon2id / AES-256-GCM format.
- **Linux distribution identity and SFTP selection.** os-release labels identify Fedora, openSUSE, Arch, Gentoo and other distributions in monitoring/window titles; `loginctl` fills in when utmp/`who` data is absent. SFTP gains row/header checkboxes, partial-selection indication, Shift-click ranges, Ctrl/Cmd+A and selected counts on transfer/delete buttons.

## Fixes

- Attach Windows Hello verification to GpuTerm's main window through the [desktop UserConsentVerifier interop API](https://learn.microsoft.com/en-us/windows/apps/develop/ui/display-ui-objects), instead of the UWP-only request. Refuse missing window handles before prompting and expose the toggle only where the desktop bridge is available.
- Compile platform-specific biometric key conversion only on macOS/Windows, keeping Linux's password-only build free of unused-code warnings under strict Clippy checks.
- Keep the existing per-profile managed-key flow while supporting assigned shared keys; avoid attempting an identical key twice across those two sources.
- Report unavailable container tools, runtime permissions and kubeconfig/RBAC failures rather than using privilege escalation. Hide Containers where no supported runtime/client is found; restrict collection/actions to remote Linux hosts.
- Keep DB passwords out of command arguments and exported password environment variables. Use stdin and mode-600 temporary client-credential files with exit cleanup; escape client-option/passfile values and keep DB passwords out of instance JSON.
- Restore master-password entry after biometric cancellation/failure, reject stale stored unlock keys by authenticating the vault ciphertext, and attempt to remove biometric key material when disabled/reset/reinitialized.
- Follow displayed sort order for SFTP range selection and keep header state and `(N)` action counts synchronized with the selected items.
- Update both READMEs to explain the new opt-in OS-key storage and unencrypted-export exceptions, replacing outdated blanket claims about OS stores and plaintext exports.

## Validation

- **325 frontend tests pass** across **45 files**, covering the key manager, profile assignment/install flows, container actions/confirmation, SQL forms/status, biometric toggle/startup fallback, Linux labels and SFTP selection/counts.
- On macOS, **462 Rust tests pass**, with **6 environment-dependent/developer tests ignored**. Coverage includes key algorithms/export/assignment, authentication ordering, container/Kubernetes parsers and command validation, DB parsing/persistence, stdin credential scripts and temporary-file permissions/cleanup, fake biometric stores/vault authentication, os-release and loginctl parsing. Platform-specific test totals can differ.
- TypeScript checking, ESLint and the Vite production build pass. ESLint retains 14 existing React Hook warnings; Clippy passes across all targets/features with warnings denied, and `git diff --check` passes. GitHub CI also checks the declared Rust 1.88 minimum.
- GitHub-hosted Windows, Ubuntu and macOS runners build native NSIS EXE, Debian DEB and Apple Silicon DMG installers. The Windows job additionally runs the local telemetry and Claude helper checks. `SHA256SUMS.txt` is generated after all installer jobs succeed.
- macOS packaging signs nested Mach-O/code containers inside-out, deep-signs the final app, verifies with `codesign --verify --deep --strict`, creates the DMG from that bundle, then mounts it and verifies the enclosed app before upload.

## Notes

- Biometric unlock is **off unless enabled**. Enabling it persists a copy of the derived vault key: a macOS login-Keychain item (plus a non-secret marker), or `vault_key.dpapi` protected for the Windows account. GpuTerm verifies with Touch ID/Windows Hello before reading/unprotecting it. This is an application-enforced gate, not a cryptographic hardware/biometric binding; another authorized process under the same OS account is outside that guarantee. Windows Hello may use the verification methods configured by Windows. Master-password-only operation remains available with the toggle off.
- Real Touch ID/Windows Hello prompts, Keychain authorization after ad-hoc upgrades, live SSH key installation, real container/cluster operations and live PostgreSQL/MySQL servers were not exercised by this release verification. Tests use fixtures/fake stores/clients; installed-device and real-server validation remains necessary. Keychain may ask separately for access.
- Container summaries refresh every minute and open popovers every 15 seconds. The first available runtime/client on PATH is used; pod display is capped at 300. Actions run with the SSH account's existing socket and Kubernetes privileges, and depend on server policy. Local/macOS/Windows container collection is not provided.
- SQL polls reachable saved instances every 10 seconds and requires `psql`, `mysql` or `mariadb` on the SSH server. Metric availability varies by engine/version/DB role; this is a read-only statistics view, not a query browser. Password input is single-line; abrupt collector termination can leave a mode-600 temporary file behind. Windows SSH servers are not supported for SQL collection.
- Explicit unencrypted private-key exports are plaintext and must be protected by the chosen directory/OS permissions. Deleting a manager key locally does not revoke it on servers; failed Install on server verification can leave the added public key in authorized_keys. Confirm/revoke server-side keys independently when needed.
- Existing 1.3.5-beta profiles and vaults remain compatible. New public metadata lives in `keys.json` and `db_instances.json`; private keys and DB passwords remain separate vault entries. No automatic remote key installation, container action or DB configuration change runs on upgrade.
- Distribution identification does not add RPM, Arch or Gentoo packages: published installers remain EXE, DEB and Apple Silicon DMG. The Windows installer lacks a trusted publisher signature; the macOS bundle is fully ad-hoc signed for integrity but not Developer ID signed or Apple-notarized. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.5-beta...v1.3.6-beta
