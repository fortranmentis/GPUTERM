import type { IBufferRange, ILink, ILinkProvider, Terminal } from "@xterm/xterm";

/** A URL found in terminal text; `start` and `end` are string offsets (end exclusive). */
export type FoundUrl = { url: string; start: number; end: number };

/** A web server on the session's host, reached through an SSH forward. */
export type TunnelTarget = { host: string; port: number; path: string };

const URL_PATTERN = /https?:\/\/[^\s"'<>`]+/g;
/** Punctuation that usually ends the sentence around a URL, not the URL. */
const TRAILING = /[.,;:!?)\]}]+$/;
/** A logical line longer than this many rows is not scanned. */
const MAX_WRAPPED_ROWS = 20;

const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"]);

export function findUrls(text: string): FoundUrl[] {
  const found: FoundUrl[] = [];
  for (const match of text.matchAll(URL_PATTERN)) {
    const url = match[0].replace(TRAILING, "");
    if (url.length > "http://".length) {
      found.push({ url, start: match.index, end: match.index + url.length });
    }
  }
  return found;
}

/**
 * The server-side target of a link that only works on the session's host:
 * plain http to a loopback or wildcard address, or to the host's own name.
 * Anything else opens as it is.
 */
export function tunnelTarget(url: string, hostNames: string[]): TunnelTarget | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:") {
    return null;
  }
  const host = parsed.hostname.toLowerCase();
  const own = hostNames.some((name) => {
    const lower = name.toLowerCase();
    return lower !== "" && (host === lower || host === lower.split(".")[0]);
  });
  if (!LOOPBACK_HOSTS.has(host) && !own) {
    return null;
  }
  return {
    host: own ? "localhost" : host.replace(/^\[|\]$/g, ""),
    port: parsed.port ? Number(parsed.port) : 80,
    path: `${parsed.pathname}${parsed.search}${parsed.hash}`,
  };
}

/**
 * Buffer range (1-based, inclusive) of `[start, end)` in a logical line that
 * begins at 0-based row `firstRow` and wraps every `cols` cells. Assumes one
 * cell per character, which holds for URLs.
 */
export function rangeFor(start: number, end: number, firstRow: number, cols: number): IBufferRange {
  const last = end - 1;
  return {
    start: { x: (start % cols) + 1, y: firstRow + Math.floor(start / cols) + 1 },
    end: { x: (last % cols) + 1, y: firstRow + Math.floor(last / cols) + 1 },
  };
}

/** Clickable URLs, including ones that wrap across rows. */
export function createUrlLinkProvider(
  terminal: Terminal,
  hint: string,
  activate: (event: MouseEvent, url: string) => void,
): ILinkProvider {
  return {
    provideLinks(bufferLineNumber, callback) {
      const buffer = terminal.buffer.active;
      const row = bufferLineNumber - 1;
      let first = row;
      while (first > 0 && row - first < MAX_WRAPPED_ROWS && buffer.getLine(first)?.isWrapped) {
        first -= 1;
      }
      let last = row;
      while (last - first < MAX_WRAPPED_ROWS && buffer.getLine(last + 1)?.isWrapped) {
        last += 1;
      }
      let text = "";
      for (let index = first; index <= last; index += 1) {
        text += buffer.getLine(index)?.translateToString(index === last) ?? "";
      }
      const links: ILink[] = findUrls(text)
        .map(({ url, start, end }) => ({ url, range: rangeFor(start, end, first, terminal.cols) }))
        .filter(({ range }) => range.start.y <= bufferLineNumber && range.end.y >= bufferLineNumber)
        .map(({ url, range }) => ({
          range,
          text: url,
          activate: (event: MouseEvent) => activate(event, url),
          hover: () => {
            if (terminal.element) terminal.element.title = hint;
          },
          leave: () => {
            if (terminal.element) terminal.element.title = "";
          },
        }));
      callback(links.length > 0 ? links : undefined);
    },
  };
}
