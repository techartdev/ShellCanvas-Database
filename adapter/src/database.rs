use crate::model::{Cell, Column, ResultSet};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::TryStreamExt;
use serde::Deserialize;
use sqlx::{Column as _, Executor as _, Row, Statement as _, TypeInfo as _, ValueRef as _};
use std::{str::FromStr, time::Instant};

const MAX_ROWS: usize = 10_000;
const MAX_CELLS: usize = 500_000;
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub provider: String,
    #[serde(default)]
    pub host: String,
    pub port: Option<u16>,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub trust_server_certificate: bool,
    #[serde(default)]
    pub sqlite_path: String,
}

pub enum Database {
    Postgres(sqlx::PgPool),
    MySql(sqlx::MySqlPool),
    Sqlite(sqlx::SqlitePool),
    SqlServer(Configuration),
}

impl Database {
    pub async fn connect(c: Configuration) -> Result<Self, String> {
        match c.provider.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => {
                let options = sqlx::postgres::PgConnectOptions::new()
                    .host(required(&c.host, "host")?)
                    .port(c.port.filter(|p| *p > 0).unwrap_or(5432))
                    .database(required(&c.database, "database")?)
                    .username(required(&c.username, "username")?)
                    .password(&c.password)
                    .ssl_mode(if !c.tls {
                        sqlx::postgres::PgSslMode::Disable
                    } else if c.trust_server_certificate {
                        sqlx::postgres::PgSslMode::Require
                    } else {
                        sqlx::postgres::PgSslMode::VerifyFull
                    });
                Ok(Self::Postgres(
                    sqlx::PgPool::connect_with(options)
                        .await
                        .map_err(public_db_error)?,
                ))
            }
            "mysql" | "mariadb" => {
                let options = sqlx::mysql::MySqlConnectOptions::new()
                    .host(required(&c.host, "host")?)
                    .port(c.port.filter(|p| *p > 0).unwrap_or(3306))
                    .database(required(&c.database, "database")?)
                    .username(required(&c.username, "username")?)
                    .password(&c.password)
                    .ssl_mode(if !c.tls {
                        sqlx::mysql::MySqlSslMode::Disabled
                    } else if c.trust_server_certificate {
                        sqlx::mysql::MySqlSslMode::Required
                    } else {
                        sqlx::mysql::MySqlSslMode::VerifyIdentity
                    });
                Ok(Self::MySql(
                    sqlx::MySqlPool::connect_with(options)
                        .await
                        .map_err(public_db_error)?,
                ))
            }
            "sqlite" => {
                let path = required(&c.sqlite_path, "sqlitePath")?;
                let options = sqlx::sqlite::SqliteConnectOptions::from_str(path)
                    .map_err(|_| "SQLite path is invalid".to_owned())?
                    .create_if_missing(false);
                Ok(Self::Sqlite(
                    sqlx::SqlitePool::connect_with(options)
                        .await
                        .map_err(public_db_error)?,
                ))
            }
            "sqlserver" | "mssql" => {
                required(&c.host, "host")?;
                required(&c.database, "database")?;
                required(&c.username, "username")?;
                // Tiberius is intentionally connected per operation: a failed TDS stream cannot poison later calls.
                sqlserver_client(&c).await?;
                Ok(Self::SqlServer(c))
            }
            _ => Err("provider must be sqlite, postgresql, mysql, mariadb, or sqlserver".into()),
        }
    }

    pub fn provider(&self) -> &'static str {
        match self {
            Self::Postgres(_) => "postgresql",
            Self::MySql(_) => "mysql",
            Self::Sqlite(_) => "sqlite",
            Self::SqlServer(_) => "sqlserver",
        }
    }

    pub async fn query(&self, sql: &str) -> Result<ResultSet, String> {
        if sql.trim().is_empty() || sql.len() > 256 * 1024 {
            return Err("Query must contain 1 to 262144 characters".into());
        }
        match self {
            Self::Postgres(pool) => query_pg(pool, sql).await,
            Self::MySql(pool) => query_mysql(pool, sql).await,
            Self::Sqlite(pool) => query_sqlite(pool, sql).await,
            Self::SqlServer(c) => query_sqlserver(c, sql).await,
        }
    }

    pub async fn schemas(&self) -> Result<Vec<String>, String> {
        let sql = match self.provider() {
            "sqlite" => "SELECT name FROM pragma_database_list ORDER BY name",
            "postgresql" => {
                "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name"
            }
            "mysql" => "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name",
            _ => "SELECT name FROM sys.schemas ORDER BY name",
        };
        Ok(self
            .query(sql)
            .await?
            .rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().map(|c| c.display))
            .collect())
    }

    pub async fn tables(&self, schema: &str) -> Result<Vec<(String, String)>, String> {
        if schema.len() > 256 {
            return Err("Schema name is too long".into());
        }
        match self {
            Self::Sqlite(_) => pairs(self.query("SELECT name, type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name").await?),
            Self::Postgres(pool) => pairs(decode_pg(sqlx::query("SELECT table_name, table_type FROM information_schema.tables WHERE table_schema = $1 ORDER BY table_name").bind(schema).fetch_all(pool).await.map_err(public_db_error)?)?),
            Self::MySql(pool) => pairs(decode_mysql(sqlx::query("SELECT table_name, table_type FROM information_schema.tables WHERE table_schema = ? ORDER BY table_name").bind(schema).fetch_all(pool).await.map_err(public_db_error)?)?),
            Self::SqlServer(c) => pairs(query_sqlserver_bound(c, "SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = @P1 ORDER BY TABLE_NAME", schema).await?),
        }
    }

    pub async fn columns(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<Vec<(String, String)>, String> {
        if schema.len() > 256 || table.len() > 256 {
            return Err("Schema or table name is too long".into());
        }
        match self {
            Self::Sqlite(pool) => {
                let escaped = table.replace('"', "\"\"");
                pairs(decode_sqlite(sqlx::query(&format!("SELECT name, type FROM pragma_table_info(\"{escaped}\") ORDER BY cid")).fetch_all(pool).await.map_err(public_db_error)?)?)
            }
            Self::Postgres(pool) => pairs(decode_pg(sqlx::query("SELECT column_name, data_type FROM information_schema.columns WHERE table_schema=$1 AND table_name=$2 ORDER BY ordinal_position").bind(schema).bind(table).fetch_all(pool).await.map_err(public_db_error)?)?),
            Self::MySql(pool) => pairs(decode_mysql(sqlx::query("SELECT column_name, data_type FROM information_schema.columns WHERE table_schema=? AND table_name=? ORDER BY ordinal_position").bind(schema).bind(table).fetch_all(pool).await.map_err(public_db_error)?)?),
            Self::SqlServer(c) => {
                let mut client = sqlserver_client(c).await?;
                let stream = client.query("SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA=@P1 AND TABLE_NAME=@P2 ORDER BY ORDINAL_POSITION", &[&schema, &table]).await.map_err(public_tds_error)?;
                decode_tds(stream.into_first_result().await.map_err(public_tds_error)?).and_then(pairs)
            }
        }
    }
}

