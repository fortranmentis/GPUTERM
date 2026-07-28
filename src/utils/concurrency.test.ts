import { describe, expect, it } from "vitest";
import { mapWithConcurrencyLimit } from "./concurrency";

describe("mapWithConcurrencyLimit", () => {
  it("never runs more than the limit at once", async () => {
    let running = 0;
    let peak = 0;
    const items = Array.from({ length: 10 }, (_, index) => index);

    const results = await mapWithConcurrencyLimit(items, 3, async (item) => {
      running += 1;
      peak = Math.max(peak, running);
      await Promise.resolve();
      running -= 1;
      return item * 2;
    });

    expect(peak).toBe(3);
    expect(results).toEqual(items.map((item) => item * 2));
  });

  it("keeps input order regardless of completion order", async () => {
    const delays = [30, 0, 10];
    const results = await mapWithConcurrencyLimit(delays, 3, async (delay, index) => {
      await new Promise((resolve) => setTimeout(resolve, delay));
      return index;
    });

    expect(results).toEqual([0, 1, 2]);
  });

  it("handles an empty list and a nonsense limit", async () => {
    expect(await mapWithConcurrencyLimit([], 4, async () => 1)).toEqual([]);
    expect(await mapWithConcurrencyLimit([1, 2], 0, async (item) => item)).toEqual([
      1, 2,
    ]);
  });
});
