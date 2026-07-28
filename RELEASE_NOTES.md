# GpuTerm 1.2.1-beta

## Highlights

- The compact Windows AI DASH card now keeps every 5-hour and weekly gauge inside its bounds, with the session count moved into the title row to preserve vertical space.
- Windows Claude Code quota setup now installs a native PowerShell 5.1 helper using an absolute forward-slash path. It works whether Claude launches status lines through Git Bash or PowerShell and no longer requires Python on Windows.
- The former **AGENTS / Coding agents** surface is now consistently named **AI DASH**.
- The 360 px AI DASH summary card replaces CPU/RAM copy with a two-column grid of thin 5-hour and weekly gauges for AGY Gemini, AGY Claude/GPT, Claude Code, and Codex. Every available balance stays visible at once; unsupported, reset, warning, and critical states remain explicit.
- AGY now retains one normalized `/usage` sample per five-minute bucket for the last 24 hours in memory and plots separate Gemini and Claude/GPT lines for both 5-hour and weekly windows. Failed reads create graph gaps instead of fabricated values.
- The AGY probe waits for hidden-PTY startup output, sends `/usage` exactly once, and uses the precise percentage printed beside the bar instead of the rounded `remaining` text. AI DASH details also list every model named in each AGY group.
- Agent detail cards now lead with large **usage remaining** gauges; context and token data moves into a collapsed **Context details** section.
- Codex reads the account-wide `account/rateLimits/read` result from `codex app-server`, caches it for 60 seconds, and normalizes `3% used` to `97% remaining`.
- Claude Code can install or safely update GpuTerm's status-line helper from the card to publish subscriber 5-hour and 7-day limits. Existing unrelated status lines are preserved as conflicts.
- AGY experimentally opens a hidden PTY and parses `/usage` every five minutes, preserving the separate **Gemini models** and **Claude and GPT models** 5-hour/weekly windows.
- Every quota displays its source, snapshot age, reset countdown, and stale/reset state. Manual refresh is available for all providers.
- **Terminal scrollback search** — `Ctrl`/`Cmd`+`F` opens an in-pane find bar with a live match count, `Enter`/`Shift`+`Enter` steps through hits, and `Escape` returns focus to the shell. The shortcut is left alone while a text field is focused.
- **A disconnected pane now says so.** The close reason the backend already produced ("Remote shell closed", "Terminal stream failed: …") is shown with a **Reconnect** button, instead of the pane silently refusing input.
- **CI runs on every push and pull request** — `.github/workflows/ci.yml` executes the typecheck, ESLint, Vitest, `cargo test`, and `cargo clippy -D warnings`, plus a `cargo check` against the MSRV declared in `Cargo.toml`. Previously only two Rust tests ran, and only during a tagged release build.

## Fixes

- **AI DASH agents were not detected on macOS at all.** `ps` pads its `comm` column to sixteen characters, so an agent launched from an absolute path was matched against a truncated fragment. Detection now reads argv[0] from the full command line, which also picks up native `claude`/`codex` binaries and the agent process the Claude desktop app launches, while excluding the desktop shell itself and its renderer helpers.
- **Windows Claude Code setup could report success without ever publishing limits.** Its `%USERPROFILE%` command was valid in `cmd.exe` but not in Git Bash, which Claude may use for status lines. Setup now installs `scripts/gputerm-claude-statusline.ps1`, registers a shell-independent absolute PowerShell command, and safely upgrades the previous GpuTerm helper.
- **Codex could show an old session's `12% remaining` even when the account API reported `97% remaining`.** Quotas are now account-wide snapshots, and all live Codex sessions share the official value. The newest timestamped session log is used only when the live app-server lookup fails, with the fallback source and age shown.
- Codex `primary` and `secondary` identifiers are no longer treated as time periods; their actual `windowDurationMins` values determine whether a window is 5-hour, weekly, or another duration.
- **An SFTP transfer could delete a directory tree to make room for a file.** Downloading a remote file into a local directory of the same name ran `remove_dir_all` on it, and uploading a file over a remote directory removed that tree recursively. Both directions now refuse the type change and say which entry is in the way. The symmetric file-replaced-by-directory cases are refused too.
- **A stalled SSH channel froze the whole window.** `terminal_write` and `terminal_resize` were synchronous Tauri commands, so their multi-second nonblocking retry ran on the main thread. They, and the synchronous filesystem commands, now run off it.
- **Deleting a saved profile failed whenever the credential vault was locked**, because vault clean-up aborted the delete. It is now best effort.
- **Only one of several selected remote items was deleted**, and the target was whichever came first in directory order rather than what was clicked. Delete now removes the whole selection and asks for confirmation first.
- **Dropping N files opened N native overwrite dialogs at once** and started N simultaneous transfers on one connection. Confirmations are sequential and transfers are capped.
- A failed telemetry-settings save left the UI showing an interval the backend never accepted; the previous value is restored.
- Transfer progress no longer emits one IPC event per 1 MiB chunk (about 10,000 for a 10 GB file); it is throttled to 100 ms while completion events are still always sent. The 1 MiB transfer buffer moved off the stack.
- Cancelled transfers are flagged explicitly instead of being recognized by matching the literal text `Transfer canceled`.
- Quota probes moved off the telemetry poll thread. An AGY probe could stall every CPU/GPU/memory card for up to 20 seconds every five minutes.
- Local telemetry collectors are now bounded by a timeout; a wedged collector previously blocked its telemetry thread forever. The agent metadata scrape got its own longer budget and a bounded directory walk, so a large `~/.claude/projects` no longer silently yields empty metadata under the 3-second telemetry timeout.
- `parse_swapusage` could panic on non-ASCII `sysctl` output, permanently killing that session's telemetry thread.
- `sessions.json`, `known_hosts.json`, and `app_settings.json` are written through the vault's existing atomic 0600 temp-and-rename helper; a crash mid-write could previously destroy every trusted host key.
- Provider reset timestamps are normalized to epoch seconds once in Rust, so the summary tooltip and the detail popover can no longer disagree about the unit.
- Disconnecting a session no longer acts on a store snapshot captured before the IPC round trip, which could switch the view to a session that had since disconnected.
- A failed Codex refresh can no longer leave an old app-server value looking current. Snapshot age and reset expiry are recalculated on every telemetry poll.
- AGY `/usage` parse failures and timeouts no longer reuse or invent a numerical balance; the card reports automatic lookup as unsupported and directs the user to `/usage`.
- Claude setup creates a settings backup, updates an existing GpuTerm helper safely, reports missing Python as unsupported on POSIX hosts, and returns a conflict with a manual integration command rather than replacing another status line.
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

