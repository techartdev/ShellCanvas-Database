# ShellCanvas Database

A first-party companion app for [ShellCanvas](https://github.com/techartdev/ShellCanvas) that connects directly to PostgreSQL, MySQL/MariaDB, Microsoft SQL Server, and SQLite.

Version 0.1.0 browses schemas and tables, runs SQL, displays bounded typed results, pages through the retained result, and saves the full retained result as CSV. It does not provide grid editing or a DDL designer.

> **SQL is not filtered into a read-only subset.** Statements run with the configured database account's privileges. Use database grants appropriate for your work. The app repeats this warning at run time.

## Install the initial Windows build

With the desktop build in ShellCanvas PR #23, open **App Manager → Add apps → From GitHub** and enter `techartdev/ShellCanvas-Database` with reference `codex/unified-native-install`. Review and install the app and its Windows connector together.

Open Database, choose **Connect database**, then select SQL Server, PostgreSQL, MySQL, MariaDB or SQLite. Enter the host, TCP port, database and credentials, or an existing SQLite file path. The app shows the endpoint and actual connection state. No database connection starts until you press Connect. Passwords are not saved.

Each app window owns its connection; Disconnect or closing the window releases it. You can still use a database service already configured on an accepted workspace. The app follows the desktop's dark/light theme and updates when it changes.

There is no automatic network or SSH-host instance discovery. Connections originate from the desktop PC. SQL Server named instances require their TCP port; SQLite files are local to this PC. Schema/table enumeration starts after a connection succeeds.

Keep this PR unmerged until a compatible desktop release ships.

ShellCanvas 0.1.11 and older require the existing manual order: install
`dist/adapter-windows-x86_64/adapter.json` under **Connection adapters**, add and
configure that source, then install `app/dist/app.shellcanvas.json` or the app
from GitHub and grant `services.dev.shellcanvas.database`.

This app is intentionally not in ShellCanvas's recommended catalog yet. The checked-in Windows package is the initial test build. PostgreSQL, MySQL/MariaDB, and SQL Server paths compile but still need disposable-server and user acceptance testing; SQLite has an automated real-database integration test.

## Connection fields

Set `provider` to `sqlite`, `postgresql`, `mysql`, `mariadb`, or `sqlserver`.

- SQLite uses `sqlitePath`, such as `sqlite:C:/data/example.db` or `sqlite:/home/me/example.db`. The path is local to the machine running the adapter, not an SSH host.
- Network providers use host, optional port (`0` selects the default), database, username, and password.
- TLS verifies the certificate and host identity by default. Trusting a server certificate without identity verification is an explicit insecure opt-in for private/self-signed deployments.
- Password fields are not retained in saved ShellCanvas profiles; supply the password when connecting.

ShellCanvas does not currently expose SSH port forwarding to custom adapters. Use an endpoint reachable from the adapter machine or manage a tunnel separately.

## Results and CSV

- Queries materialize at most 10,000 rows and 8 MiB for five minutes. Pages are capped below the adapter frame ceiling. Truncated results are marked.
- Integers and decimals are strings, preserving values beyond JavaScript's exact range. Binary is base64. Unsupported database types show a placeholder and warning instead of silent coercion.
- **Spreadsheet-safe CSV** defaults on. It prefixes text-like cells and headers beginning with `=`, `+`, `-`, `@`, tab, or carriage return with an apostrophe. Turn it off for raw displayed text. Null is an empty field; CSV is UTF-8 with BOM, quoted where needed, with CRLF lines.
- Save runs as a leased adapter job. Closing or rebinding requests cancellation; abandoned jobs cannot commit once their polling lease expires. Once atomic commit starts, cancellation truthfully reports that it is too late.

## Build and test

Requirements: Rust 1.93+, Node.js 20+, and npm.

```powershell
cargo test -p shellcanvas-database-adapter --locked
cargo clippy -p shellcanvas-database-adapter --all-targets --locked -- -D warnings
cd app
npm install
npm run build
npm run repository
cd ..
```

The repository command uses the pinned 0.1.11 app SDK for the app artifact, then
validates and reattaches the native dependency with a freshly computed adapter
manifest hash. Regeneration therefore cannot silently drop or stale the pin.

Build/package the native adapter with the vendored ShellCanvas 0.1.11 SDK:

```powershell
cargo run --manifest-path vendor/shellcanvas-adapter-sdk/Cargo.toml --bin shellcanvas-adapter -- build adapter dist/adapter-windows-x86_64
```

Use a fresh output directory for each package. The source pins SQLx 0.8.6 and uses concrete PostgreSQL, MySQL, and SQLite decoders rather than SQLx `Any`. SQL Server uses Tiberius 0.12.3. Optional live-provider credentials are never stored here; future disposable-server tests will be explicitly gated. Never point development tests at production databases.

## Layout

- `app/` — runtime app and app package
- `adapter/` — trusted native database adapter
- `vendor/` — exact ShellCanvas 0.1.11 SDK inputs
- `dist/` — installable checked-in artifacts

Licensed under MPL-2.0. See [LICENSE](LICENSE).

### Sandboxed connection regression

After building the app, run `node app/scripts/check-sandbox.mjs` with Playwright available. If supplied externally, set `PLAYWRIGHT_MODULE` to its entry point and optionally `BROWSER_EXECUTABLE` to a Chromium/Edge executable. This exercises the actual built package and SDK inside an `allow-scripts` iframe with form submission blocked, covering SQL Server click, Enter retry, SQLite click, required-field validation and visible connection errors. The broker is a local test fixture; no real database or credentials are used.
