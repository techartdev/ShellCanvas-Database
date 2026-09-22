// SPDX-License-Identifier: MPL-2.0
import { createHash, randomUUID } from "node:crypto";
import { readFile, rename, unlink, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseAppPackage } from "../node_modules/@techartdev/shellcanvas-app-sdk/dist/package.js";
import { parseAppRepository } from "../node_modules/@techartdev/shellcanvas-app-sdk/dist/repository.js";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const packagePath = "app/dist/app.shellcanvas.json";
const adapterPath = "dist/adapter-windows-x86_64/adapter.json";
const description =
  "Query SQL Server, PostgreSQL, MySQL/MariaDB and SQLite from ShellCanvas.";

const packageBytes = await readFile(join(root, packagePath));
const app = parseAppPackage(
  new TextDecoder("utf-8", { fatal: true }).decode(packageBytes),
);
const adapterBytes = await readFile(join(root, adapterPath));
let adapter;
try {
  adapter = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(adapterBytes));
} catch {
  throw new Error("The packaged native adapter manifest is not valid UTF-8 JSON.");
}
if (
  adapter?.id !== "dev.shellcanvas.database" ||
  adapter?.version !== "0.1.0" ||
  adapter?.platform !== "windows-x86_64"
)
  throw new Error("The packaged native adapter identity, version, or platform changed.");

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
    id: adapter.id,
    version: adapter.version,
    packages: [
      {
        platform: adapter.platform,
        path: adapterPath,
        sha256: createHash("sha256").update(adapterBytes).digest("hex"),
      },
    ],
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
