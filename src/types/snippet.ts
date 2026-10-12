/** A saved command (`Snippet` in src-tauri/src/ssh/snippets.rs). */
export type Snippet = {
  id: string;
  name: string;
  command: string;
  /** Sends Return after the command, so it runs instead of waiting. */
  appendReturn: boolean;
  /** Profiles it is offered for; absent or empty means every server. */
  profileIds?: string[];
};
