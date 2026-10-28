import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WebServices } from "./WebServices";
import { useSessionStore } from "../stores/sessionStore";
import { useWebServicesStore } from "../stores/webServicesStore";
import type { SessionProfile } from "../types/session";
import type { WebServiceScan } from "../types/webServices";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const profile: SessionProfile = {
  id: "gpu",
  name: "gpu",
  host: "gpu.lab",
  port: 22,
  username: "me",
};

const scan: WebServiceScan = {
  supported: true,
  services: [
    {
      kind: "tensorboard",
      pid: 1,
      host: "localhost",
      port: 6006,
      path: "/",
      detail: "runs/exp42",
      container: null,
    },
    {
      kind: "jupyter",
      pid: null,
      host: "172.17.0.2",
      port: 8888,
      path: "/",
      detail: "jupyter/scipy-notebook",
      container: "notebook",
    },
    {
      kind: "other",
      pid: 3,
      host: "localhost",
      port: 9000,
      path: "/",
      detail: "node server.js",
      container: null,
    },
  ],
};

describe("WebServices", () => {
  beforeEach(() => {
    vi.stubGlobal("localStorage", {
      getItem: () => null,
      setItem: () => {},
      removeItem: () => {},
    });
    useWebServicesStore.setState({ scans: { gpu: scan }, errors: {}, scanning: {}, notify: true });
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "scan_web_services") return scan;
      if (command === "open_web_service") return "http://127.0.0.1:6006/";
      if (command === "validate_forward") return { ...(args as { spec: object }).spec, id: "rule" };
      if (command === "save_profile_forwards") return [profile];
      return null;
    });
  });

  it("lists servers, containers and other ports, and opens one", async () => {
    render(<WebServices profile={profile} connected />);
    expect(screen.getByText("runs/exp42")).toBeInTheDocument();
    expect(screen.getByText("notebook")).toBeInTheDocument();
    expect(screen.getByText("2 running")).toBeInTheDocument();
    expect(screen.queryByText("node server.js")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Other listening ports \(1\)/ }));
    expect(screen.getByText("node server.js")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open Jupyter on port 8888" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("open_web_service", {
        sessionId: "gpu",
        host: "172.17.0.2",
        port: 8888,
        path: "/",
      }),
    );
  });

  it("pins a service as a saved forward", async () => {
    const setSessions = vi.fn();
    useSessionStore.setState({ setSessions });
    render(<WebServices profile={profile} connected />);
    fireEvent.click(screen.getByRole("button", { name: "Forward port 6006 on every connect" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("save_profile_forwards", {
        profileId: "gpu",
        forwards: [
          expect.objectContaining({ bindPort: 6006, destHost: "localhost", destPort: 6006 }),
        ],
      }),
    );
    expect(setSessions).toHaveBeenCalledWith([profile]);
  });

  it("shows open errors and renders nothing while disconnected", async () => {
    mockInvoke.mockRejectedValue("Port 6006 is already in use");
    const { rerender } = render(<WebServices profile={profile} connected />);
    fireEvent.click(screen.getByRole("button", { name: "Open TensorBoard on port 6006" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("already in use");

    rerender(<WebServices profile={profile} connected={false} />);
    expect(screen.queryByText("Web services")).not.toBeInTheDocument();
  });

  it("reports only new services after the first scan", async () => {
    useWebServicesStore.setState({ scans: {} });
    let added = await act(() => useWebServicesStore.getState().refresh("gpu"));
    expect(added).toEqual([]);
    mockInvoke.mockResolvedValueOnce({
      ...scan,
      services: [
        ...scan.services,
        { ...scan.services[0], kind: "mlflow", port: 5000, detail: null },
      ],
    });
    added = await act(() => useWebServicesStore.getState().refresh("gpu"));
    expect(added.map((service) => service.port)).toEqual([5000]);
  });
});
