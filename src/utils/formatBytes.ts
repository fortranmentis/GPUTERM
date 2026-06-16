export function formatBytes(value: number | null | undefined) {
  if (value == null) {
    return "n/a";
  }
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let size = value / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size >= 10 ? 1 : 2)} ${units[unit]}`;
}

export function formatGiBOrTiB(value: number | null | undefined) {
  if (value == null) {
    return "n/a";
  }
  const tib = 1024 ** 4;
  const gib = 1024 ** 3;
  if (value >= tib) {
    return `${(value / tib).toFixed(1)} TiB`;
  }
  return `${(value / gib).toFixed(value >= 10 * gib ? 1 : 2)} GiB`;
}
