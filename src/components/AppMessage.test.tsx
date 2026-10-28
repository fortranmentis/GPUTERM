import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppMessageOverlay } from "./AppMessage";
import { useSessionStore } from "../stores/sessionStore";

describe("AppMessageOverlay", () => {
  it("runs a toast's action and dismisses it", () => {
    const run = vi.fn();
    useSessionStore.setState({
      message: { kind: "info", text: "TensorBoard started", action: { label: "Open", run } },
    });
    render(<AppMessageOverlay />);
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(run).toHaveBeenCalledOnce();
    expect(useSessionStore.getState().message).toBeNull();
  });
});
