import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SessionSidebar } from "./SessionSidebar";
import { useSessionStore } from "../stores/sessionStore";
import type { SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

const savedProfile: SessionProfile = {
  id: "profile-1",
  name: "lab-a100",
  host: "10.0.0.21",
  port: 22,
  username: "ubuntu",
  privateKeyPath: null,
};

describe("SessionSidebar profile form", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command) => {
      if (command === "save_session") {
        return Promise.resolve([savedProfile]);
      }
      return Promise.resolve([]);
    });
    useSessionStore.setState({
      sessions: [savedProfile],
      activeSessionId: null,
      connectedSessionIds: [],
      message: null,
    });
  });

  it("binds the form to a selected profile and unbinds it with New", () => {
    render(<SessionSidebar />);

    expect(screen.getByText("New profile")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    expect(screen.getByText(/editing saved profile: lab-a100/i)).toBeInTheDocument();
    expect(screen.getByPlaceholderText("10.0.0.21")).toHaveValue("10.0.0.21");

    fireEvent.click(screen.getByRole("button", { name: /^new$/i }));
    expect(screen.getByText("New profile")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("10.0.0.21")).toHaveValue("");
  });

  it("saves a fresh profile with a new id after New", async () => {
    render(<SessionSidebar />);

    fireEvent.click(screen.getByRole("button", { name: /lab-a100/i }));
    fireEvent.click(screen.getByRole("button", { name: /^new$/i }));
    fireEvent.change(screen.getByPlaceholderText("10.0.0.21"), {
      target: { value: "10.0.0.99" },
    });
    fireEvent.change(screen.getByPlaceholderText("ubuntu"), {
      target: { value: "sang" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      const saveCall = mockInvoke.mock.calls.find(
        ([command]) => command === "save_session",
      );
      expect(saveCall).toBeTruthy();
      const profile = (saveCall?.[1] as { profile: SessionProfile }).profile;
      expect(profile.host).toBe("10.0.0.99");
      expect(profile.id).not.toBe(savedProfile.id);
      expect(profile.id.length).toBeGreaterThan(0);
    });
  });
});