fn required<'a>(value: &'a str, name: &str) -> Result<&'a str, String> {
    if value.trim().is_empty() {
        Err(format!("{name} is required"))
    } else {
        Ok(value)
    }
}
fn clean_message(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(700)
        .collect()
}
fn public_db_error(error: sqlx::Error) -> String {
    match error {
        sqlx::Error::Database(db) => format!(
            "Database {}: {}",
            db.code().as_deref().unwrap_or("error"),
            clean_message(db.message())
        ),
        _ => "Database connection or protocol operation failed".into(),
    }
}
fn public_tds_error(error: tiberius::error::Error) -> String {
    match error {
        tiberius::error::Error::Server(token) => format!(
            "SQL Server {}: {}",
            token.code(),
            clean_message(token.message())
        ),
        _ => "SQL Server connection or protocol operation failed".into(),
    }
}

fn pairs(set: ResultSet) -> Result<Vec<(String, String)>, String> {
    Ok(set
        .rows
        .into_iter()
        .filter_map(|r| {
            if r.len() >= 2 {
                Some((r[0].display.clone(), r[1].display.clone()))
            } else {
                None
            }
        })
        .collect())
}

fn empty(
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
    warnings: Vec<String>,
    truncated: bool,
) -> Result<ResultSet, String> {
    if columns.len().saturating_mul(rows.len()) > MAX_CELLS {
        return Err("Result has too many cells".into());
    }
    Ok(ResultSet {
        columns,
        truncated,
        rows,
        warnings,
        affected_rows: 0,
        created: Instant::now(),
    })
}
fn null() -> Cell {
    Cell {
        kind: "null",
        display: String::new(),
    }
}
fn value(kind: &'static str, display: impl ToString) -> Cell {
    Cell {
        kind,
        display: display.to_string(),
    }
}
fn unsupported(name: &str) -> Cell {
    value("unsupported", format!("<unsupported: {name}>"))
}
fn expects_columns(sql: &str) -> bool {
    let mut sql = sql;
    loop {
        sql = sql.trim_start();
        if let Some(rest) = sql.strip_prefix("--") {
            sql = rest.split_once('\n').map(|(_, v)| v).unwrap_or("");
            continue;
        }
        if let Some(rest) = sql.strip_prefix("/*")
            && let Some((_, tail)) = rest.split_once("*/")
        {
            sql = tail;
            continue;
        }
        break;
    }
    matches!(
        sql.split_whitespace()
            .next()
            .map(|v| v.to_ascii_uppercase())
            .as_deref(),
        Some("SELECT" | "WITH" | "PRAGMA" | "EXPLAIN" | "SHOW" | "DESCRIBE")
    )
}

