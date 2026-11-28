import { useRef } from "react";

/** Keeps the previous array reference while its ordered string items are unchanged. */
export function useStableStringArray(items: string[]): string[] {
  const ref = useRef<string[]>(items);
  if (
    ref.current.length !== items.length ||
    ref.current.some((item, index) => item !== items[index])
  ) {
    ref.current = items;
  }
  return ref.current;
}
