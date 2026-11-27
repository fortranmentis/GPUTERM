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

/** Display cell width of a single Unicode code point in a terminal grid. */
export function charCellWidth(cp: number): number {
  // Control chars, zero-width joiner/non-joiner, variation selectors, combining marks.
  if (
    cp === 0 ||
    cp < 0x20 ||
    (cp >= 0x7f && cp < 0xa0) ||
    cp === 0x200b ||
    cp === 0x200c ||
    cp === 0x200d ||
    cp === 0xfeff ||
    (cp >= 0x0300 && cp <= 0x036f) ||
    (cp >= 0x1ab0 && cp <= 0x1aff) ||
    (cp >= 0x1dc0 && cp <= 0x1dff) ||
    (cp >= 0x20d0 && cp <= 0x20ff) ||
    (cp >= 0xfe00 && cp <= 0xfe0f) ||
    (cp >= 0xfe20 && cp <= 0xfe2f) ||
    (cp >= 0xe0100 && cp <= 0xe01ef)
  ) {
    return 0;
  }

  // East Asian Wide / Fullwidth (Hangul, CJK, fullwidth forms) and common emoji.
  if (
    (cp >= 0x1100 && cp <= 0x115f) ||
    cp === 0x231a ||
    cp === 0x231b ||
    cp === 0x2329 ||
    cp === 0x232a ||
    (cp >= 0x23e9 && cp <= 0x23f3) ||
    (cp >= 0x25fd && cp <= 0x25fe) ||
    (cp >= 0x2614 && cp <= 0x2615) ||
    (cp >= 0x2648 && cp <= 0x2653) ||
    cp === 0x267f ||
    cp === 0x2693 ||
    cp === 0x26a1 ||
    (cp >= 0x26aa && cp <= 0x26ab) ||
    (cp >= 0x26bd && cp <= 0x26be) ||
    (cp >= 0x26c4 && cp <= 0x26c5) ||
    cp === 0x26ce ||
    cp === 0x26d4 ||
    cp === 0x26ea ||
    (cp >= 0x26f2 && cp <= 0x26f3) ||
    cp === 0x26f5 ||
    cp === 0x26fa ||
    cp === 0x26fd ||
    cp === 0x2705 ||
    (cp >= 0x270a && cp <= 0x270b) ||
    cp === 0x2728 ||
    cp === 0x274c ||
    cp === 0x274e ||
    (cp >= 0x2753 && cp <= 0x2755) ||
    cp === 0x2757 ||
    (cp >= 0x2795 && cp <= 0x2797) ||
    cp === 0x27b0 ||
    cp === 0x27bf ||
    (cp >= 0x2b1b && cp <= 0x2b1c) ||
    cp === 0x2b50 ||
    cp === 0x2b55 ||
    (cp >= 0x2e80 && cp <= 0x303e) ||
    (cp >= 0x3040 && cp <= 0xa4cf) ||
    (cp >= 0xa960 && cp <= 0xa97c) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe10 && cp <= 0xfe19) ||
    (cp >= 0xfe30 && cp <= 0xfe6f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1faff) ||
    (cp >= 0x20000 && cp <= 0x2fffd) ||
    (cp >= 0x30000 && cp <= 0x3fffd)
  ) {
    return 2;
  }

  return 1;
}

/**
 * Converts a UTF-16 string offset in `text` to a 0-based terminal cell offset,
 * accounting for wide (East Asian / Hangul / emoji) and zero-width characters,
 * plus row-end padding when a wide character wraps at the last column.
 */
export function cellOffsetFor(text: string, utf16Index: number, cols?: number): number {
  let cell = 0;
  let index = 0;
  const limit = Math.min(utf16Index, text.length);
  while (index < limit) {
    const cp = text.codePointAt(index)!;
    const step = cp > 0xffff ? 2 : 1;
    const width = charCellWidth(cp);
    if (cols && cols > 0 && width === 2 && cell % cols === cols - 1) {
      cell += 1;
    }
    cell += width;
    index += step;
  }
  return cell;
}

/**
 * Buffer range (1-based, inclusive) of cell span `[start, end)` in a logical
 * line that begins at 0-based row `firstRow` and wraps every `cols` cells.
 */
export function rangeFor(start: number, end: number, firstRow: number, cols: number): IBufferRange {
  const last = end - 1;
  return {
    start: { x: (start % cols) + 1, y: firstRow + Math.floor(start / cols) + 1 },
    end: { x: (last % cols) + 1, y: firstRow + Math.floor(last / cols) + 1 },
  };
}

/** Buffer range (1-based, inclusive) for UTF-16 match `[start, end)` in `text`. */
export function rangeForText(
  text: string,
  start: number,
  end: number,
  firstRow: number,
  cols: number,
): IBufferRange {
  const startCell = cellOffsetFor(text, start, cols);
  const endCell = cellOffsetFor(text, end, cols);
  return rangeFor(startCell, endCell, firstRow, cols);
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
        .map(({ url, start, end }) => ({
          url,
          range: rangeForText(text, start, end, first, terminal.cols),
        }))
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
