import { describe, expect, it } from "vitest";
import { parseHostKeyChallenge } from "./hostKeyPrompt";

describe("parseHostKeyChallenge", () => {
  it("parses an unknown host key sentinel", () => {
    expect(
      parseHostKeyChallenge(
        "Error: UNKNOWN_HOST_KEY:abc123|ssh-ed25519|gpu.lab:2222",
      ),
    ).toEqual({
      kind: "unknown",
      expected: undefined,
      fingerprint: "abc123",
      keyType: "ssh-ed25519",
      hostKey: "gpu.lab:2222",
      host: "gpu.lab",
      port: 2222,
    });
  });

  it("parses a changed host key sentinel with an IPv6 host", () => {
    expect(
      parseHostKeyChallenge("HOST_KEY_CHANGED:aa11|bb22|ssh-rsa|2001:db8::1:22"),
    ).toEqual({
      kind: "changed",
      expected: "aa11",
      fingerprint: "bb22",
      keyType: "ssh-rsa",
      hostKey: "2001:db8::1:22",
      host: "2001:db8::1",
      port: 22,
    });
  });

  it("ignores ordinary and malformed errors", () => {
    expect(parseHostKeyChallenge("Connection refused")).toBeNull();
    expect(parseHostKeyChallenge("HOST_KEY_CHANGED:aa|bb|gpu.lab:22")).toBeNull();
    expect(parseHostKeyChallenge("UNKNOWN_HOST_KEY:aa|ssh-rsa|gpu.lab")).toBeNull();
  });
});
