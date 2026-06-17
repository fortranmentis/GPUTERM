import { create } from "zustand";
import type { SftpProgressPayload } from "../types/session";
import type {
  ActiveTransferDrag,
  TransferTask,
  TransferStatus,
} from "../types/transfer";

type TransferStore = {
  tasks: TransferTask[];
  activeDrag: ActiveTransferDrag;
  addTask: (task: TransferTask) => void;
  updateTask: (id: string, patch: Partial<TransferTask>) => void;
  updateFromProgress: (payload: SftpProgressPayload) => void;
  setActiveDrag: (activeDrag: ActiveTransferDrag) => void;
  clearActiveDrag: () => void;
  clearFinished: () => void;
};

export const useTransferStore = create<TransferStore>((set) => ({
  tasks: [],
  activeDrag: null,
  addTask: (task) =>
    set((state) => ({
      tasks: [task, ...state.tasks.filter((existing) => existing.id !== task.id)],
    })),
  updateTask: (id, patch) =>
    set((state) => ({
      tasks: state.tasks.map((task) =>
        task.id === id ? { ...task, ...patch } : task,
      ),
    })),
  updateFromProgress: (payload) => {
    if (!payload.transferId) {
      return;
    }
    set((state) => ({
      tasks: state.tasks.map((task) => {
        if (task.id !== payload.transferId) {
          return task;
        }
        const totalBytes = payload.totalBytes ?? task.totalBytes;
        const progressPercent =
          totalBytes && totalBytes > 0
            ? Math.min(
                100,
                Math.round((payload.transferredBytes / totalBytes) * 100),
              )
            : null;
        const status = progressStatus(payload, task.status);
        return {
          ...task,
          totalBytes,
          transferredBytes: payload.transferredBytes,
          progressPercent,
          status,
          error: payload.error ?? task.error,
        };
      }),
    }));
  },
  setActiveDrag: (activeDrag) => set({ activeDrag }),
  clearActiveDrag: () => set({ activeDrag: null }),
  clearFinished: () =>
    set((state) => ({
      tasks: state.tasks.filter(
        (task) => task.status === "pending" || task.status === "running",
      ),
    })),
}));

function progressStatus(
  payload: SftpProgressPayload,
  fallback: TransferStatus,
): TransferStatus {
  if (payload.error) {
    return "failed";
  }
  if (payload.done) {
    return "done";
  }
  return fallback === "pending" ? "running" : fallback;
}
