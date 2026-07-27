# GpuTerm 1.2.0-beta

## Highlights

- The former **AGENTS / Coding agents** surface is now consistently named **AI DASH**.
- The 360 px AI DASH summary card replaces CPU/RAM copy with a two-column grid of thin 5-hour and weekly gauges for AGY Gemini, AGY Claude/GPT, Claude Code, and Codex. Every available balance stays visible at once; unsupported, reset, warning, and critical states remain explicit.
- AGY now retains one normalized `/usage` sample per five-minute bucket for the last 24 hours in memory and plots separate Gemini and Claude/GPT lines for both 5-hour and weekly windows. Failed reads create graph gaps instead of fabricated values.
- The AGY probe waits for hidden-PTY startup output, sends `/usage` exactly once, and uses the precise percentage printed beside the bar instead of the rounded `remaining` text. AI DASH details also list every model named in each AGY group.
- Agent detail cards now lead with large **usage remaining** gauges; context and token data moves into a collapsed **Context details** section.
- Codex reads the account-wide `account/rateLimits/read` result from `codex app-server`, caches it for 60 seconds, and normalizes `3% used` to `97% remaining`.
- Claude Code can install or safely update GpuTerm's status-line helper from the card to publish subscriber 5-hour and 7-day limits. Existing unrelated status lines are preserved as conflicts.
- AGY experimentally opens a hidden PTY and parses `/usage` every five minutes, preserving the separate **Gemini models** and **Claude and GPT models** 5-hour/weekly windows.
- Every quota displays its source, snapshot age, reset countdown, and stale/reset state. Manual refresh is available for all providers.

## Fixes

- **AI DASH agents were not detected on macOS at all.** `ps` pads its `comm` column to sixteen characters, so an agent launched from an absolute path was matched against a truncated fragment. Detection now reads argv[0] from the full command line, which also picks up native `claude`/`codex` binaries and the agent process the Claude desktop app launches, while excluding the desktop shell itself and its renderer helpers.
- **Claude Code usage limits could never appear.** The 5-hour and 7-day windows exist only in the status-line payload, and nothing published them. `scripts/gputerm-claude-statusline.sh` now supplies them; the empty state explains how to install it instead of only reporting an absence.
- **Codex could show an old session's `12% remaining` even when the account API reported `97% remaining`.** Quotas are now account-wide snapshots, and all live Codex sessions share the official value. The newest timestamped session log is used only when the live app-server lookup fails, with the fallback source and age shown.
- Codex `primary` and `secondary` identifiers are no longer treated as time periods; their actual `windowDurationMins` values determine whether a window is 5-hour, weekly, or another duration.
- A failed Codex refresh can no longer leave an old app-server value looking current. Snapshot age and reset expiry are recalculated on every telemetry poll.
- AGY `/usage` parse failures and timeouts no longer reuse or invent a numerical balance; the card reports automatic lookup as unsupported and directs the user to `/usage`.
- Claude setup creates a settings backup, updates an existing GpuTerm helper safely, reports missing Python as unsupported, and returns a conflict with a manual integration command rather than replacing another status line.
- Fixed a subagent transcript overwriting the session that spawned it: worker records share their parent's session id, so their smaller context and token counts replaced the real ones. `*/subagents/*` transcripts and sidechain records are now excluded.
- Fixed usage being attributed to the wrong session when several agents run at once. Snapshots that record the agent pid are matched to that process instead of being paired by process-id order.
- Fixed `context_window.total_input_tokens` being reported as a cumulative session total. It describes the live context window, so it now feeds only the context gauge; the Claude card shows the latest request's token breakdown, and a partial transcript sum is no longer presented as a session total.
- Fixed a Codex weekly quota being blanked by a later `token_count` event that carried no rate-limit snapshot.
- Fixed the duplicate-message guard also skipping that record's context, quota, and cost fields.
- Rate-limit windows whose reset time has passed are reported as rolled over rather than shown with a stale balance; relative `resets_in_seconds` values are accepted, and snapshots older than two minutes are labelled with their age.
- The five-second metadata scan now runs only for providers with a live process, and refreshes immediately when an agent starts or exits.
- Fixed provider quota data being shown as generic labels such as `primary`, `five_hour`, or `seven_day` instead of user-facing weekly and 5-hour windows.
- Fixed nested AGY model-group quota objects being ignored when the remaining value lived below the group object.
- Fixed remaining context being visually buried below CPU, memory, PID, and session metadata.
- Reset timestamps now show a readable countdown while retaining the exact local date and time as hover text.
- Missing provider quota data is identified explicitly instead of being estimated or rendered as an ambiguous empty section.

## Validation

- Frontend: 111 Vitest tests pass across terminal sessions/splits, SFTP, monitoring, saved profiles, ProxyJump, native drag-and-drop, credential-vault behavior, and provider-specific AI DASH gauges and charts.
- Backend: 137 Rust tests pass (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including official Codex `used → remaining` normalization, newest-account fallback selection, Claude status-line conflicts, AGY precise-bar precedence, ANSI/cursor-positioned redraw parsing, `/usage`-only startup probing, five-minute history buckets, reconnect sharing, timeout gaps, and 24-hour/288-point pruning.
- UI regression coverage verifies the 360 px all-provider summary, duplicate-session removal, newest snapshot selection, `n/a`/reset/warning/critical states, remaining-percentage accessibility values, collapsed context details, manual refresh, Claude setup, AGY group separation, graph gaps, and narrow-window behavior.
- Static/build checks: Clippy passes with warnings denied, `git diff --check` passes, and the TypeScript/Vite production build completes successfully.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS release packaging fully ad-hoc signs nested Mach-O files and code containers inside-out, deep-signs and strictly verifies the final app, creates the DMG from that verified bundle, then mounts it and verifies the enclosed app again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No profile, known-host, or credential-vault migration is required when upgrading from 1.1.9-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- AGY 1.0 token/context extraction requires `python3` or `python` on the monitored host. It opens only the two newest conversation databases read-only and selects generator metadata; steps, prompts, responses, tool arguments, credentials, and environment data are not extracted or serialized.
- AGY account-level quota/work state and Claude subscriber limits remain provider-reported data. The AGY TUI probe is experimental; its raw output is discarded after normalization, and GpuTerm does not estimate unavailable balances.
- AGY history contains only capture time, lookup status, model group, window length, and normalized remaining percentage. It is shared across reconnects for the same host/user during one app run, never written to disk, and discarded when GpuTerm exits.
- Claude 5-hour/weekly gauges require the GpuTerm status line on each monitored host (see the README). Use **Set up** from the card, or install `scripts/gputerm-claude-statusline.sh` manually on POSIX. Windows uses `scripts/gputerm-claude-statusline.py`. The snapshot is limited to session id, working directory, model, context window, cost, the two rate-limit windows, capture time, and the agent pid where available.
- Process resources, session metadata, AGY subagents/background tasks, and Claude session cost/time remain available below the priority gauges. Cumulative session token totals are shown for Codex and AGY, which report them; the Claude card shows per-request tokens instead.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.1.9-beta...v1.2.0-beta
