export type DirectoryEntrySize = { name: string; path: string; bytes: number };

/** One level of `du` under a folder (`directory_sizes`). */
export type DirectorySizes = {
  path: string;
  /** Missing when the scan hit its time limit. */
  totalBytes: number | null;
  /** Files directly in the folder, when known. */
  filesBytes: number | null;
  /** Largest first. */
  entries: DirectoryEntrySize[];
  /** Folders left out of `entries`. */
  moreEntries: number;
  /** False when the scan was stopped; sizes are then lower bounds. */
  complete: boolean;
  /** Paths that could not be read. */
  unreadable: number;
};
