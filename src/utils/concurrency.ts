/**
 * Runs `worker` over every item, with at most `limit` running at once.
 *
 * Transfers each open their own SSH connection, so an unbounded `Promise.all`
 * over a large selection means dozens of simultaneous handshakes (and, with a
 * jump host, a bastion connection each). Results keep the input order.
 */
export async function mapWithConcurrencyLimit<T, R>(
  items: readonly T[],
  limit: number,
  worker: (item: T, index: number) => Promise<R>,
): Promise<R[]> {
  const results = new Array<R>(items.length);
  if (items.length === 0) {
    return results;
  }

  const workers = Math.max(1, Math.min(Math.floor(limit) || 1, items.length));
  let next = 0;
  const runners = Array.from({ length: workers }, async () => {
    while (next < items.length) {
      const index = next;
      next += 1;
      results[index] = await worker(items[index], index);
    }
  });
  await Promise.all(runners);
  return results;
}
