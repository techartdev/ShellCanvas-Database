// SPDX-License-Identifier: MPL-2.0
import { test } from "node:test";
import assert from "node:assert/strict";
import { connectionState, applyAppearance } from "./connection-state.js";
const methods = [
  "capabilities",
  "schemas",
  "tables",
  "query",
  "page",
  "closeResult",
  "exportBegin",
  "exportStatus",
  "exportCancel",
].map((name) => ({
  name: `dev.shellcanvas.database.${name}`,
  available: true,
  granted: true,
}));
test("missing service is a setup state, and the same host can recover when it appears", () => {
  const env = { connection: "connected" };
  assert.equal(connectionState(env, []).title, "Connect a database");
  assert.equal(connectionState(env, methods).ready, true);
  assert.equal(connectionState({ connection: "local" }, []).ready, false);
});
test("permissions, reconnect review, and missing operations are distinct", () => {
  assert.equal(
    connectionState(
      { connection: "connected" },
      methods.map((method) => ({ ...method, granted: false })),
    ).title,
    "Database permission needed",
  );
  assert.equal(
    connectionState({ connection: "review-required" }, methods).ready,
    false,
  );
  assert.equal(
    connectionState({ connection: "disconnected" }, methods).ready,
    false,
  );
  assert.equal(
    connectionState({ connection: "connected" }, methods.slice(1)).ready,
    false,
  );
});
test("a private app connection works without an SSH workspace", () => {
  assert.equal(
    connectionState({ connection: "local" }, methods, true).ready,
    true,
  );
  assert.equal(
    connectionState({ connection: "review-required" }, methods, true).ready,
    true,
  );
});
test("theme updates follow desktop colors and reject arbitrary CSS", () => {
  const values = new Map();
  const root = {
    dataset: {},
    style: { setProperty: (name, value) => values.set(name, value) },
  };
  applyAppearance(root, {
    mode: "light",
    colors: {
      surface: "#ffffff",
      text: "#202020",
      accent: "url(secret)",
      private: "#123456",
    },
  });
  assert.equal(root.dataset.mode, "light");
  assert.equal(values.get("--sc-surface"), "#ffffff");
  assert.equal(values.has("--sc-accent"), false);
  assert.equal(values.has("--sc-private"), false);
  applyAppearance(root, { mode: "dark", colors: { surface: "#222225" } });
  assert.equal(values.get("--sc-surface"), "#222225");
  assert.equal(values.get("color-scheme"), "dark");
});
