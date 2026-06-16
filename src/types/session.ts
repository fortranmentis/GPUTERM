export type SessionProfile = {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  privateKeyPath?: string | null;
};

export type SessionConnectRequest = {
  id?: string | null;
  name: string;
  host: string;
  port: number;
  username: string;
  password?: string | null;
  privateKeyPath?: string | null;
  cols?: number;
  rows?: number;
};

export type TerminalSessionInfo = {
  sessionId: string;
  profile: SessionProfile;
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

export type SftpProgressPayload = {
  sessionId: string;
  operation: "download" | "upload";
  remotePath: string;
  localPath: string;
  transferredBytes: number;
  totalBytes: number | null;
  done: boolean;
  error?: string | null;
};
