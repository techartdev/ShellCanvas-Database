// SPDX-License-Identifier: MPL-2.0
import { createHash, randomUUID } from "node:crypto";
import { readFile, readdir, rename, unlink, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseAppPackage } from "../node_modules/@techartdev/shellcanvas-app-sdk/dist/package.js";
import { parseAppRepository } from "../node_modules/@techartdev/shellcanvas-app-sdk/dist/repository.js";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const packagePath = "app/dist/app.shellcanvas.json";
const description =
  "Query SQL Server, PostgreSQL, MySQL/MariaDB and SQLite from ShellCanvas.";

const packageBytes = await readFile(join(root, packagePath));
const app = parseAppPackage(
  new TextDecoder("utf-8", { fatal: true }).decode(packageBytes),
);
const platforms = ["windows-x86_64", "linux-x86_64", "macos-x86_64", "macos-aarch64"];
const packages = [];
for (const platform of platforms) {
  const path = `dist/adapter-${platform}/adapter.json`;
  const bytes = await readFile(join(root, path));
  let adapter;
  try {
    adapter = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw new Error(`${platform} adapter manifest is not valid UTF-8 JSON.`);
  }
  if (
    adapter?.id !== "dev.shellcanvas.database" ||
    adapter?.version !== app.version ||
    adapter?.platform !== platform
  )
    throw new Error(`${platform} adapter identity, version, or platform changed.`);
  const names = await readdir(join(root, `dist/adapter-${platform}/bin`));
  if (names.length !== 1 || adapter.entrypoint !== `bin/${names[0]}`)
    throw new Error(`${platform} adapter entrypoint is missing or unexpected.`);
  const executable = await readFile(join(root, `dist/adapter-${platform}`, adapter.entrypoint));
  const entry = adapter.files.find((file) => file.path === adapter.entrypoint);
  if (entry?.size !== executable.length || entry.sha256 !== createHash("sha256").update(executable).digest("hex"))
    throw new Error(`${platform} adapter executable does not match its manifest.`);
  packages.push({ platform, path, sha256: createHash("sha256").update(bytes).digest("hex") });
}

const descriptorPath = join(root, "shellcanvas.repo.json");
const descriptor = {
  ...parseAppRepository(JSON.stringify({
    format: 1,
    kind: "app-repository",
    id: app.id,
    version: app.version,
    title: app.title,
    description,
    package: {
      path: packagePath,
      sha256: createHash("sha256").update(packageBytes).digest("hex"),
    },
  })),
  nativeAdapter: {
    id: app.id,
    version: app.version,
    packages,
  },
};
const temporary = join(root, `.repository-native-${randomUUID()}.tmp`);
try {
  await writeFile(temporary, `${JSON.stringify(descriptor, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  });
  await rename(temporary, descriptorPath);
} finally {
  await unlink(temporary).catch((error) => {
    if (error.code !== "ENOENT") throw error;
  });
}
console.log(`Repository manifest: ${descriptorPath}`);
