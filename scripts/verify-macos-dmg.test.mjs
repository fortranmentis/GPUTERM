import { describe, it, expect } from "vitest";
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
import process from "node:process";

describe.skipIf(process.platform === "win32")("DMG integrity gate", () => {
  function fixture(mode, check) {
    const dir = mkdtempSync(join(tmpdir(), "gputerm-dmg-test-"));
    const trace = join(dir, "trace");
    const image = join(dir, "test.dmg");
    writeFileSync(image, "test fixture, not a real image");
    writeFileSync(join(dir, "hdiutil"), `#!/bin/bash
echo "$1" >> "$TRACE"
if [ "$1" = verify ] && [ "$FAIL_MODE" = permanent ]; then exit 1; fi
if [ "$1" = verify ] && [ "$FAIL_MODE" = transient ] && [ ! -f "$TRACE.once" ]; then touch "$TRACE.once"; exit 1; fi
exit 0
`, { mode: 0o700 });
    writeFileSync(join(dir, "codesign"), `#!/bin/bash
echo codesign >> "$TRACE"
test "$FAIL_MODE" != signature
`, { mode: 0o700 });
    writeFileSync(join(dir, "sleep"), "#!/bin/bash\nexit 0\n", { mode: 0o700 });
    const run = () => execFileSync("bash", [resolve("scripts/verify-macos-dmg.sh"), image, join(dir, "mount")], {
      env: { ...process.env, PATH: `${dir}:${process.env.PATH}`, TRACE: trace, FAIL_MODE: mode },
      stdio: "pipe",
    });
    const calls = () => readFileSync(trace, "utf8").trim().split("\n");
    try { check(run, calls); } finally { rmSync(dir, { recursive: true, force: true }); }
  }

  it("retries transient image errors and still checks the app signature", () => {
    fixture("transient", (run, calls) => {
      run();
      expect(calls()).toEqual(["verify", "verify", "attach", "codesign", "detach"]);
    });
  });
  it("fails after five unsuccessful integrity checks without mounting", () => {
    fixture("permanent", (run, calls) => {
      expect(run).toThrow();
      expect(calls()).toEqual(Array(5).fill("verify"));
    });
  });
  it("does not retry or ignore a broken app signature and detaches on failure", () => {
    fixture("signature", (run, calls) => {
      expect(run).toThrow();
      expect(calls()).toEqual(["verify", "attach", "codesign", "detach"]);
    });
  });
});
