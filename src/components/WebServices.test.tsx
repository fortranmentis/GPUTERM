import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WebServices } from "./WebServices";
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
    { kind: "tensorboard", pid: 1, host: "localhost", port: 6006, path: "/", detail: "runs/exp42" },
    {
      kind: "jupyter",
      pid: 2,
      host: "localhost",
      port: 8888,
      path: "/?token=abc",
      detail: "/home/me",
    },
  ],
};

describe("WebServices", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "scan_web_services") return scan;
      if (command === "open_web_service") return "http://127.0.0.1:6006/";
      return null;
    });
  });

  it("lists detected servers and opens one through a forward", async () => {
    render(<WebServices profile={profile} connected />);
    expect(await screen.findByText("runs/exp42")).toBeInTheDocument();
    expect(screen.getByText("2 running")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open Jupyter on port 8888" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("open_web_service", {
        sessionId: "gpu",
        host: "localhost",
        port: 8888,
        path: "/?token=abc",
      }),
    );
  });

  it("explains hosts detection does not cover", async () => {
    mockInvoke.mockResolvedValue({ supported: false, services: [] });
    render(<WebServices profile={profile} connected />);
    expect(await screen.findByText(/Linux servers/)).toBeInTheDocument();
  });

  it("shows open errors and renders nothing while disconnected", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "scan_web_services") return scan;
      throw "Port 6006 is already in use";
    });
    const { rerender } = render(<WebServices profile={profile} connected />);
    fireEvent.click(await screen.findByRole("button", { name: "Open TensorBoard on port 6006" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("already in use");

    rerender(<WebServices profile={profile} connected={false} />);
    expect(screen.queryByText("Web services")).not.toBeInTheDocument();
  });
});
