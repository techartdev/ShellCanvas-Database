// SPDX-License-Identifier: MPL-2.0
const prefix = "dev.shellcanvas.database.";
const required = [
  "capabilities",
  "schemas",
  "tables",
  "query",
  "page",
  "closeResult",
  "exportBegin",
  "exportStatus",
  "exportCancel",
];

/** @param {{connection:string}} environment
 * @param {readonly {name:string,available:boolean,granted:boolean}[]} methods
 * @param {boolean} ownConnection */
export function connectionState(environment, methods, ownConnection = false) {
  const database = methods.filter((method) => method.name.startsWith(prefix));
  if (database.some((method) => !method.granted))
    return {
      ready: false,
      title: "Database permission needed",
      detail:
        "Review this app in App Manager and allow its Database service permission.",
    };
  if (!ownConnection && environment.connection === "review-required")
    return {
      ready: false,
      title: "Review the reconnected host",
      detail:
        "Use the reconnected host in the bar above this app, or connect a database directly.",
    };
  if (!ownConnection && environment.connection === "disconnected")
    return {
      ready: false,
      title: "Workspace disconnected",
      detail:
        "Reconnect the workspace, or create a database connection directly from this app.",
    };
  if (!database.length)
    return {
      ready: false,
      title: "Connect a database",
      detail:
        "Choose a database server or an existing SQLite file to browse tables and run queries.",
    };
  if (
    !required.every((name) =>
      database.some(
        (method) =>
          method.name === prefix + name && method.available && method.granted,
      ),
    )
  )
    return {
      ready: false,
      title: "Database service unavailable",
      detail:
        "The selected workspace does not expose all database operations. Reconnect it, or connect a database directly.",
    };
  return {
    ready: true,
    title: "Connected",
    detail: "Database services are available.",
  };
}

const colors = [
  "base",
  "surface",
  "raised",
  "inset",
  "text",
  "muted",
  "accent",
  "onAccent",
  "border",
  "hover",
  "selection",
  "danger",
  "dangerSoft",
  "warning",
  "warningSoft",
  "success",
];
/** @param {{style:{setProperty:(name:string,value:string)=>void},dataset:DOMStringMap}} root
 * @param {{mode:"light"|"dark",colors:Readonly<Record<string,string>>}|undefined} appearance */
export function applyAppearance(root, appearance) {
  if (!appearance) return;
  root.dataset.mode = appearance.mode === "light" ? "light" : "dark";
  root.style.setProperty("color-scheme", root.dataset.mode);
  for (const key of colors) {
    const value = appearance.colors[key];
    if (typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value))
      root.style.setProperty(`--sc-${key}`, value);
  }
}
