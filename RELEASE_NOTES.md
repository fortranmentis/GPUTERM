# GpuTerm 1.3.3-beta

## Highlights

- **SSH workflows brought into the desktop app.** Preview and selectively import `~/.ssh/config`, including identity files, ProxyJump chains, and forwarding rules. Saved profiles can now own local (`-L`), remote (`-R`), and SOCKS5 dynamic (`-D`) forwards with live state and traffic counters.
- **A more personal and productive terminal.** Save global or host-scoped command snippets, choose from five terminal themes and desktop fonts, adjust the persistent font size, and use platform-safe shortcuts for splitting, navigating, searching, clearing, and zooming panes.
- **Complete SFTP directory operations.** Rename or drag remote items between folders, upload and download planned directory trees with aggregate progress, and choose Replace, Keep existing, or Skip when folders collide.
- **Five additional LLM adapters.** Monitor SGLang, Hugging Face TGI, llama.cpp, generic OpenAI-compatible servers, and NVIDIA Triton alongside Ollama and vLLM, directly or through an SSH tunnel.
- **Recovery without hiding security decisions.** Unexpected terminal loss after sleep or a network change retries with bounded backoff, while authentication failures and changed host keys stop for explicit review.

## Fixes

- Run AMD ROCm telemetry through the exact probed `rocm-smi` path and report hosts that expose only `amd-smi` instead of silently issuing an unavailable command.
- Accept bracketed IPv6 literals and try every resolved address with a timeout rather than failing after the first address.
- Show the previous and replacement SHA-256 fingerprints when a known host key changes; serialize known-host/profile writes so concurrent updates cannot overwrite one another.
- Refuse to delete profiles still used by ProxyJump or LLM tunnels, and tear down their terminals, forwards, monitoring connections, and in-memory credentials before removing the UI entry.
- Upload to a unique remote `.gputerm-part` path and rename only after completion, preserving the old destination on cancellation or failure and retaining a recoverable complete part if finalization cannot succeed.
- Catch otherwise fatal React render errors in both the main and detached detail windows and offer a window reload action.
- Keep imported identities, forwarded addresses, SFTP paths, and runtime failures platform-safe across Windows, macOS, Linux, IPv4, and IPv6.

## Validation

- Frontend: all **213 Vitest tests** pass across 28 files, including new ErrorBoundary, forwarding, SSH config import, auto-reconnect, host-key prompt, terminal appearance/shortcut, snippet, SFTP, and runtime coverage.
- Backend: **350 Rust tests pass** with 2 environment-dependent tests ignored. New coverage exercises ROCm path discovery, IPv6 formatting, changed-key errors, dependency-safe deletion, remote part paths, directory planning, snippets, SSH config chains, all forwarding modes, the five new runtime adapters, and reconnect credential restoration.
- Static/build checks: TypeScript type checking and the Vite production build succeed; ESLint reports 0 errors and 14 existing React Hook warnings; Clippy passes with warnings denied across all targets; the newly added Rust modules pass rustfmt; and `git diff --check` passes.
- GitHub-hosted Windows, Ubuntu, and macOS runners build a native NSIS `.exe`, Debian `.deb`, Apple Silicon `.dmg`, and `SHA256SUMS.txt`.
- The macOS job signs every nested Mach-O and code container inside-out, applies a final deep ad-hoc signature to `GpuTerm.app`, verifies it with `codesign --deep --strict`, creates the DMG only from that verified bundle, mounts it, and verifies the enclosed app again before upload.

## Notes

- Port forwarding follows SSH semantics. Binding a port below 1024 may still require operating-system privileges, and a remote forward can be rejected by the server's SSH policy.
- Recursive SFTP transfer skips symbolic links, limits a plan to 100,000 entries and depth 64, and leaves a fully uploaded `.gputerm-part` path visible when the server cannot finalize an overwrite safely.
- Generic OpenAI-compatible monitoring is limited to the shared `/v1/models` surface. Other adapters display only metrics their installed server version actually publishes; unavailable values remain unsupported rather than becoming zero.
- Automatic reconnect never revives a deliberate disconnect and stops on authentication or host-key errors. Saved credentials still require the local encrypted vault to be unlocked.
- No credential-vault or known-host migration is required from 1.3.2-beta. Existing session files load with no forwarding rules until rules are added; snippets are stored separately in `snippets.json`.
- This is a beta prerelease. The Windows installer is not signed by a trusted publisher. The macOS bundle is fully ad-hoc signed for integrity but is not Developer ID signed or notarized, so SmartScreen or Gatekeeper may still require a one-time confirmation.
- The macOS installer is Apple Silicon (`aarch64`) only. Intel Mac users can build from source.

**Full changelog:** https://github.com/fortranmentis/GPUTERM/compare/v1.3.2-beta...v1.3.3-beta
