import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { DatabasesPopover } from "./DatabasesPopover";
import { useSessionStore } from "../stores/sessionStore";
import type { DbInstance, DbReading, DbStatus } from "../types/databases";
import type { SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockConfirm = vi.mocked(confirm);

const PROFILES: SessionProfile[] = [
  { id: "p1", name: "Lab", host: "lab.internal", port: 22, username: "me" },
  { id: "local", name: "Mine", host: "localhost", port: 0, username: "me", isLocal: true },
];

const INSTANCE: DbInstance = {
  id: "db1",
  name: "main",
  kind: "postgres",
  profileId: "p1",
  host: "127.0.0.1",
  port: 5432,
  user: "monitor",
  database: "app",
  hasPassword: true,
};

const STATUS: DbStatus = {
  instanceId: "db1",
  version: "16.4",
  connections: 190,
  maxConnections: 200,
  active: 3,
  idleInTransaction: 1,
  waitingOnLocks: 0,
  longestQuerySecs: 12,
  commits: 1000,
  rollbacks: 0,
  queries: null,
  slowQueries: null,
  cacheHitPercent: 99.5,
  deadlocks: 2,
  sizeBytes: 1024 * 1024 * 1024,
  uptimeSecs: 60,
  replicationLagSecs: null,
  isReplica: false,
};

function renderPopover(
  options: {
    instances?: DbInstance[];
    readings?: Record<string, DbReading>;
    connected?: string[];
    onInstancesChange?: (instances: DbInstance[], changedId?: string) => void;
  } = {},
) {
  render(
    <DatabasesPopover
      instances={options.instances ?? [INSTANCE]}
      readings={options.readings ?? {}}
      profiles={PROFILES}
      connectedProfileIds={options.connected ?? ["p1"]}
      onInstancesChange={options.onInstancesChange ?? vi.fn()}
      onRefresh={vi.fn()}
      anchorRef={createRef<HTMLElement>()}
      onClose={vi.fn()}
    />,
  );
  return screen.getByRole("dialog", { name: "Databases" });
}

describe("DatabasesPopover", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockConfirm.mockReset();
    useSessionStore.setState({ message: null });
  });

  it("shows the status grid and marks a database that needs attention", () => {
    const dialog = renderPopover({
      readings: { db1: { status: STATUS, error: null, at: 0, rate: 12.34 } },
    });
    const row = within(dialog).getByTestId("database-db1");
    expect(row).toHaveClass("attention");
    expect(row).toHaveTextContent("PostgreSQL · monitor@127.0.0.1:5432 via Lab");
    expect(row).toHaveTextContent("Connections190/200 connections");
    expect(row).toHaveTextContent("Throughput12.3 commits/s");
    expect(row).toHaveTextContent("Cache hit99.5%");
    expect(row).toHaveTextContent("Size1.00 GiB");
    expect(row).toHaveTextContent("Longest query12s");
    // A primary has no replication lag row; MySQL-only rows stay hidden.
    expect(row).not.toHaveTextContent("Replication lag");
    expect(row).not.toHaveTextContent("Slow queries");
  });

  it("explains a database whose server is not connected", () => {
    const dialog = renderPopover({ connected: [] });
    expect(within(dialog).getByTestId("database-db1")).toHaveTextContent(
      "Connect to Lab to read it.",
    );
  });

  it("adds a database through a remote server without sending an empty password", async () => {
    const onInstancesChange = vi.fn();
    mockInvoke.mockResolvedValue([INSTANCE]);
    const dialog = renderPopover({ instances: [], onInstancesChange });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add database" }));
    const form = within(dialog).getByRole("form", { name: "Add database" });
    // Local terminals are not offered as servers.
    expect(within(form).queryByRole("option", { name: /Mine/ })).not.toBeInTheDocument();
    expect(within(form).getByLabelText("Host (from the server)")).toHaveValue("127.0.0.1");

    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "main" } });
    fireEvent.change(within(form).getByLabelText("Server"), { target: { value: "p1" } });
    fireEvent.change(within(form).getByLabelText("User"), { target: { value: "monitor" } });
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("save_db_instance", {
        input: {
          id: undefined,
          name: "main",
          kind: "postgres",
          profileId: "p1",
          host: "127.0.0.1",
          port: undefined,
          user: "monitor",
          database: "",
          password: undefined,
        },
      }),
    );
    await waitFor(() => expect(onInstancesChange).toHaveBeenCalledWith([INSTANCE], undefined));
  });

  it("removes a saved password and rejects a bad port", async () => {
    mockInvoke.mockResolvedValue([{ ...INSTANCE, hasPassword: false }]);
    const dialog = renderPopover();
    fireEvent.click(within(dialog).getByRole("button", { name: "Edit main" }));
    const form = within(dialog).getByRole("form", { name: "Edit main" });

    fireEvent.change(within(form).getByLabelText("Port"), { target: { value: "70000" } });
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("between 1 and 65535");
    expect(mockInvoke).not.toHaveBeenCalled();

    fireEvent.change(within(form).getByLabelText("Port"), { target: { value: "5433" } });
    fireEvent.click(within(form).getByLabelText("Remove the saved password"));
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("save_db_instance", {
        input: expect.objectContaining({ id: "db1", port: 5433, password: "" }),
      }),
    );
  });

  it("shows a vault error from saving", async () => {
    mockInvoke.mockRejectedValue("Unlock the credential vault to save the database password");
    const dialog = renderPopover();
    fireEvent.click(within(dialog).getByRole("button", { name: "Edit main" }));
    const form = within(dialog).getByRole("form", { name: "Edit main" });
    fireEvent.change(within(form).getByLabelText("Password"), { target: { value: "s3cret" } });
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent(
      "Unlock the credential vault",
    );
  });

  it("deletes a database only after confirmation", async () => {
    const onInstancesChange = vi.fn();
    mockInvoke.mockResolvedValue([]);
    const dialog = renderPopover({ onInstancesChange });
    mockConfirm.mockResolvedValueOnce(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove main" }));
    await waitFor(() => expect(mockConfirm).toHaveBeenCalledTimes(1));
    expect(mockInvoke).not.toHaveBeenCalled();

    mockConfirm.mockResolvedValueOnce(true);
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove main" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("delete_db_instance", { id: "db1" }),
    );
    await waitFor(() => expect(onInstancesChange).toHaveBeenCalledWith([]));
  });
});
