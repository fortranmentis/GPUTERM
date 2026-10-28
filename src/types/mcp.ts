/** Mirrors `McpSettings` in src-tauri/src/mcp/mod.rs. */
export type McpSettings = {
  enabled: boolean;
  allowActions: boolean;
  hiddenProfiles: string[];
};

/** Mirrors `McpStatus` in src-tauri/src/mcp/mod.rs. */
export type McpStatus = {
  settings: McpSettings;
  running: boolean;
  port: number | null;
  clients: string[];
  executable: string | null;
  auditLog: string;
  error: string | null;
};

/** Mirrors `ApprovalRequest` in src-tauri/src/mcp/approval.rs. */
export type McpApprovalRequest = {
  requestId: string;
  client: string;
  tool: string;
  host: string;
  summary: string;
  detail: string;
};
