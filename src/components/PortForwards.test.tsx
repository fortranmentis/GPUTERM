import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { describeForward, PortForwards } from "./PortForwards";
import type { SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const profile: SessionProfile = {
  id: "gpu",
  name: "gpu",
  host: "gpu.lab",
  port: 22,
  username: "me",
  forwards: [
    {
      id: "jupyter",
      kind: "local",
      bindHost: "127.0.0.1",
      bindPort: 8888,
      destHost: "localhost",
      destPort: 8888,
      enabled: true,
    },
  ],
};

describe("PortForwards", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "list_forwards") {
        return [
          {
            profileId: "gpu",
            spec: profile.forwards![0],
            state: "listening",
            boundPort: 8888,
            activeConnections: 2,
            bytesTransferred: 2048,
            temporary: false,
            exposed: false,
          },
        ];
      }
      if (command === "validate_forward") {
        return { ...(args as { spec: object }).spec, id: "new-id" };
      }
      if (command === "save_profile_forwards") {
        return [];
      }
      return [];
    });
  });

  it("describes each kind of rule", () => {
    expect(describeForward(profile.forwards![0])).toBe(
      "127.0.0.1:8888 → localhost:8888 (via server)",
    );
    expect(
      describeForward({ ...profile.forwards![0], kind: "dynamic", bindPort: 1080 }),
    ).toBe("SOCKS5 on 127.0.0.1:1080");
  });

  it("shows live status while connected", async () => {
    render(<PortForwards profile={profile} connected />);
    fireEvent.click(screen.getByRole("button", { name: /port forwards/i }));
    expect(await screen.findByText("listening")).toBeInTheDocument();
    expect(screen.getByText(/2 open/)).toBeInTheDocument();
  });

  it("saves a new rule on the profile", async () => {
    render(<PortForwards profile={profile} connected={false} />);
    fireEvent.click(screen.getByRole("button", { name: /port forwards/i }));
    fireEvent.click(screen.getByRole("button", { name: /add rule/i }));
    fireEvent.change(screen.getAllByPlaceholderText("8888")[0], {
      target: { value: "6006" },
    });
    fireEvent.change(screen.getAllByPlaceholderText("8888")[1], {
      target: { value: "6006" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("save_profile_forwards", {
        profileId: "gpu",
        forwards: [
          profile.forwards![0],
          expect.objectContaining({ id: "new-id", bindPort: 6006, destPort: 6006 }),
        ],
      }),
    );
    expect(mockInvoke).not.toHaveBeenCalledWith("start_forward", expect.anything());
  });
});
