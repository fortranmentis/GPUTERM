#!/usr/bin/env node
// Builds the updater manifest (latest.json) for a release from its signed
// update bundles. Usage:
//   node scripts/make-updater-manifest.mjs <tag> <assets-dir> <notes-file> <repo> > latest.json
// <assets-dir> holds the release's `.sig` files; each one names the bundle
// it signs (e.g. GpuTerm_1.3.8-beta_x64-setup.exe.sig).

/* global process, console */

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/** Which update bundle each updater platform installs. */
const PLATFORMS = [
  { key: "darwin-aarch64", matches: (name) => name.endsWith(".app.tar.gz") },
  { key: "windows-x86_64", matches: (name) => name.endsWith("-setup.exe") },
  { key: "linux-x86_64", matches: (name) => name.endsWith(".AppImage") },
];

/**
 * @param {{ tag: string, repo: string, notes: string, pubDate: string,
 *           signatures: Record<string, string> }} input
 *   `signatures` maps a bundle's asset name to the contents of its `.sig`.
 */
export function buildManifest({ tag, repo, notes, pubDate, signatures }) {
  const platforms = {};
  for (const platform of PLATFORMS) {
    const found = Object.keys(signatures).filter(platform.matches);
    if (found.length !== 1) {
      throw new Error(
        `expected one ${platform.key} update bundle, found ${found.length}: ${found.join(", ") || "none"}`,
      );
    }
    const asset = found[0];
    platforms[platform.key] = {
      signature: signatures[asset].trim(),
      url: `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(asset)}`,
    };
  }
  return {
    version: tag.replace(/^v/, ""),
    notes: notes.trim(),
    pub_date: pubDate,
    platforms,
  };
}

/** Reads every `<bundle>.sig` in `dir` into a bundle-name → signature map. */
export function readSignatures(dir) {
  const signatures = {};
  for (const file of readdirSync(dir)) {
    if (file.endsWith(".sig")) {
      signatures[file.slice(0, -".sig".length)] = readFileSync(join(dir, file), "utf8");
    }
  }
  return signatures;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [tag, dir, notesFile, repo] = process.argv.slice(2);
  if (!tag || !dir || !notesFile || !repo) {
    console.error("usage: make-updater-manifest.mjs <tag> <assets-dir> <notes-file> <repo>");
    process.exit(2);
  }
  const manifest = buildManifest({
    tag,
    repo,
    notes: readFileSync(notesFile, "utf8"),
    pubDate: new Date().toISOString(),
    signatures: readSignatures(dir),
  });
  process.stdout.write(`${JSON.stringify(manifest, null, 2)}\n`);
}
