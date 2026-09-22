// SPDX-License-Identifier: MPL-2.0
import {
  connectToShellCanvas,
  RpcError,
  type Json,
  type AppEnvironment,
  type ServiceMethodInfo,
} from "@techartdev/shellcanvas-app-sdk";
import { nextOffset } from "./paging.js";
import { connectionState, applyAppearance } from "./connection-state.js";

type Cell = { kind: string; display: string };
type Column = { name: string; databaseType: string };
type Page = {
  resultId: string;
  columns: Column[];
  rows: Cell[][];
  offset: number;
  totalRows: number;
  hasMore: boolean;
  truncated: boolean;
  affectedRows: number;
  warnings: string[];
};
type Table = { name: string; kind: string };
const service = "dev.shellcanvas.database";
const databaseIcon =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0"/></svg>';
const root = document.querySelector<HTMLDivElement>("#root")!;
root.innerHTML = `
<main>
  <header class="app-header"><div class="app-heading"><span class="app-icon">${databaseIcon}</span><div><h1>Database</h1><p id="destination">No database connected</p></div></div><div class="header-actions"><span id="connection-state" class="connection-state">Not connected</span><button id="connection-settings">Connect database</button><button id="disconnect" hidden>Disconnect</button></div></header>
  <section id="setup" class="setup"><div class="setup-card"><span class="setup-icon">${databaseIcon}</span><h2 id="setup-title">Connect a database</h2><p id="setup-detail">Choose a database server or an existing SQLite file to browse tables and run queries.</p><div class="provider-list"><span>SQL Server</span><span>PostgreSQL</span><span>MySQL / MariaDB</span><span>SQLite</span></div><button id="setup-connect" class="primary">Connect database</button><button id="retry">Check connection again</button><p class="setup-note">Connections run from this PC. Your SSH host is not automatically a database connection.</p></div></section>
  <section id="workspace" class="workspace" hidden><aside><div class="side-head"><h2>Schema</h2><button id="reload" class="icon-button" title="Refresh schemas" aria-label="Refresh schemas">↻</button></div><div id="tree"></div></aside><div class="work"><div class="editor-heading"><label for="sql">SQL query</label><span>Ctrl+Enter to run</span></div><textarea id="sql" spellcheck="false" placeholder="SELECT * FROM …;"></textarea><div class="toolbar"><button id="run" class="primary" disabled>Run query</button><button id="cancel" disabled>Cancel</button><span class="query-note">SQL uses your database account’s permissions.</span></div><p id="warnings" role="status" hidden></p><div class="result-head"><h2 id="summary">Results</h2><div><label class="safe"><input id="safe" type="checkbox" checked> Spreadsheet-safe CSV</label><button id="export" disabled>Save CSV…</button></div></div><div class="grid-wrap"><table><thead></thead><tbody></tbody></table><div id="empty">Run a query to see results.</div></div><div class="pager"><span id="page"></span><button id="prev" disabled>Previous</button><button id="next" disabled>Next</button></div></div></section>
  <footer><span id="status" role="status">Checking the app connection…</span><span>Database workspace</span></footer>
</main>
<dialog id="connect-dialog" aria-labelledby="connect-title"><form id="connect-form"><div class="dialog-heading"><span class="app-icon">${databaseIcon}</span><button type="button" id="close-dialog" class="icon-button" aria-label="Close connection settings">×</button></div><h2 id="connect-title">Connect a database</h2><p>Choose an endpoint reachable from this PC. This does not scan your SSH host or network for database instances.</p><fieldset id="connection-fields"><label>Database provider<select id="db-provider"><option value="sqlserver">Microsoft SQL Server</option><option value="postgresql">PostgreSQL</option><option value="mysql">MySQL</option><option value="mariadb">MariaDB</option><option value="sqlite">SQLite</option></select></label><div id="network-fields"><div class="field-row"><label class="host-field">Host<input id="db-host" placeholder="db.example.com" autocomplete="off"></label><label class="port-field">Port<input id="db-port" type="number" min="1" max="65535" value="1433"></label></div><label>Database<input id="db-name" placeholder="Database name" autocomplete="off"></label><div class="field-row"><label>Username<input id="db-user" autocomplete="off"></label><label>Password<input id="db-password" type="password" autocomplete="off"></label></div><label class="checkbox"><input id="db-tls" type="checkbox" checked> Use TLS</label><label class="checkbox"><input id="db-trust" type="checkbox"> Trust a self-signed server certificate</label><p class="field-hint">For SQL Server named instances, enter the server host and its TCP port.</p></div><div id="sqlite-fields" hidden><label>SQLite file on this PC<input id="db-path" placeholder="C:/data/example.db" autocomplete="off"></label><p class="field-hint">Choose an existing file. This path refers to the PC running ShellCanvas, not your SSH host.</p></div></fieldset><p id="connect-error" class="error" role="alert" hidden></p><div class="dialog-actions"><span>Passwords are not saved.</span><button id="connect-cancel" type="button">Cancel</button><button id="connect-submit" class="primary" type="submit">Connect</button></div></form></dialog>`;
const $ = <T extends HTMLElement = HTMLElement>(id: string) =>
  document.getElementById(id)! as T;
