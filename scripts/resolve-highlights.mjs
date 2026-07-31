#!/usr/bin/env node
// Resolve (dismiss) Marrow AI highlights from outside the app, by writing the
// same per-PR dismissed-highlights file the desktop app reads. The app reloads
// that file when its window regains focus, so resolutions show up by switching
// back to Marrow.
//
// Usage:
//   node scripts/resolve-highlights.mjs <pr>            resolve ALL highlights
//   node scripts/resolve-highlights.mjs <pr> --list     show notes + status, write nothing
//   node scripts/resolve-highlights.mjs <pr> --undo     un-resolve (restore) instead
//   node scripts/resolve-highlights.mjs <pr> --severity=info,warning
//   node scripts/resolve-highlights.mjs <pr> --file=commands.rs
//   node scripts/resolve-highlights.mjs <pr> --prune    also drop stale keys (notes that no longer exist)
//   node scripts/resolve-highlights.mjs <pr> --state=fixed|intentional|noise   (default: noise)
//   node scripts/resolve-highlights.mjs <pr> --reason="why this was resolved"
//
// <pr> may be a bare number (e.g. 126 — matched against the manifests dir), an
// owner/repo/number or PR URL, or a path to a manifest .json.
// Config dir defaults to ~/.config/marrow (override with MARROW_CONFIG_DIR).

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

// ── highlightKey: byte-for-byte identical to app/src/utils.ts ────────────────
function hashString(s) {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  return (h >>> 0).toString(36);
}
function highlightKey(filePath, h) {
  return `${filePath}:${h.start_line}-${h.end_line}:${hashString(h.comment)}`;
}

const CONFIG_DIR = process.env.MARROW_CONFIG_DIR || path.join(os.homedir(), ".config", "marrow");
const MANIFESTS = path.join(CONFIG_DIR, "manifests");
const DISMISSED = path.join(CONFIG_DIR, "dismissed");

