import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AuthPromptDialog, type AuthPromptRequest } from "./AuthPromptDialog";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const request = (overrides: Partial<AuthPromptRequest> = {}): AuthPromptRequest => ({
  requestId: "r1",
  sessionId: "gpu",
  host: "gpu.lab",
  port: 22,
  username: "alice",
  isJump: false,
  instructions: "",
  prompts: [{ text: "Verification code: ", echo: false }],
  ...overrides,
});

describe("AuthPromptDialog", () => {
  let emit: (payload: AuthPromptRequest) => void;

  beforeEach(() => {
    vi.mocked(invoke).mockClear();
    vi.mocked(listen).mockImplementation(async (_event, handler) => {
      emit = (payload) => (handler as (event: { payload: unknown }) => void)({ payload });
      return () => undefined;
    });
  });

  async function show(payload: AuthPromptRequest) {
    render(<AuthPromptDialog />);
    await waitFor(() => expect(listen).toHaveBeenCalled());
    act(() => emit(payload));
  }

  it("sends the typed code for the waiting sign-in", async () => {
    await show(request());
    const field = screen.getByLabelText("Verification code:");
    expect(field).toHaveAttribute("type", "password");
    fireEvent.change(field, { target: { value: "123456" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(invoke).toHaveBeenCalledWith("answer_auth_prompt", {
      requestId: "r1",
      responses: ["123456"],
    });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("cancels with no answers", async () => {
    await show(request());
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(invoke).toHaveBeenCalledWith("answer_auth_prompt", { requestId: "r1", responses: null });
  });

  it("asks the jump host first, then the target, and labels the jump host", async () => {
    await show(request({ requestId: "jump", isJump: true, host: "bastion" }));
    act(() => emit(request({ requestId: "target" })));
    expect(screen.getByText("Jump host")).toBeInTheDocument();
    expect(screen.getByText("alice@bastion")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("alice@gpu.lab")).toBeInTheDocument();
    expect(screen.queryByText("Jump host")).toBeNull();
  });

  it("shows an echoed question as plain text", async () => {
    await show(request({ prompts: [{ text: "Duo passcode or option (1-3):", echo: true }] }));
    expect(screen.getByLabelText("Duo passcode or option (1-3):")).toHaveAttribute("type", "text");
  });
});
