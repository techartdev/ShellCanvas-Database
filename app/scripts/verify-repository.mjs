// SPDX-License-Identifier: MPL-2.0
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { dirname, posix, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const readTree = (path) =>
  execFileSync("git", ["-C", root, "show", `HEAD:${path}`], {
    encoding: null,
    maxBuffer: 32 * 1024 * 1024,
  });
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const parse = (bytes, label) => {
  try {
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw new Error(`${label} is not valid UTF-8 JSON.`);
  }
};
const expectHash = (bytes, expected, label) => {
  const actual = sha256(bytes);
  if (actual !== expected)
    throw new Error(`${label} SHA-256 mismatch: expected ${expected}, got ${actual}.`);
};

const descriptor = parse(readTree("shellcanvas.repo.json"), "Repository descriptor");
const appBytes = readTree(descriptor.package.path);
expectHash(appBytes, descriptor.package.sha256, "App package");
const app = parse(appBytes, "App package");
if (app.id !== descriptor.id || app.version !== descriptor.version)
  throw new Error("App package identity/version does not match the descriptor.");

const dependency = descriptor.nativeAdapter;
const expected = ["windows-x86_64", "linux-x86_64", "darwin-x86_64", "darwin-aarch64"];
if (!dependency || dependency.packages.length !== expected.length)
  throw new Error("Expected four pinned native adapter packages.");
if (dependency.packages.some((entry, index) => entry.platform !== expected[index]))
  throw new Error("Native adapter platform set or order changed.");
for (const entry of dependency.packages) {
  const manifestBytes = readTree(entry.path);
  expectHash(manifestBytes, entry.sha256, `${entry.platform} adapter manifest`);
  const manifest = parse(manifestBytes, `${entry.platform} adapter manifest`);
  if (
    manifest.id !== dependency.id ||
    manifest.version !== dependency.version ||
    manifest.platform !== entry.platform
  )
    throw new Error(`${entry.platform} adapter identity/version/platform mismatch.`);
  const base = posix.dirname(entry.path);
  for (const file of manifest.files) {
    const bytes = readTree(posix.join(base, file.path));
    if (bytes.length !== file.size)
      throw new Error(`${file.path} size mismatch: expected ${file.size}, got ${bytes.length}.`);
    expectHash(bytes, file.sha256, file.path);
  }
}
console.log(`Verified committed app, ${dependency.packages.length} adapter manifests, and all native assets.`);
