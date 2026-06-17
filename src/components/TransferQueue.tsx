import { XCircle } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useTransferStore } from "../stores/transferStore";
import { useSessionStore } from "../stores/sessionStore";
import { formatBytes } from "../utils/formatBytes";

export function TransferQueue() {
  const tasks = useTransferStore((state) => state.tasks);
  const updateTask = useTransferStore((state) => state.updateTask);
  const clearFinished = useTransferStore((state) => state.clearFinished);
  const setMessage = useSessionStore((state) => state.setMessage);

  const cancelTransfer = async (id: string) => {
    try {
      await invoke("cancel_transfer", { transferId: id });
      updateTask(id, { status: "canceled" });
    } catch (error) {
      setMessage({ kind: "error", text: String(error) });
    }
  };

  return (
    <section className="transfer-queue" aria-label="Transfer queue">
      <div className="transfer-queue-title">
        <strong>Transfers</strong>
        <span>{tasks.length}</span>
        <button
          className="secondary-button compact"
          type="button"
          disabled={tasks.every(
            (task) => task.status === "pending" || task.status === "running",
          )}
          onClick={clearFinished}
        >
          Clear finished
        </button>
      </div>
      <div className="transfer-task-list">
        {tasks.length === 0 && (
          <div className="empty-list compact">No transfers yet</div>
        )}
        {tasks.map((task) => (
          <article className={`transfer-task ${task.status}`} key={task.id}>
            <div className="transfer-task-main">
              <strong>{task.filename}</strong>
              <span>{task.direction}</span>
              <small title={task.sourcePath}>{task.sourcePath}</small>
              <small title={task.targetPath}>{task.targetPath}</small>
            </div>
            <div className="transfer-task-side">
              <span className={`transfer-status ${task.status}`}>{task.status}</span>
              <span>
                {formatBytes(task.transferredBytes)} / {formatBytes(task.totalBytes)}
              </span>
              <div className="progress-track">
                <div
                  className="progress-fill"
                  style={{ width: `${task.progressPercent ?? 0}%` }}
                />
              </div>
              <button
                className="icon-button ghost"
                type="button"
                disabled={task.status !== "running" && task.status !== "pending"}
                title="Cancel transfer"
                aria-label={`Cancel ${task.filename}`}
                onClick={() => cancelTransfer(task.id)}
              >
                <XCircle size={15} />
              </button>
            </div>
            {task.error && <p className="transfer-error">{task.error}</p>}
          </article>
        ))}
      </div>
    </section>
  );
}
