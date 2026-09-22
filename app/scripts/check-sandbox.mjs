// SPDX-License-Identifier: MPL-2.0
// Optional browser regression: tests the built package and SDK in the desktop's sandbox.
// Set PLAYWRIGHT_MODULE to an existing Playwright entry point when it is not installed locally.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE
    ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
    : "playwright"
);
const app = JSON.parse(
  await readFile(
    new URL("../dist/app.shellcanvas.json", import.meta.url),
    "utf8",
  ),
);
const browser = await chromium.launch({
  headless: true,
  ...(process.env.BROWSER_EXECUTABLE
    ? { executablePath: process.env.BROWSER_EXECUTABLE }
    : {}),
});
try {
  const host = await browser.newPage();
  host.setDefaultTimeout(5000);
  const errors = [];
  host.on("pageerror", (error) => errors.push(error.message));
  host.on("console", (message) => {
    if (message.text().includes("Blocked form submission"))
      errors.push(message.text());
  });
  await host.setContent(
    '<iframe sandbox="allow-scripts" style="width:1000px;height:850px"></iframe>',
  );
  await host.evaluate((app) => {
    window.connectionAttempts = [];
    const frame = document.querySelector("iframe");
    window.addEventListener("message", (event) => {
      if (
        event.source !== frame.contentWindow ||
        event.data?.type !== "shellcanvas:ready:v1" ||
        event.data.token !== "sandbox-test"
      )
        return;
      const { port1, port2 } = new MessageChannel();
      port1.onmessage = ({ data }) => {
        const request = JSON.parse(data);
        if (
          request.type !== "request" ||
          request.method === "system.events.next"
        )
          return;
        let value;
        if (request.method === "system.environment.get")
          value = {
            apiVersion: 1,
            connection: "connected",
            binding: "fixture-ssh",
            host: { name: "Fixture SSH", system: "Linux" },
            visible: true,
            capabilities: [],
          };
        else if (request.method === "system.services.list")
          value = [
            {
              name: "system.companion.connect",
              version: 1,
              granted: true,
              available: true,
              permissions: [],
            },
          ];
        else if (request.method === "system.companion.routes")
          value = { ssh: true };
        else if (request.method === "system.companion.status")
          value = { connected: false, binding: null };
        else if (request.method === "system.companion.connect") {
          window.connectionAttempts.push(request.params);
          port1.postMessage(
            JSON.stringify({
              v: 1,
              type: "error",
              id: request.id,
              code: "failed",
              message: "Fixture connection rejected",
            }),
          );
          return;
        } else throw new Error("Unexpected test method: " + request.method);
        port1.postMessage(
          JSON.stringify({ v: 1, type: "result", id: request.id, value }),
        );
      };
      frame.contentWindow.postMessage(
        { type: "shellcanvas:connect:v1", token: "sandbox-test" },
        "*",
        [port2],
      );
    });
    frame.srcdoc = `<!doctype html><html><head><meta name="shellcanvas-instance" content="sandbox-test"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'nonce-sandbox-test'; style-src 'unsafe-inline'; form-action 'none'"><style>${app.style.replaceAll("</style", "<\\/style")}</style></head><body><div id="root"></div><script nonce="sandbox-test">${app.script.replaceAll("</script", "<\\/script")}</script></body></html>`;
  }, app);
  const page = host.frameLocator("iframe");
  await page.locator("#setup-connect").click();
  await page.locator("#connect-submit").click();
  assert.equal(
    await host.evaluate(() => window.connectionAttempts.length),
    0,
    "Empty required fields must not dispatch",
  );
  await page.locator("#db-host").fill("127.0.0.1");
  await page.locator("#db-name").fill("fixture");
  await page.locator("#db-user").fill("fixture-user");
  await page.locator("#db-password").fill("fixture-only");
  await page.locator("#connect-submit").click();
  await host.waitForFunction(() => window.connectionAttempts.length === 1);
  await page.locator("#connect-error").waitFor({ state: "visible" });
  assert.equal(
    await page.locator("#connect-error").innerText(),
    "Fixture connection rejected",
  );
  const first = await host.evaluate(() => window.connectionAttempts[0]);
  assert.equal(first.configuration.provider, "sqlserver");
  assert.equal(first.configuration.port, 1433);
  assert.equal(first.route, "ssh");
  assert.match(
    await page.locator("#route-hint").innerText(),
    /Fixture SSH.*remote host/,
  );
  await page.locator("#db-password").press("Enter");
  await host.waitForFunction(() => window.connectionAttempts.length === 2);
  await page.locator("#db-route").selectOption("direct");
  await page.locator("#connect-submit").click();
  await host.waitForFunction(() => window.connectionAttempts.length === 3);
  assert.equal(
    await host.evaluate(() => window.connectionAttempts[2].route),
    "direct",
  );
  await page.locator("#db-provider").selectOption("sqlite");
  await page.locator("#db-path").fill("C:/fixture.db");
  await page.locator("#connect-submit").click();
  await host.waitForFunction(() => window.connectionAttempts.length === 4);
  const last = await host.evaluate(() => window.connectionAttempts[3]);
  assert.equal(last.route, "direct");
  assert.deepEqual(last.configuration, {
    provider: "sqlite",
    sqlitePath: "C:/fixture.db",
  });
  assert.equal(
    await host.locator("iframe").getAttribute("sandbox"),
    "allow-scripts",
  );
  assert.deepEqual(errors, []);
  console.log(
    "Packaged sandbox regression passed: required fields, SQL Server click, Enter retry, SQLite click, visible connection errors; no form submission permission.",
  );
} finally {
  await browser.close();
}
