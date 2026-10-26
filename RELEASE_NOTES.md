# GpuTerm 1.3.7-beta

## Highlights

- **A more compact Sessions header.** Import and SSH keys now use icon-only buttons, leaving room for the labelled **New** action in the fixed-width sidebar.
- Hover tooltips and accessible button names remain available. The import and SSH key manager workflows are unchanged.

## Fixes

- Fix the Sessions header overflowing its sidebar and clipping **New** when three labelled actions competed for space.
- Center the compact tool icons in 34px buttons and allow the header action group to shrink with `min-width: 0`.
- Update the English and Korean READMEs to explain where to find the Import and SSH keys icons.

## Validation

- **326 frontend tests pass across 45 files.** Regression coverage checks icon-only content, accessible names, tooltips and the visible **New** label, alongside existing key-manager and profile actions.
- **462 macOS Rust tests pass; 6 are ignored.** TypeScript, the Vite production build and Clippy with all targets/features and warnings denied pass. ESLint reports 0 errors and 14 existing React Hook warnings; React test `act` warnings remain non-fatal. Platform-specific test totals can differ.
- GitHub CI verifies frontend/backend checks and the declared Rust 1.88 minimum. The Windows packaging job also tests local telemetry, the Claude quota helper and the biometric missing-owner guard.
- The release workflow builds Windows NSIS EXE, Debian amd64 DEB and Apple Silicon DMG, then generates `SHA256SUMS.txt` after all installer jobs succeed.
- macOS packaging signs nested code inside-out and the complete app ad-hoc, verifies it with `codesign --verify --deep --strict`, then mounts and verifies the packaged app before upload. Published installers are downloaded again for checksum and package-version checks.

## Notes

- This is a focused sidebar layout maintenance release relative to 1.3.6-beta. It introduces no credential-format, profile-schema or server-side migration.
- Existing SSH key management, container/SQL monitoring and opt-in biometric unlock remain available. Biometric unlock still persists a derived vault key in macOS Keychain or Windows DPAPI when explicitly enabled; it is an application-enforced gate, not a hardware-bound biometric encryption guarantee.
- Unit tests do not establish real server, Touch ID/Windows Hello or installed-device behavior. Cross-platform interactive layout and assistive-technology testing remain separate from DOM assertions and automated package verification.
- The Windows installer is not trusted-publisher signed. The macOS app is fully ad-hoc signed for integrity, not Developer ID signed or Apple-notarized; Intel Mac users can build from source. Linux assets remain DEB, not RPM/Arch/Gentoo packages.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.6-beta...v1.3.7-beta