function die(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

const args = process.argv.slice(2);
const flags = new Set(args.filter((a) => a.startsWith("--") && !a.includes("=")));
const opts = Object.fromEntries(
  args.filter((a) => a.startsWith("--") && a.includes("=")).map((a) => a.slice(2).split("=")),
);
const positional = args.filter((a) => !a.startsWith("--"));
if (positional.length !== 1) die("expected exactly one <pr> argument. See header for usage.");
const target = positional[0];

const VALID_STATES = new Set(["fixed", "intentional", "noise"]);
const state = opts.state || "noise";
if (!VALID_STATES.has(state)) die(`--state must be one of fixed|intentional|noise, got "${state}"`);
const reason = opts.reason || "";

// ── Locate the manifest + derive owner/repo/number ──────────────────────────
function manifestNameToParts(file) {
  const m = path.basename(file, ".json").match(/^(.+?)_(.+?)_(\d+)$/);
  return m ? { owner: m[1], repo: m[2], number: Number(m[3]) } : null;
}

let manifestPath, parts;
if (fs.existsSync(target) && target.endsWith(".json")) {
  manifestPath = target;
  parts = manifestNameToParts(target);
} else {
  const url = target.match(/(?:github\.com\/)?([^/\s]+)\/([^/\s]+)\/(?:pull\/)?(\d+)/);
  if (url) {
    parts = { owner: url[1], repo: url[2], number: Number(url[3]) };
    manifestPath = path.join(MANIFESTS, `${parts.owner}_${parts.repo}_${parts.number}.json`);
  } else if (/^\d+$/.test(target)) {
    const matches = fs
      .readdirSync(MANIFESTS)
      .filter((f) => f.endsWith(`_${target}.json`) && !f.endsWith(".meta.json"));
    if (matches.length === 0) die(`no cached manifest for PR #${target} in ${MANIFESTS}`);
    if (matches.length > 1) die(`PR #${target} is ambiguous: ${matches.join(", ")} — pass owner/repo/${target}`);
    manifestPath = path.join(MANIFESTS, matches[0]);
    parts = manifestNameToParts(matches[0]);
  } else {
    die(`could not parse <pr>="${target}"`);
  }
}
if (!parts) die(`could not derive owner/repo/number from ${manifestPath}`);
if (!fs.existsSync(manifestPath)) die(`manifest not found: ${manifestPath}`);

// ── Gather highlights (with optional filters) ───────────────────────────────
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const sevFilter = opts.severity ? new Set(opts.severity.split(",").map((s) => s.trim().toLowerCase())) : null;
const fileFilter = opts.file || null;

const notes = [];
for (const f of manifest.files ?? []) {
  for (const h of f.highlights ?? []) {
    if (sevFilter && !sevFilter.has((h.severity || "").toLowerCase())) continue;
    if (fileFilter && !f.path.includes(fileFilter)) continue;
    notes.push({ ...h, path: f.path, severity: (h.severity || "").toUpperCase(), key: highlightKey(f.path, h) });
  }
}

const dismissedPath = path.join(DISMISSED, `${parts.owner}_${parts.repo}_${parts.number}.json`);
const existingRaw = fs.existsSync(dismissedPath) ? JSON.parse(fs.readFileSync(dismissedPath, "utf8")) : {};
const existing = new Set(existingRaw.keys ?? []);
const existingResolutions = existingRaw.resolutions ?? {};

const label = `${parts.owner}/${parts.repo}#${parts.number}`;
console.log(`${label} — ${notes.length} highlight(s)${sevFilter || fileFilter ? " (filtered)" : ""}`);
for (const n of notes) {
  const res = existingResolutions[n.key];
  let mark = "· open";
  if (existing.has(n.key)) {
    mark = res ? `✓ resolved (${res.state}${res.reason ? `: ${res.reason}` : ""})` : "✓ resolved";
  }
  console.log(`  [${n.severity.padEnd(7)}] ${n.path} L${n.start_line}-${n.end_line}  ${mark}`);
}

if (flags.has("--list")) {
  console.log(`\n(--list) wrote nothing.`);
  process.exit(0);
}

// ── Apply ───────────────────────────────────────────────────────────────────
const undo = flags.has("--undo");
const nextKeys = new Set(existing);
const nextResolutions = { ...existingResolutions };
let changed = 0;
for (const n of notes) {
  if (undo) {
    if (nextKeys.delete(n.key)) changed++;
    delete nextResolutions[n.key];
  } else if (!nextKeys.has(n.key)) {
    nextKeys.add(n.key);
    nextResolutions[n.key] = { state, reason, at: new Date().toISOString() };
    changed++;
  }
}

if (flags.has("--prune")) {
  // Drop any stored key that doesn't correspond to a current highlight in this PR.
  const live = new Set((manifest.files ?? []).flatMap((f) => (f.highlights ?? []).map((h) => highlightKey(f.path, h))));
  for (const k of [...nextKeys]) {
    if (!live.has(k)) {
      nextKeys.delete(k);
      delete nextResolutions[k];
      changed++;
    }
  }
  for (const k of Object.keys(nextResolutions)) {
    if (!nextKeys.has(k)) delete nextResolutions[k];
  }
}

fs.mkdirSync(DISMISSED, { recursive: true });
const sortedKeys = [...nextKeys].sort();
const sortedResolutions = Object.fromEntries(sortedKeys.filter((k) => nextResolutions[k]).map((k) => [k, nextResolutions[k]]));
fs.writeFileSync(dismissedPath, JSON.stringify({ keys: sortedKeys, resolutions: sortedResolutions }, null, 2));
fs.chmodSync(dismissedPath, 0o600);

console.log(
  `\n${undo ? "un-resolved" : `resolved (${state})`} ${changed} | file now has ${nextKeys.size} key(s)\n` +
    `→ ${dismissedPath}\n` +
    `→ switch back to Marrow (it reloads on window focus) to see the change.`,
);
