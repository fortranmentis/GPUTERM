# GpuTerm 1.1.8-beta

## Highlights

- Windows hybrid-GPU hosts now retain every physical adapter discovered through `Win32_VideoController`, including an idle Intel or AMD integrated GPU for which WDDM has not yet created a GPU Engine counter instance.
- AGY 1.0 monitoring now reads recent conversation generator metadata through Python 3's standard `sqlite3` module and a bounded protobuf decoder, adding model, input/output/total tokens, context size, context used, and context remaining.
- Claude Code monitoring now understands official status-line context-remaining fields and subscriber 5-hour/7-day rate-limit snapshots.
- The AGY, Claude Code, and Codex detail views now present context as separate **used** and **remaining** values and render available provider limits as remaining percentages with readable window labels.

## Fixes

- Fixed hybrid Windows PCs showing only the active discrete GPU when the integrated GPU was powered down or otherwise absent from the current WDDM counter sample.
- Fixed AGY sessions exposing only process CPU/RAM while token and context information remained empty under the current `~/.gemini/antigravity-cli/conversations/*.db` storage format.
- Fixed Claude Code details showing context use without the corresponding remaining context or subscriber usage windows when those fields were emitted by the CLI.
- Metadata fragments for the same agent session are now merged, allowing a read-only conversation record and an optional live status snapshot to contribute complementary counters, state, and quota data.

## Validation

- Frontend: 104 Vitest tests pass across terminal sessions/splits, SFTP, monitoring, saved profiles, ProxyJump, native drag-and-drop, credential-vault behavior, and agent usage presentation.
- Backend: 117 Rust tests pass (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including idle-iGPU retention, empty WDDM-counter fallback, AGY token/context parsing, Claude remaining-context/rate-limit parsing, and metadata-fragment merging.
- A live AGY 1.0 conversation database was read through the same bounded extractor to verify model, token totals, context size, used context, and derived remaining context without exporting conversation content.
- Static/build checks: Clippy passes with warnings denied, `git diff --check` passes, and the TypeScript/Vite production build completes successfully.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS release packaging fully ad-hoc signs nested Mach-O files and code containers inside-out, deep-signs and strictly verifies the final app, creates the DMG from that verified bundle, then mounts it and verifies the enclosed app again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No profile, known-host, or credential-vault migration is required when upgrading from 1.1.7-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- AGY 1.0 token/context extraction requires `python3` or `python` on the monitored host. It opens only the two newest conversation databases read-only and selects generator metadata; steps, prompts, responses, tool arguments, credentials, and environment data are not extracted or serialized.
- AGY account-level quota/work state and Claude subscriber limits remain best effort because providers do not expose every value through a stable noninteractive interface. Optional status snapshots can supply fields that are not present in local conversation records; unavailable values display as `n/a`.
- Windows WDDM creates activity counters lazily. A retained idle adapter can therefore show `0%` or `n/a` until its first activity sample while its identity remains visible.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.7-beta...v1.1.8-beta
