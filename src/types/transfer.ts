export type TransferDirection = "upload" | "download";

export type TransferStatus =
  | "pending"
  | "running"
  | "done"
  | "failed"
  | "canceled";

export type TransferTask = {
  id: string;
  direction: TransferDirection;
  filename: string;
  sourcePath: string;
  targetPath: string;
  totalBytes: number | null;
  transferredBytes: number;
  progressPercent: number | null;
  status: TransferStatus;
  error?: string;
  sessionId?: string;
};

export type LocalTransferDragFile = {
  name: string;
  path: string;
  entryType: "file" | "directory" | "other";
  size: number | null;
};

export type RemoteTransferDragFile = {
  name: string;
  path: string;
  type: "file" | "directory" | "symlink" | "other";
  size: number | null;
};

export type ActiveTransferDrag =
  | {
      kind: "local";
      files: LocalTransferDragFile[];
    }
  | {
      kind: "remote";
      files: RemoteTransferDragFile[];
    }
  | null;
