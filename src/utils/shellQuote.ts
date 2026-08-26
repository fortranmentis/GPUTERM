/**
 * POSIX single-quoting for a value that will be typed into a live login shell,
 * mirroring `shell_quote` in `src-tauri/src/ssh/system_monitor.rs`. The `'\''`
 * idiom is understood by sh, bash, zsh, fish and tcsh alike, so it holds even
 * though the user's login shell is unknown.
 *
 * This is a second line of defence only. Values reaching a shell are validated
 * first — tmux sessions are addressed by their `$N` id, never by name.
 */
export function shellQuote(value: string) {
  return `'${value.replace(/'/g, "'\\''")}'`;
}
