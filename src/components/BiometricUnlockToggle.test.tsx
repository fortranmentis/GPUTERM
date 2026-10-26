import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { BiometricUnlockToggle } from "./BiometricUnlockToggle";
import { useSessionStore } from "../stores/sessionStore";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

describe("BiometricUnlockToggle", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    useSessionStore.setState({ message: null });
  });

  it("is hidden on a computer without Touch ID or Windows Hello", async () => {
    mockInvoke.mockResolvedValue({ available: false, enabled: false, kind: null });
    const { container } = render(<BiometricUnlockToggle />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("biometric_status"));
    expect(container).toBeEmptyDOMElement();
  });

  it("turns Windows Hello unlock on and off", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "biometric_status") {
        return Promise.resolve({ available: true, enabled: false, kind: "Windows Hello" });
      }
      if (command === "enable_biometric_unlock") {
        return Promise.resolve({ available: true, enabled: true, kind: "Windows Hello" });
      }
      if (command === "disable_biometric_unlock") {
        return Promise.resolve({ available: true, enabled: false, kind: "Windows Hello" });
      }
      return Promise.resolve(undefined);
    });
    render(<BiometricUnlockToggle />);
    const toggle = await screen.findByRole("checkbox", {
      name: "Unlock the vault with Windows Hello",
    });
    expect(toggle).not.toBeChecked();

    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toBeChecked());
    expect(mockInvoke).toHaveBeenCalledWith("enable_biometric_unlock");

    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(mockInvoke).toHaveBeenCalledWith("disable_biometric_unlock");
  });

  it("reports a failure to turn it on", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "biometric_status") {
        return Promise.resolve({ available: true, enabled: false, kind: "Touch ID" });
      }
      if (command === "enable_biometric_unlock") {
        return Promise.reject("Unlock the credential vault before turning on Touch ID");
      }
      return Promise.resolve(undefined);
    });
    render(<BiometricUnlockToggle />);
    const toggle = await screen.findByRole("checkbox", {
      name: "Unlock the vault with Touch ID",
    });
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(useSessionStore.getState().message).toEqual({
        kind: "error",
        text: "Unlock the credential vault before turning on Touch ID",
      }),
    );
    expect(toggle).not.toBeChecked();
  });
});