- Frontend: 121 Vitest tests pass across terminal sessions/splits, SFTP, monitoring, saved profiles, ProxyJump, native drag-and-drop, credential-vault behavior, and provider-specific AI DASH gauges and charts.
- Backend: 144 Rust tests pass (1 ignored because it requires host telemetry permissions unavailable in some sandboxes), including official Codex `used → remaining` normalization, newest-account fallback selection, Claude status-line conflicts, AGY precise-bar precedence, ANSI/cursor-positioned redraw parsing, `/usage`-only startup probing, five-minute history buckets, reconnect sharing, timeout gaps, and 24-hour/288-point pruning.
- UI regression coverage verifies the 360 px all-provider summary, duplicate-session removal, newest snapshot selection, `n/a`/reset/warning/critical states, remaining-percentage accessibility values, collapsed context details, manual refresh, Claude setup, AGY group separation, graph gaps, and narrow-window behavior.
- Static/build checks: Clippy passes with warnings denied, ESLint reports no errors, `git diff --check` passes, and the TypeScript/Vite production build completes successfully. All of these now run in CI on every push and pull request.
- Packaging: the tagged source is built on GitHub-hosted Windows, Ubuntu, and macOS runners into native NSIS `.exe`, Debian `.deb`, and Apple Silicon `.dmg` installers.
- macOS release packaging fully ad-hoc signs nested Mach-O files and code containers inside-out, deep-signs and strictly verifies the final app, creates the DMG from that verified bundle, then mounts it and verifies the enclosed app again before upload.
- Release assets include `SHA256SUMS.txt` covering the `.exe`, `.deb`, and `.dmg` installers.

## Notes

- No profile, known-host, or credential-vault migration is required when upgrading from 1.2.0-beta. Saved profiles and the Argon2id + AES-256-GCM local vault remain unchanged.
- AGY 1.0 token/context extraction requires `python3` or `python` on the monitored host. It opens only the two newest conversation databases read-only and selects generator metadata; steps, prompts, responses, tool arguments, credentials, and environment data are not extracted or serialized.
- AGY account-level quota/work state and Claude subscriber limits remain provider-reported data. The AGY TUI probe is experimental; its raw output is discarded after normalization, and GpuTerm does not estimate unavailable balances.
- AGY history contains only capture time, lookup status, model group, window length, and normalized remaining percentage. It is shared across reconnects for the same host/user during one app run, never written to disk, and discarded when GpuTerm exits.
- Claude 5-hour/weekly gauges require the GpuTerm status line on each monitored host (see the README). Use **Set up** from the card, or install `scripts/gputerm-claude-statusline.sh` manually on POSIX. Windows uses `scripts/gputerm-claude-statusline.ps1`; rerun **Set up** once to upgrade an older Python command, restart Claude Code, accept workspace trust, and send one message. The snapshot is limited to session id, working directory, model, context window, cost, the two rate-limit windows, capture time, and the agent pid where available.
- Process resources, session metadata, AGY subagents/background tasks, and Claude session cost/time remain available below the priority gauges. Cumulative session token totals are shown for Codex and AGY, which report them; the Claude card shows per-request tokens instead.
- This is a beta prerelease. Windows builds do not have a trusted publisher signature, and the macOS build is fully ad-hoc signed rather than Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.2.0-beta...v1.2.1-beta
