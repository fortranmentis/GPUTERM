# GpuTerm 1.1.7-beta

## Highlights

- Added simultaneous integrated and discrete GPU monitoring on Linux. Vendor collectors are supplemented by DRM/sysfs discovery so Intel/AMD integrated graphics remain visible beside NVIDIA, AMD, or Intel discrete adapters.
- Added an **Agents** telemetry card and resizable/pop-out detail view for AGY (Antigravity), Codex, and Claude Code on local and remote Linux, macOS, and Windows hosts.
- Agent CPU and memory values now aggregate the complete child-process tree, including workers, language servers, subagents, and background commands instead of reporting only the launcher process.
- Added provider-aware metadata:
  - AGY: agent state, model, token/context usage, subagents, and background tasks.
  - Claude Code: session/model, input/output/total tokens, context use, session cost, and session duration.
  - Codex: session/model, token/context usage, and available primary/secondary rate-limit windows and reset data.

## Fixes

- Fixed Linux hybrid systems omitting an integrated GPU when a richer vendor tool was available for the discrete adapter.
- Fixed same-vendor hybrid configurations, such as AMD iGPU + AMD dGPU, being collapsed into a single vendor card; DRM card counts and memory characteristics now preserve uncovered adapters while avoiding duplicates.
- Fixed the NVIDIA-rich GPU detail path omitting non-NVIDIA integrated adapters from the expanded GPU detail view.
- Prevented coding-agent prompt/response content, authentication data, environment data, and full process command lines from being serialized into GpuTerm telemetry.
- Kept agent metadata scans bounded to recent session records and throttled them independently from the regular process/resource poll.

## Validation

- Frontend: 104 Vitest tests pass across terminal sessions/splits, SFTP, monitoring, saved profiles, ProxyJump, native drag-and-drop, credential-vault behavior, and the new Agents card/detail view.
- Backend: 114 Rust tests pass (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including process-tree aggregation, provider metadata, prompt/command-line non-serialization, Linux DRM discovery, and same-vendor hybrid GPU coverage.
- Static/build checks: Clippy passes with warnings denied, `git diff --check` passes, and the TypeScript/Vite production build completes successfully.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS release packaging fully ad-hoc signs nested Mach-O files and code containers inside-out, deep-signs and strictly verifies the final app, creates the DMG from that verified bundle, then mounts it and verifies the enclosed app again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No profile, known-host, or credential-vault migration is required when upgrading from 1.1.6-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- Agent process CPU/RAM/process counts are available whenever the CLI process is visible to the monitoring user. Provider-specific model, token, context, work-state, cost, and rate-limit fields are best effort because each CLI's local record/status schema can change; unavailable values display as `n/a`.
- Agent monitoring is read-only. GpuTerm extracts counters and identifiers from recent local CLI session records at most every five seconds and does not include prompt, response, authentication, environment, or complete command-line content in emitted telemetry.
- Linux DRM shared-memory GPUs may expose utilization without dedicated VRAM, power, or temperature. Those unavailable values display as `n/a`.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.6-beta...v1.1.7-beta
