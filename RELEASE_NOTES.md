# GpuTerm 1.1.9-beta

## Highlights

- Agent detail cards now lead with **context remaining** and **quota remaining** instead of raw process and token totals, using the same at-a-glance gauge treatment as GPU and RAM telemetry.
- Codex rate-limit records with a 10,080-minute window are normalized as a weekly allowance with remaining percentage and reset countdown.
- Claude Code subscriber snapshots are normalized into separate 5-hour and weekly remaining gauges.
- AGY quota snapshots can preserve separate **Gemini models** and **Claude and GPT models** groups, each with weekly and 5-hour remaining gauges.
- Gauge colors surface low remaining capacity, and accessible progress-bar metadata keeps the values readable to assistive technologies.

## Fixes

- Fixed provider quota data being shown as generic labels such as `primary`, `five_hour`, or `seven_day` instead of user-facing weekly and 5-hour windows.
- Fixed nested AGY model-group quota objects being ignored when the remaining value lived below the group object.
- Fixed remaining context being visually buried below CPU, memory, PID, and session metadata.
- Reset timestamps now show a readable countdown while retaining the exact local date and time as hover text.
- Missing provider quota data is identified explicitly instead of being estimated or rendered as an ambiguous empty section.

## Validation

- Frontend: 105 Vitest tests pass across terminal sessions/splits, SFTP, monitoring, saved profiles, ProxyJump, native drag-and-drop, credential-vault behavior, and provider-specific agent gauges.
- Backend: 118 Rust tests pass (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including Codex weekly-window parsing, Claude 5-hour/weekly parsing, and nested AGY model-group quota parsing.
- UI regression coverage verifies context-remaining accessibility values, AGY group separation, and Claude/Codex window labels.
- Static/build checks: Clippy passes with warnings denied, `git diff --check` passes, and the TypeScript/Vite production build completes successfully.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS release packaging fully ad-hoc signs nested Mach-O files and code containers inside-out, deep-signs and strictly verifies the final app, creates the DMG from that verified bundle, then mounts it and verifies the enclosed app again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No profile, known-host, or credential-vault migration is required when upgrading from 1.1.8-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- AGY 1.0 token/context extraction requires `python3` or `python` on the monitored host. It opens only the two newest conversation databases read-only and selects generator metadata; steps, prompts, responses, tool arguments, credentials, and environment data are not extracted or serialized.
- AGY account-level quota/work state and Claude subscriber limits remain provider-reported data. Optional status snapshots can supply fields that are not present in local conversation records; GpuTerm does not estimate unavailable balances.
- Process resources, token totals, session metadata, AGY subagents/background tasks, and Claude session cost/time remain available below the new priority gauges.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.8-beta...v1.1.9-beta
