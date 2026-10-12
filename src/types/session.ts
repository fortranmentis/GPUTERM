export type SessionProfile = {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  isLocal?: boolean;
  privateKeyPath?: string | null;
  proxyJumpId?: string | null;
};

export type SessionConnectRequest = {
  id?: string | null;
  name: string;
  host: string;
  port: number;
  username: string;
  isLocal?: boolean;
  password?: string | null;
  privateKeyPath?: string | null;
  proxyJumpId?: string | null;
  proxyJumpPassword?: string | null;
  reuseStoredCredentials?: boolean;
  cols?: number;
  rows?: number;
};

export type TerminalSessionInfo = {
  sessionId: string;
  terminalId: string;
  profile: SessionProfile;
  credentialWarning?: string | null;
};

export type TerminalPaneInfo = {
  sessionId: string;
  terminalId: string;
};

export type AppMessage = {
  kind: "info" | "success" | "error";
  text: string;
};

export type SftpEntry = {
  name: string;
  path: string;
  type: "file" | "directory" | "symlink" | "other";
  size: number | null;
  modifiedTime: number | null;
};

export type SftpListResponse = {
  path: string;
  entries: SftpEntry[];
};

export type LocalEntry = {
  name: string;
  path: string;
  entryType: "file" | "directory" | "other";
  size: number | null;
  modifiedTime: number | null;
};

export type LocalListResponse = {
  path: string;
  entries: LocalEntry[];
};

/** One `Host` alias read from an ssh_config file. */
export type SshConfigHost = {
  alias: string;
  host: string;
  user?: string | null;
  port: number;
  identityFile?: string | null;
  proxyJump?: string | null;
  /** A saved profile with the same host, port and user, updated on import. */
  conflictProfileId?: string | null;
  warnings: string[];
};

export type SshConfigPreview = {
  hosts: SshConfigHost[];
  warnings: string[];
  sourcePath?: string;
};

export type SshConfigImportResult = {
  created: number;
  updated: number;
  warnings: string[];
  profiles: SessionProfile[];
};

export type AppSettings = {
  recentLocalPath?: string | null;
  terminalTheme?: string | null;
  terminalFontFamily?: string | null;
  terminalFontSize?: number | null;
};

export type SftpProgressPayload = {
  transferId?: string | null;
  sessionId: string;
  operation: "download" | "upload";
  remotePath: string;
  localPath: string;
  transferredBytes: number;
  totalBytes: number | null;
  done: boolean;
  /** Set when the user cancelled, so callers need not match on `error` text. */
  canceled: boolean;
  error?: string | null;
  /** Folder transfers only. */
  currentFile?: string;
  filesDone?: number;
  filesTotal?: number;
  /** Entries left alone: kept existing files and links, never followed. */
  skipped?: number;
};

/**
 * Emitted when a terminal's channel or shell ends. Mirrors the Rust
 * `TerminalClosedPayload`, which always populates every field — declared here
 * once so the three listeners cannot drift apart or drop `message`.
 */
export type TerminalClosedPayload = {
  sessionId: string;
  terminalId: string;
  /** The whole SSH session ended, not just this pane. */
  sessionClosed: boolean;
  /** Human-readable reason, e.g. "Remote shell closed". */
  message: string | null;
};

/** Mirrors the Rust `TmuxStatus`; the three cases need distinct UI copy. */
export type TmuxStatus = "ok" | "not-installed" | "unsupported-os";

export type TmuxSession = {
  /**
   * tmux's own handle (`$0`, `$1`, …). The only field ever put back into a
   * command — `name` is remote-controlled data and stays display-only.
   */
  id: string;
  name: string;
  windows: number;
  attached: boolean;
  /** Unix seconds, or null on tmux versions that emit a date string. */
  created: number | null;
};

export type TmuxListing = {
  status: TmuxStatus;
  sessions: TmuxSession[];
};