fn append_result(
    target: &mut ResultSet,
    mut one: ResultSet,
    bytes: &mut usize,
) -> Result<(), String> {
    if target.columns.is_empty() {
        target.columns = one.columns;
    }
    for warning in one.warnings.drain(..) {
        if !target.warnings.contains(&warning) {
            target.warnings.push(warning);
        }
    }
    let Some(row) = one.rows.pop() else {
        return Ok(());
    };
    let row_bytes = row
        .iter()
        .map(|c| c.display.len() + c.kind.len() + 16)
        .sum::<usize>();
    if *bytes + row_bytes > MAX_RESULT_BYTES {
        return Err("Query result exceeds the 8 MiB materialization limit; add a LIMIT or narrower projection".into());
    }
    *bytes += row_bytes;
    target.rows.push(row);
    Ok(())
}

async fn query_sqlite(pool: &sqlx::SqlitePool, sql: &str) -> Result<ResultSet, String> {
    let mut stream = sqlx::query(sql).fetch(pool);
    let mut result = empty(vec![], vec![], vec![], false)?;
    let mut bytes = 0;
    while let Some(row) = stream.try_next().await.map_err(public_db_error)? {
        if result.rows.len() == MAX_ROWS {
            result.truncated = true;
            break;
        }
        append_result(&mut result, decode_sqlite(vec![row])?, &mut bytes)?;
    }
    drop(stream);
    if result.columns.is_empty() && expects_columns(sql) {
        let statement = pool.prepare(sql).await.map_err(public_db_error)?;
        result.columns = statement
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().into(),
                database_type: c.type_info().name().into(),
            })
            .collect();
    }
    Ok(result)
}
async fn query_pg(pool: &sqlx::PgPool, sql: &str) -> Result<ResultSet, String> {
    let mut stream = sqlx::query(sql).fetch(pool);
    let mut result = empty(vec![], vec![], vec![], false)?;
    let mut bytes = 0;
    while let Some(row) = stream.try_next().await.map_err(public_db_error)? {
        if result.rows.len() == MAX_ROWS {
            result.truncated = true;
            break;
        }
        append_result(&mut result, decode_pg(vec![row])?, &mut bytes)?;
    }
    drop(stream);
    if result.columns.is_empty() && expects_columns(sql) {
        let statement = pool.prepare(sql).await.map_err(public_db_error)?;
        result.columns = statement
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().into(),
                database_type: c.type_info().name().into(),
            })
            .collect();
    }
    Ok(result)
}
async fn query_mysql(pool: &sqlx::MySqlPool, sql: &str) -> Result<ResultSet, String> {
    let mut stream = sqlx::query(sql).fetch(pool);
    let mut result = empty(vec![], vec![], vec![], false)?;
    let mut bytes = 0;
    while let Some(row) = stream.try_next().await.map_err(public_db_error)? {
        if result.rows.len() == MAX_ROWS {
            result.truncated = true;
            break;
        }
        append_result(&mut result, decode_mysql(vec![row])?, &mut bytes)?;
    }
    drop(stream);
    if result.columns.is_empty() && expects_columns(sql) {
        let statement = pool.prepare(sql).await.map_err(public_db_error)?;
        result.columns = statement
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().into(),
                database_type: c.type_info().name().into(),
            })
            .collect();
    }
    Ok(result)
}

