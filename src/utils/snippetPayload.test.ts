import { describe, expect, it } from "vitest";
import { snippetAppliesTo, snippetPayload } from "./snippetPayload";

describe("snippetPayload", () => {
  it("sends a single line with an optional Return", () => {
    expect(snippetPayload("nvidia-smi", true, true)).toBe("nvidia-smi\r");
    expect(snippetPayload("nvidia-smi\n", false, true)).toBe("nvidia-smi");
  });

  it("brackets multi-line commands only when the remote enabled it", () => {
    expect(snippetPayload("cd /srv\r\nls\n", true, true)).toBe(
      "\u001b[200~cd /srv\rls\u001b[201~\r",
    );
    expect(snippetPayload("cd /srv\nls", false, false)).toBe("cd /srv\rls");
  });
});

describe("snippetAppliesTo", () => {
  const base = { id: "1", name: "n", command: "c", appendReturn: true };

  it("offers unscoped snippets everywhere", () => {
    expect(snippetAppliesTo(base, null)).toBe(true);
    expect(snippetAppliesTo({ ...base, profileIds: [] }, "a")).toBe(true);
  });

  it("limits scoped snippets to their profiles", () => {
    const scoped = { ...base, profileIds: ["a"] };
    expect(snippetAppliesTo(scoped, "a")).toBe(true);
    expect(snippetAppliesTo(scoped, "b")).toBe(false);
    expect(snippetAppliesTo(scoped, null)).toBe(false);
  });
});
