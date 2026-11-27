import { describe, it, expect } from "vitest";
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
import process from "node:process";

const shScript = resolve("scripts/gputerm-claude-statusline.sh");
const macScript = resolve("scripts/gputerm-claude-statusline-macos.js");
const psScript = resolve("scripts/gputerm-claude-statusline.ps1");

function runStatusline(command, args, payload, homeDir) {
  return execFileSync(command, args, {
    input: JSON.stringify(payload),
    env: { ...process.env, HOME: homeDir },
    encoding: "utf8",
    stdio: ["pipe", "pipe", "pipe"],
  });
}

describe("Claude status-line script hardening", () => {
  it("avoids GPUTERM_PAYLOAD env var and validates session_id across all platforms", () => {
    const shSource = readFileSync(shScript, "utf8");
    const macSource = readFileSync(macScript, "utf8");
    const psSource = readFileSync(psScript, "utf8");

    expect(shSource).not.toContain("GPUTERM_PAYLOAD");
    expect(shSource).toContain("sys.stdin.read()");
    expect(shSource).toContain("^[A-Za-z0-9._-]+$");
    expect(shSource).toContain(".tmp.");

    expect(macSource).toContain("/^[A-Za-z0-9._-]+$/");
    expect(psSource).toContain("^[A-Za-z0-9._-]+$");
    expect(psSource).toContain(".tmp.");
  });

  describe.skipIf(process.platform === "win32")("POSIX shell statusline", () => {
    it("writes valid session and account snapshots without leaving temp files", () => {
      const home = mkdtempSync(join(tmpdir(), "gputerm-sl-sh-"));
      try {
        const output = runStatusline(
          "sh",
          [shScript],
          {
            session_id: "sess_abc-1.2",
            model: { display_name: "Claude Opus" },
            context_window: { used_percentage: 42 },
            rate_limits: { five_hour: { used_percentage: 20 } },
            cost: { total_cost_usd: 1.5 },
          },
          home,
        );
        expect(output.trim()).toBe("Claude Opus · ctx 42% · 5h 80% · $1.50");

        const cacheDir = join(home, ".cache", "gputerm", "agent-status", "claude");
        const files = readdirSync(cacheDir).sort();
        expect(files).toEqual(["account.json", "sess_abc-1.2.json"]);
        const snapshot = JSON.parse(readFileSync(join(cacheDir, "sess_abc-1.2.json"), "utf8"));
        expect(snapshot.session_id).toBe("sess_abc-1.2");
      } finally {
        rmSync(home, { recursive: true, force: true });
      }
    });

    it("rejects path traversal and dot session_ids without writing files", () => {
      const home = mkdtempSync(join(tmpdir(), "gputerm-sl-sh-trav-"));
      try {
        for (const badId of ["../escape", "..", ".", "a/b", "a\\b", ""]) {
          const output = runStatusline(
            "sh",
            [shScript],
            {
              session_id: badId,
              model: { display_name: "Claude" },
              rate_limits: { five_hour: { used_percentage: 10 } },
            },
            home,
          );
          expect(output.trim()).toContain("Claude");
        }
        expect(existsSync(join(home, ".cache", "gputerm", "agent-status", "escape.json"))).toBe(
          false,
        );
        expect(existsSync(join(home, ".cache", "gputerm", "agent-status", "claude"))).toBe(false);
      } finally {
        rmSync(home, { recursive: true, force: true });
      }
    });

    it("handles oversized hook payloads via stdin without ARG_MAX failure", () => {
      const home = mkdtempSync(join(tmpdir(), "gputerm-sl-sh-big-"));
      try {
        const output = runStatusline(
          "sh",
          [shScript],
          {
            session_id: "big-payload-1",
            model: { id: "claude-sonnet" },
            unused_large_prompt: "x".repeat(300_000),
          },
          home,
        );
        expect(output.trim()).toBe("claude-sonnet");
        const snapshotPath = join(
          home,
          ".cache",
          "gputerm",
          "agent-status",
          "claude",
          "big-payload-1.json",
        );
        expect(existsSync(snapshotPath)).toBe(true);
        const snapshot = JSON.parse(readFileSync(snapshotPath, "utf8"));
        expect(snapshot.unused_large_prompt).toBeUndefined();
      } finally {
        rmSync(home, { recursive: true, force: true });
      }
    });
  });

  describe.skipIf(process.platform !== "darwin")("macOS JXA statusline", () => {
    it("writes valid session_id and rejects traversal session_ids", () => {
      const home = mkdtempSync(join(tmpdir(), "gputerm-sl-mac-"));
      try {
        for (const badId of ["../escape", "..", ".", "sub/id"]) {
          runStatusline(
            "osascript",
            ["-l", "JavaScript", macScript],
            { session_id: badId, model: { display_name: "Claude" } },
            home,
          );
        }
        expect(existsSync(join(home, ".cache", "gputerm", "agent-status", "escape.json"))).toBe(
          false,
        );
        expect(existsSync(join(home, ".cache", "gputerm", "agent-status", "claude"))).toBe(false);

        const output = runStatusline(
          "osascript",
          ["-l", "JavaScript", macScript],
          { session_id: "mac-ok.1", model: { display_name: "Claude" } },
          home,
        );
        expect(output.trim()).toBe("Claude");
        expect(
          existsSync(
            join(home, ".cache", "gputerm", "agent-status", "claude", "mac-ok.1.json"),
          ),
        ).toBe(true);
      } finally {
        rmSync(home, { recursive: true, force: true });
      }
    });
  });
});