fn decode_sqlite(mut rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<ResultSet, String> {
    rows.truncate(MAX_ROWS);
    let columns: Vec<Column> = rows
        .first()
        .map(|r| {
            r.columns()
                .iter()
                .map(|c| Column {
                    name: c.name().into(),
                    database_type: c.type_info().name().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cells = Vec::new();
        for i in 0..row.len() {
            let raw = row.try_get_raw(i).map_err(public_db_error)?;
            let t = raw.type_info().name().to_owned();
            let c = if raw.is_null() {
                null()
            } else {
                match t.as_str() {
                    "INTEGER" => value(
                        "integer",
                        row.try_get::<i64, _>(i).map_err(public_db_error)?,
                    ),
                    "REAL" => value("number", row.try_get::<f64, _>(i).map_err(public_db_error)?),
                    "TEXT" => value(
                        "text",
                        row.try_get::<String, _>(i).map_err(public_db_error)?,
                    ),
                    "BLOB" => value(
                        "binary",
                        STANDARD.encode(row.try_get::<Vec<u8>, _>(i).map_err(public_db_error)?),
                    ),
                    _ => unsupported(&t),
                }
            };
            cells.push(c);
        }
        out.push(cells);
    }
    empty(columns, out, vec![], false)
}

fn decode_pg(mut rows: Vec<sqlx::postgres::PgRow>) -> Result<ResultSet, String> {
    rows.truncate(MAX_ROWS);
    let columns: Vec<Column> = rows
        .first()
        .map(|r| {
            r.columns()
                .iter()
                .map(|c| Column {
                    name: c.name().into(),
                    database_type: c.type_info().name().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for row in rows {
        let mut cells = Vec::new();
        for (i, column) in columns.iter().enumerate().take(row.len()) {
            let raw = row.try_get_raw(i).map_err(public_db_error)?;
            let t = raw.type_info().name().to_owned();
            let c = if raw.is_null() {
                null()
            } else {
                match t.as_str() {
                    "BOOL" => value(
                        "boolean",
                        row.try_get::<bool, _>(i).map_err(public_db_error)?,
                    ),
                    "INT2" => value(
                        "integer",
                        row.try_get::<i16, _>(i).map_err(public_db_error)?,
                    ),
                    "INT4" => value(
                        "integer",
                        row.try_get::<i32, _>(i).map_err(public_db_error)?,
                    ),
                    "INT8" => value(
                        "integer",
                        row.try_get::<i64, _>(i).map_err(public_db_error)?,
                    ),
                    "FLOAT4" => value("number", row.try_get::<f32, _>(i).map_err(public_db_error)?),
                    "FLOAT8" => value("number", row.try_get::<f64, _>(i).map_err(public_db_error)?),
                    "NUMERIC" => value(
                        "decimal",
                        row.try_get::<bigdecimal::BigDecimal, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" => value(
                        "text",
                        row.try_get::<String, _>(i).map_err(public_db_error)?,
                    ),
                    "BYTEA" => value(
                        "binary",
                        STANDARD.encode(row.try_get::<Vec<u8>, _>(i).map_err(public_db_error)?),
                    ),
                    "UUID" => value(
                        "uuid",
                        row.try_get::<uuid::Uuid, _>(i).map_err(public_db_error)?,
                    ),
                    "JSON" | "JSONB" => value(
                        "json",
                        row.try_get::<serde_json::Value, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "DATE" => value(
                        "date",
                        row.try_get::<chrono::NaiveDate, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "TIME" => value(
                        "time",
                        row.try_get::<chrono::NaiveTime, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "TIMESTAMP" => value(
                        "datetime",
                        row.try_get::<chrono::NaiveDateTime, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "TIMESTAMPTZ" => value(
                        "datetime",
                        row.try_get::<chrono::DateTime<chrono::Utc>, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    _ => {
                        warnings.push(format!(
                            "Column {} uses unsupported PostgreSQL type {t}",
                            column.name
                        ));
                        unsupported(&t)
                    }
                }
            };
            cells.push(c);
        }
        out.push(cells);
    }
    warnings.sort();
    warnings.dedup();
    empty(columns, out, warnings, false)
}

fn decode_mysql(mut rows: Vec<sqlx::mysql::MySqlRow>) -> Result<ResultSet, String> {
    rows.truncate(MAX_ROWS);
    let columns: Vec<Column> = rows
        .first()
        .map(|r| {
            r.columns()
                .iter()
                .map(|c| Column {
                    name: c.name().into(),
                    database_type: c.type_info().name().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for row in rows {
        let mut cells = Vec::new();
        for (i, column) in columns.iter().enumerate().take(row.len()) {
            let raw = row.try_get_raw(i).map_err(public_db_error)?;
            let t = raw.type_info().name().to_owned();
            let c = if raw.is_null() {
                null()
            } else {
                match t.as_str() {
                    "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" => value(
                        "integer",
                        row.try_get::<i64, _>(i).map_err(public_db_error)?,
                    ),
                    "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED"
                    | "INT UNSIGNED" | "BIGINT UNSIGNED" => value(
                        "integer",
                        row.try_get::<u64, _>(i).map_err(public_db_error)?,
                    ),
                    "FLOAT" => value("number", row.try_get::<f32, _>(i).map_err(public_db_error)?),
                    "DOUBLE" => value("number", row.try_get::<f64, _>(i).map_err(public_db_error)?),
                    "DECIMAL" => value(
                        "decimal",
                        row.try_get::<bigdecimal::BigDecimal, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "VARCHAR" | "CHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT"
                    | "ENUM" | "SET" => value(
                        "text",
                        row.try_get::<String, _>(i).map_err(public_db_error)?,
                    ),
                    "VARBINARY" | "BINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => {
                        value(
                            "binary",
                            STANDARD.encode(row.try_get::<Vec<u8>, _>(i).map_err(public_db_error)?),
                        )
                    }
                    "JSON" => value(
                        "json",
                        row.try_get::<serde_json::Value, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "DATE" => value(
                        "date",
                        row.try_get::<chrono::NaiveDate, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "TIME" => value(
                        "time",
                        row.try_get::<chrono::NaiveTime, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    "DATETIME" | "TIMESTAMP" => value(
                        "datetime",
                        row.try_get::<chrono::NaiveDateTime, _>(i)
                            .map_err(public_db_error)?,
                    ),
                    _ => {
                        warnings.push(format!(
                            "Column {} uses unsupported MySQL type {t}",
                            column.name
                        ));
                        unsupported(&t)
                    }
                }
            };
            cells.push(c);
        }
        out.push(cells);
    }
    warnings.sort();
    warnings.dedup();
    empty(columns, out, warnings, false)
}

type TdsClient = tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>;
async fn sqlserver_client(c: &Configuration) -> Result<TdsClient, String> {
    use tokio_util::compat::TokioAsyncWriteCompatExt;
    let mut config = tiberius::Config::new();
    config.host(&c.host);
    config.port(c.port.filter(|p| *p > 0).unwrap_or(1433));
    config.database(&c.database);
    config.authentication(tiberius::AuthMethod::sql_server(&c.username, &c.password));
    if !c.tls {
        config.encryption(tiberius::EncryptionLevel::NotSupported);
    }
    if c.trust_server_certificate {
        config.trust_cert();
    }
    let tcp = tokio::net::TcpStream::connect(config.get_addr())
        .await
        .map_err(|_| "Could not connect to SQL Server".to_owned())?;
    tcp.set_nodelay(true)
        .map_err(|_| "Could not configure SQL Server connection".to_owned())?;
    tiberius::Client::connect(config, tcp.compat_write())
        .await
        .map_err(public_tds_error)
}
async fn query_sqlserver(c: &Configuration, sql: &str) -> Result<ResultSet, String> {
    let mut client = sqlserver_client(c).await?;
    let mut stream = client.simple_query(sql).await.map_err(public_tds_error)?;
    let mut result = empty(vec![], vec![], vec![], false)?;
    let mut bytes = 0;
    while let Some(item) = stream.try_next().await.map_err(public_tds_error)? {
        match item {
            tiberius::QueryItem::Metadata(meta) if meta.result_index() == 0 => {
                if result.columns.is_empty() {
                    result.columns = meta
                        .columns()
                        .iter()
                        .map(|c| Column {
                            name: c.name().into(),
                            database_type: format!("{:?}", c.column_type()),
                        })
                        .collect();
                }
            }
            tiberius::QueryItem::Row(row) if row.result_index() == 0 => {
                if result.rows.len() == MAX_ROWS {
                    result.truncated = true;
                    break;
                }
                append_result(&mut result, decode_tds(vec![row])?, &mut bytes)?;
            }
            tiberius::QueryItem::Metadata(meta) if meta.result_index() > 0 => {
                result
                    .warnings
                    .push("Only the first result set is displayed".into());
                break;
            }
            _ => {}
        }
    }
    Ok(result)
}
async fn query_sqlserver_bound(
    c: &Configuration,
    sql: &str,
    arg: &str,
) -> Result<ResultSet, String> {
    let mut client = sqlserver_client(c).await?;
    let rows = client
        .query(sql, &[&arg])
        .await
        .map_err(public_tds_error)?
        .into_first_result()
        .await
        .map_err(public_tds_error)?;
    decode_tds(rows)
}
fn numeric_text(value: tiberius::numeric::Numeric) -> String {
    let raw = value.value();
    let scale = value.scale() as usize;
    let negative = raw < 0;
    let mut digits = raw.unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits = format!("{}{}", "0".repeat(scale + 1 - digits.len()), digits);
        }
        digits.insert(digits.len() - scale, '.');
    }
    if negative {
        format!("-{digits}")
    } else {
        digits
    }
}
fn decode_tds(mut rows: Vec<tiberius::Row>) -> Result<ResultSet, String> {
    use tiberius::ColumnData as D;
    rows.truncate(MAX_ROWS);
    let columns = rows
        .first()
        .map(|r| {
            r.columns()
                .iter()
                .map(|c| Column {
                    name: c.name().into(),
                    database_type: format!("{:?}", c.column_type()),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for row in rows {
        let mut cells = Vec::new();
        for (i, (col, data)) in row.cells().enumerate() {
            let c = match data {
                D::U8(v) => v.map(|x| value("integer", x)),
                D::I16(v) => v.map(|x| value("integer", x)),
                D::I32(v) => v.map(|x| value("integer", x)),
                D::I64(v) => v.map(|x| value("integer", x)),
                D::F32(v) => v.map(|x| value("number", x)),
                D::F64(v) => v.map(|x| value("number", x)),
                D::Bit(v) => v.map(|x| value("boolean", x)),
                D::String(v) => v.as_ref().map(|x| value("text", x)),
                D::Guid(v) => v.map(|x| value("uuid", x)),
                D::Binary(v) => v.as_ref().map(|x| value("binary", STANDARD.encode(x))),
                D::Numeric(v) => v.map(|x| value("decimal", numeric_text(x))),
                D::Xml(v) => v.as_ref().map(|x| value("text", format!("{x:?}"))),
                D::DateTime(_) | D::SmallDateTime(_) | D::DateTime2(_) => row
                    .try_get::<chrono::NaiveDateTime, _>(i)
                    .map_err(public_tds_error)?
                    .map(|x| value("datetime", x)),
                D::Date(_) => row
                    .try_get::<chrono::NaiveDate, _>(i)
                    .map_err(public_tds_error)?
                    .map(|x| value("date", x)),
                D::Time(_) => row
                    .try_get::<chrono::NaiveTime, _>(i)
                    .map_err(public_tds_error)?
                    .map(|x| value("time", x)),
                D::DateTimeOffset(_) => row
                    .try_get::<chrono::DateTime<chrono::FixedOffset>, _>(i)
                    .map_err(public_tds_error)?
                    .map(|x| value("datetime", x)),
            }
            .unwrap_or_else(null);
            if c.kind == "unsupported" {
                warnings.push(format!("Column {} could not be decoded", col.name()));
            }
            cells.push(c);
        }
        out.push(cells);
    }
    warnings.sort();
    warnings.dedup();
    empty(columns, out, warnings, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn sqlite_preserves_types_and_metadata() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let url = format!("sqlite:{}", file.path().display());
        let db = Database::connect(Configuration {
            provider: "sqlite".into(),
            host: "".into(),
            port: None,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            tls: false,
            trust_server_certificate: false,
            sqlite_path: url,
        })
        .await
        .unwrap();
        db.query("CREATE TABLE items(id INTEGER, label TEXT, payload BLOB, score REAL)")
            .await
            .unwrap();
        db.query("INSERT INTO items VALUES(9007199254740993, 'café', x'00ff', 1.5)")
            .await
            .unwrap();
        let result = db.query("SELECT * FROM items").await.unwrap();
        assert_eq!(result.rows[0][0].display, "9007199254740993");
        assert_eq!(result.rows[0][1].display, "café");
        assert_eq!(result.rows[0][2].display, "AP8=");
        assert!(
            db.tables("main")
                .await
                .unwrap()
                .iter()
                .any(|(name, _)| name == "items")
        );
        assert_eq!(db.columns("main", "items").await.unwrap()[0].0, "id");
    }
    #[tokio::test]
    async fn sqlite_zero_rows_keep_columns_and_large_results_are_bounded() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let db = Database::connect(Configuration {
            provider: "sqlite".into(),
            host: "".into(),
            port: None,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            tls: false,
            trust_server_certificate: false,
            sqlite_path: format!("sqlite:{}", file.path().display()),
        })
        .await
        .unwrap();
        let empty = db
            .query("-- empty result\nSELECT 1 AS id, 'x' AS label WHERE 0")
            .await
            .unwrap();
        assert_eq!(
            empty
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["id", "label"]
        );
        assert!(empty.rows.is_empty());
        let bounded=db.query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<10001) SELECT x FROM n").await.unwrap();
        assert_eq!(bounded.rows.len(), MAX_ROWS);
        assert!(bounded.truncated);
    }
    #[tokio::test]
    async fn sqlite_rejects_results_over_byte_budget() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let db = Database::connect(Configuration {
            provider: "sqlite".into(),
            host: "".into(),
            port: None,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            tls: false,
            trust_server_certificate: false,
            sqlite_path: format!("sqlite:{}", file.path().display()),
        })
        .await
        .unwrap();
        let error = db
            .query("SELECT printf('%*s', 9000000, 'x') AS huge")
            .await
            .unwrap_err();
        assert!(error.contains("8 MiB"));
    }
    #[test]
    fn sqlserver_numeric_text_is_lossless() {
        use tiberius::numeric::Numeric;
        assert_eq!(numeric_text(Numeric::new_with_scale(-1, 2)), "-0.01");
        assert_eq!(
            numeric_text(Numeric::new_with_scale(
                12345678901234567890123456789012345678i128,
                0
            )),
            "12345678901234567890123456789012345678"
        );
    }
}