const input = (id: string) => $<HTMLInputElement>(id);
const button = (id: string) => $<HTMLButtonElement>(id);
const sql = $<HTMLTextAreaElement>("sql");
const dialog = $<HTMLDialogElement>("connect-dialog");
const status = $("status");
let client: Awaited<ReturnType<typeof connectToShellCanvas>>;
let current: Page | null = null;
let queryController: AbortController | null = null;
let connectController: AbortController | null = null;
let provider = "",
  generation = 0,
  schemaOp = 0,
  pageOp = 0,
  bindOp = 0;
let exportJob: string | null = null,
  pageBusy = false,
  ready = false;
let exportBusy = false;
let previousOffsets: number[] = [],
  signature: string | null = null;
let privateConnection = false,
  companionAvailable = false,
  destination = "";
let closed = false;
const pageSize = 100;
function message(error: unknown) {
  return error instanceof RpcError
    ? error.message
    : error instanceof Error
      ? error.message
      : String(error);
}
function report(error: unknown) {
  status.textContent = message(error);
}
function setControls() {
  button("run").disabled =
    !ready || !!queryController || !!connectController || exportBusy;
  button("cancel").disabled = !queryController;
  button("reload").disabled = !ready || !!connectController;
  button("export").disabled =
    !ready || !current?.columns.length || exportBusy || !!queryController;
  button("prev").disabled = !ready || pageBusy || !previousOffsets.length;
  button("next").disabled = !ready || pageBusy || !current?.hasMore;
  button("disconnect").hidden = !privateConnection || !ready;
  button("disconnect").disabled = !!connectController;
  button("connection-settings").disabled =
    !companionAvailable ||
    !!connectController ||
    !!queryController ||
    exportBusy;
  button("setup-connect").disabled = !companionAvailable || !!connectController;
}
async function call<T>(
  method: string,
  params: Json = {},
  signal?: AbortSignal,
): Promise<T> {
  return (privateConnection
    ? await client.call(
        "system.companion.call",
        { method: `${service}.${method}`, params },
        signal,
      )
    : await client.services.call(
        `${service}.${method}`,
        params,
        signal,
      )) as unknown as T;
}
function unavailable(title: string, detail: string) {
  ready = false;
  $("connection-state").textContent = "Not connected";
  $("connection-state").classList.remove("connected");
  $("destination").textContent = "No database connected";
  $("setup").hidden = false;
  $("workspace").hidden = true;
  $("setup-title").textContent = title;
  $("setup-detail").textContent = detail;
  button("connection-settings").textContent = "Connect database";
  status.textContent = detail;
  setControls();
}
function clearResult(note = "Run a query to see results.", release = true) {
  const old = current;
  current = null;
  if (release && old)
    void call("closeResult", {
      resultId: old.resultId,
      offset: 0,
      pageSize: 1,
    }).catch(() => {});
  document.querySelector("thead")!.replaceChildren();
  document.querySelector("tbody")!.replaceChildren();
  $("summary").textContent = "Results";
  $("page").textContent = "";
  $("warnings").hidden = true;
  $("empty").hidden = false;
  $("empty").textContent = note;
  previousOffsets = [];
  setControls();
}
function render(page: Page) {
  current = page;
  const head = document.querySelector("thead")!,
    body = document.querySelector("tbody")!;
  head.replaceChildren();
  body.replaceChildren();
  if (page.columns.length) {
    const tr = document.createElement("tr");
    for (const column of page.columns) {
      const th = document.createElement("th");
      th.textContent = column.name;
      th.title = column.databaseType;
      tr.append(th);
    }
    head.append(tr);
  }
  for (const row of page.rows) {
    const tr = document.createElement("tr");
    for (const cell of row) {
      const td = document.createElement("td");
      td.textContent = cell.kind === "null" ? "NULL" : cell.display;
      td.className = `cell-${cell.kind}`;
      tr.append(td);
    }
    body.append(tr);
  }
  $("empty").hidden = page.rows.length > 0;
  $("empty").textContent = page.columns.length
    ? "The query returned no rows."
    : `Statement completed. ${page.affectedRows.toLocaleString()} rows affected.`;
  $("summary").textContent = page.columns.length
    ? `${page.totalRows.toLocaleString()} rows${page.truncated ? " · limit reached" : ""}`
    : `${page.affectedRows.toLocaleString()} rows affected`;
  $("page").textContent = page.totalRows
    ? `${page.offset + 1}–${page.offset + page.rows.length} of ${page.totalRows.toLocaleString()}`
    : "";
  $("warnings").textContent = page.warnings.join(" · ");
  $("warnings").hidden = !page.warnings.length;
  setControls();
}
function quote(name: string) {
  return provider === "mysql"
    ? `\`${name.replaceAll("`", "``")}\``
    : provider === "sqlserver"
      ? `[${name.replaceAll("]", "]]")}]`
      : `"${name.replaceAll('"', '""')}"`;
}
function tableSql(schema: string, table: string) {
  const full =
    provider === "sqlite" ? quote(table) : `${quote(schema)}.${quote(table)}`;
  return provider === "sqlserver"
    ? `SELECT TOP (200) * FROM ${full};`
    : `SELECT * FROM ${full} LIMIT 200;`;
}
async function loadSchemas() {
  const op = ++schemaOp,
    gen = generation,
    tree = $("tree");
  tree.textContent = "Loading schemas…";
  try {
    const value = await call<{ schemas: string[] }>("schemas");
    if (op !== schemaOp || gen !== generation) return;
    tree.replaceChildren();
    for (const schema of value.schemas) {
      const details = document.createElement("details"),
        summary = document.createElement("summary");
      summary.textContent = schema;
      details.append(summary);
      details.addEventListener("toggle", () => {
        if (details.open && details.childElementCount === 1)
          void loadTables(details, schema, gen);
      });
      tree.append(details);
      if (value.schemas.length === 1) details.open = true;
    }
    if (!value.schemas.length)
      tree.textContent = "No schemas visible to this account.";
  } catch (error) {
    if (op === schemaOp && gen === generation) {
      tree.textContent = "Could not load schemas. Use Refresh to retry.";
      report(error);
    }
  }
}
async function loadTables(
  details: HTMLDetailsElement,
  schema: string,
  gen: number,
) {
  const wait = document.createElement("p");
  wait.textContent = "Loading tables…";
  details.append(wait);
  try {
    const value = await call<{ tables: Table[] }>("tables", { schema });
    if (gen !== generation || !details.isConnected) return;
    wait.remove();
    for (const table of value.tables) {
      const item = document.createElement("button");
      item.className = "table";
      item.textContent = table.name;
      item.title = `${table.kind} · insert a query`;
      item.onclick = () => {
        sql.value = tableSql(schema, table.name);
        sql.focus();
      };
      details.append(item);
    }
    if (!value.tables.length) {
      wait.textContent = "No tables";
      details.append(wait);
    }
  } catch (error) {
    if (gen === generation && details.isConnected) {
      wait.textContent = "Could not load tables. Refresh schemas to retry.";
      report(error);
    }
  }
}
async function execute() {
  if (
    !ready ||
    queryController ||
    !sql.value.trim() ||
    connectController ||
    exportBusy
  )
    return;
  const gen = generation,
    active = new AbortController();
  queryController = active;
  clearResult("Running query…");
  setControls();
  status.textContent = "Running query…";
  try {
    const page = await call<Page>(
      "query",
      { sql: sql.value, pageSize },
      active.signal,
    );
    if (gen !== generation) return;
    render(page);
    status.textContent = "Query completed.";
  } catch (error) {
    if (gen === generation) {
      clearResult(
        active.signal.aborted
          ? "Query canceled. Check the database before retrying statements that change data."
          : "The query failed.",
      );
      report(error);
    }
  } finally {
    if (queryController === active) queryController = null;
    setControls();
  }
}
async function move(offset: number, previous = false) {
  if (!current || pageBusy || !ready) return;
  const op = ++pageOp,
    gen = generation,
    original = current;
  pageBusy = true;
  setControls();
  try {
    const page = await call<Page>("page", {
      resultId: original.resultId,
      offset,
      pageSize,
    });
    if (op === pageOp && gen === generation && current === original) {
      if (previous) previousOffsets.pop();
      else previousOffsets.push(original.offset);
      render(page);
    }
  } catch (error) {
    if (op === pageOp && gen === generation) report(error);
  } finally {
    if (op === pageOp) pageBusy = false;
    setControls();
  }
}
async function cancelExport() {
  const job = exportJob;
  exportJob = null;
  if (job) await call("exportCancel", { jobId: job }).catch(() => {});
}
async function exportCsv() {
  if (!current || exportBusy) return;
  const gen = generation;
  exportBusy = true;
  setControls();
  status.textContent = "Choose a CSV destination…";
  try {
    const start = await call<{ jobId: string }>("exportBegin", {
      resultId: current.resultId,
      suggestedName: "query-results.csv",
      spreadsheetSafe: input("safe").checked,
    });
    if (gen !== generation) {
      await call("exportCancel", { jobId: start.jobId }).catch(() => {});
      return;
    }
    exportJob = start.jobId;
    setControls();
    for (let polls = 0; polls < 180 && exportJob === start.jobId; polls++) {
      await new Promise((resolve) => setTimeout(resolve, 350));
      if (gen !== generation) return;
      const job = await call<{
        state: string;
        name?: string;
        message?: string;
      }>("exportStatus", { jobId: start.jobId });
      if (gen !== generation) return;
      if (job.state === "saved" || job.state === "canceled") {
        status.textContent =
          job.state === "saved"
            ? `Saved ${job.name ?? "CSV"}.`
            : "CSV save canceled.";
        exportJob = null;
        return;
      }
      if (job.state === "failed")
        throw new Error(job.message ?? "CSV export failed");
      if (job.state !== "choosing" && job.state !== "writing")
        throw new Error("Unknown CSV export state");
    }
    throw new Error("CSV save timed out");
  } catch (error) {
    if (gen === generation) {
      await cancelExport();
      report(error);
    }
  } finally {
    exportBusy = false;
    setControls();
  }
}
async function refresh(force = false) {
  if (closed || !client) return;
  const op = ++bindOp;
  try {
    const env = await client.environment.get();
    if (op !== bindOp || closed) return;
    applyAppearance(document.documentElement, env.appearance);
    const systemMethods = await client.services.list();
    if (op !== bindOp || closed) return;
    companionAvailable = systemMethods.some(
      (method) =>
        method.name === "system.companion.connect" &&
        method.available &&
        method.granted,
    );
    const own = companionAvailable
      ? ((await client.call("system.companion.status")) as {
          connected: boolean;
          binding: string | null;
        })
      : { connected: false, binding: null };
    if (op !== bindOp || closed || connectController) return;
    const methods = own.connected
      ? ((await client.call(
          "system.companion.list",
        )) as unknown as ServiceMethodInfo[])
      : systemMethods;
    if (op !== bindOp || closed) return;
    const next = JSON.stringify([
      own.connected,
      own.binding,
      own.connected ? null : env.binding,
      own.connected ? null : env.connection,
      methods
        .filter((m) => m.name.startsWith(service + "."))
        .map((m) => [m.name, m.available, m.granted, m.source]),
    ]);
    if (!force && signature === next) return;
    generation++;
    schemaOp++;
    pageOp++;
    pageBusy = false;
    ready = false;
    queryController?.abort();
    queryController = null;
    await cancelExport();
    if (op !== bindOp || closed) return;
    privateConnection = own.connected;
    clearResult("Run a query to see results.", false);
    const state = connectionState(env, methods, own.connected);
    if (!state.ready) {
      signature = next;
      unavailable(state.title, state.detail);
      return;
    }
    $("connection-state").textContent = "Checking database…";
    const info = await call<{ provider: string }>("capabilities");
    if (op !== bindOp || closed) return;
    signature = next;
    provider = info.provider;
    ready = true;
    $("setup").hidden = true;
    $("workspace").hidden = false;
    $("connection-state").textContent = "Connected";
    $("connection-state").classList.add("connected");
    $("destination").textContent = privateConnection
      ? destination || `${provider} · app connection`
      : `${provider} · ${env.host?.name ?? "workspace"}`;
    button("connection-settings").textContent = "Connection…";
    status.textContent = "Ready. Choose a table or write a query.";
    setControls();
    await loadSchemas();
  } catch (error) {
    if (op === bindOp && !closed) {
      signature = null;
      unavailable("Connection unavailable", message(error));
    }
  }
}
function selectProvider() {
  const sqlite = input("db-provider").value === "sqlite";
  $("network-fields").hidden = sqlite;
  $("sqlite-fields").hidden = !sqlite;
  for (const id of [
    "db-host",
    "db-port",
    "db-name",
    "db-user",
    "db-password",
    "db-tls",
    "db-trust",
  ])
    input(id).disabled = sqlite;
  input("db-path").disabled = !sqlite;
  input("db-trust").disabled = sqlite || !input("db-tls").checked;
  for (const id of ["db-host", "db-port", "db-name", "db-user"])
    input(id).required = !sqlite;
  input("db-path").required = sqlite;
  input("db-port").value = String(
    (
      {
        sqlserver: 1433,
        postgresql: 5432,
        mysql: 3306,
        mariadb: 3306,
      } as Record<string, number>
    )[input("db-provider").value] ?? 0,
  );
}
function openConnection() {
  if (!companionAvailable) return;
  $("connect-error").hidden = true;
  dialog.showModal();
  $<HTMLSelectElement>("db-provider").focus();
}
function closeConnection() {
  connectController?.abort();
  input("db-password").value = "";
  dialog.close();
}
async function connect(event: SubmitEvent) {
  event.preventDefault();
  if (connectController) return;
  const active = new AbortController();
  connectController = active;
  setControls();
  $<HTMLFieldSetElement>("connection-fields").disabled = true;
  button("connect-submit").disabled = true;
  button("connect-submit").textContent = "Connecting…";
  $("connect-error").hidden = true;
  const selected = input("db-provider").value;
  const configuration: Record<string, string | number | boolean> =
    selected === "sqlite"
      ? { provider: selected, sqlitePath: input("db-path").value.trim() }
      : {
          provider: selected,
          host: input("db-host").value.trim(),
          port: Number(input("db-port").value),
          database: input("db-name").value.trim(),
          username: input("db-user").value.trim(),
          password: input("db-password").value,
          tls: input("db-tls").checked,
          trustServerCertificate:
            input("db-tls").checked && input("db-trust").checked,
        };
  const target =
    selected === "sqlite"
      ? input("db-path").value.trim()
      : `${configuration.host}:${configuration.port} / ${configuration.database}`;
  status.textContent = `Connecting to ${target}…`;
  try {
    await client.call(
      "system.companion.connect",
      { configuration },
      active.signal,
    );
    destination = `${$<HTMLSelectElement>("db-provider").selectedOptions[0].text} · ${target}`;
    input("db-password").value = "";
    dialog.close();
  } catch (error) {
    if (!active.signal.aborted) {
      $("connect-error").textContent = message(error);
      $("connect-error").hidden = false;
    }
    status.textContent = active.signal.aborted
      ? "Connection canceled."
      : `Could not connect to ${target}.`;
  } finally {
    connectController = null;
    $<HTMLFieldSetElement>("connection-fields").disabled = false;
    button("connect-submit").disabled = false;
    button("connect-submit").textContent = "Connect";
    await refresh(true);
  }
}
async function start() {
  client = await connectToShellCanvas();
  // Subscribe before the initial check: missing services must not prevent recovery.
  client.events.subscribe((batch) => {
    for (const event of batch.events)
      if (event.topic === "system.environment")
        applyAppearance(
          document.documentElement,
          (event.value as unknown as AppEnvironment)?.appearance,
        );
    if (
      batch.events.some((event) =>
        ["system.environment", "system.services", "system.companion"].includes(
          event.topic,
        ),
      )
    )
      void refresh();
  }, report);
  window.addEventListener(
    "pagehide",
    () => {
      closed = true;
      generation++;
      bindOp++;
      queryController?.abort();
      connectController?.abort();
      client.dispose();
    },
    { once: true },
  );
  await refresh();
}
button("run").onclick = () => void execute();
button("cancel").onclick = () => queryController?.abort();
button("reload").onclick = () => void loadSchemas();
button("export").onclick = () => void exportCsv();
button("prev").onclick = () => {
  const offset = previousOffsets.at(-1);
  if (offset !== undefined) void move(offset, true);
};
button("next").onclick = () => {
  if (current) void move(nextOffset(current));
};
button("retry").onclick = () => void refresh(true);
button("connection-settings").onclick = openConnection;
button("setup-connect").onclick = openConnection;
button("close-dialog").onclick = closeConnection;
button("connect-cancel").onclick = closeConnection;
dialog.addEventListener("cancel", (event) => {
  event.preventDefault();
  closeConnection();
});
$("connect-form").addEventListener(
  "submit",
  (event) => void connect(event as SubmitEvent),
);
$("db-provider").addEventListener("change", selectProvider);
input("db-tls").addEventListener("change", () => {
  input("db-trust").disabled = !input("db-tls").checked;
});
button("disconnect").onclick = () => {
  void (async () => {
    queryController?.abort();
    await cancelExport();
    await client.call("system.companion.disconnect");
    destination = "";
    await refresh(true);
  })().catch(report);
};
sql.addEventListener("keydown", (event) => {
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    event.preventDefault();
    void execute();
  }
});
selectProvider();
setControls();
void start().catch((error) =>
  unavailable("ShellCanvas connection unavailable", message(error)),
);
