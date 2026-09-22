// SPDX-License-Identifier: MPL-2.0
mod csv;
mod database;
mod model;

use database::{Configuration, Database};
use model::{ExportRequest, Page, PageRequest, QueryRequest, ResultSet};
use serde::Deserialize;
use shellcanvas_adapter_sdk::{
    Adapter, CallError, RequestContext, ServiceDescriptor, Value, async_trait, json, run,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};

const SERVICE: &str = "dev.shellcanvas.database";
const MAX_SESSIONS: usize = 8;

#[derive(Clone)]
struct ExportJob {
    state: Arc<Mutex<ExportState>>,
    canceled: Arc<AtomicBool>,
    last_poll: Arc<Mutex<Instant>>,
    created: Instant,
}
#[derive(Clone)]
enum ExportState {
    Choosing,
    Writing,
    Saved(PathBuf),
    Canceled,
    Failed(String),
}

#[derive(Default)]
struct DatabaseAdapter {
    database: RwLock<Option<Arc<Database>>>,
    results: Mutex<HashMap<String, Arc<ResultSet>>>,
    exports: Mutex<HashMap<String, ExportJob>>,
}

#[derive(Deserialize)]
struct NameParam {
    schema: String,
}
#[derive(Deserialize)]
struct TableParam {
    schema: String,
    table: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobParam {
    job_id: String,
}

fn invalid(message: impl Into<String>) -> CallError {
    CallError::new("invalid", message)
}
fn failed(message: impl Into<String>) -> CallError {
    CallError::new("failed", message)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CallError> {
    serde_json::from_value(value).map_err(|_| invalid("Request parameters are invalid"))
}

impl DatabaseAdapter {
    async fn db(&self) -> Result<Arc<Database>, CallError> {
        self.database
            .read()
            .await
            .clone()
            .ok_or_else(|| CallError::new("closed", "Database is not initialized"))
    }
    async fn page(&self, request: PageRequest) -> Result<Value, CallError> {
        let mut results = self.results.lock().await;
        results.retain(|_, v| v.created.elapsed() < Duration::from_secs(300));
        let set = results
            .get(&request.result_id)
            .ok_or_else(|| CallError::new("closed", "Query result expired; run the query again"))?;
        let size = request.page_size.clamp(1, 500);
        let start = request.offset.min(set.rows.len());
        let mut end = start;
        let mut bytes = 0usize;
        while end < set.rows.len() && end < start + size {
            let row_bytes = set.rows[end]
                .iter()
                .map(|c| c.display.len() + c.kind.len() + 32)
                .sum::<usize>();
            if row_bytes > 512 * 1024 {
                return Err(failed("A result row is too large to display"));
            }
            if bytes + row_bytes > 512 * 1024 {
                break;
            }
            bytes += row_bytes;
            end += 1;
        }
        serde_json::to_value(Page {
            result_id: &request.result_id,
            columns: &set.columns,
            rows: &set.rows[start..end],
            offset: start,
            total_rows: set.rows.len(),
            has_more: end < set.rows.len(),
            truncated: set.truncated,
            affected_rows: set.affected_rows,
            warnings: &set.warnings,
        })
        .map_err(|_| failed("Could not encode query result"))
    }
    async fn begin_export(
        &self,
        request: ExportRequest,
        context: &RequestContext,
    ) -> Result<Value, CallError> {
        if context.is_canceled() {
            return Err(CallError::new("aborted", "CSV export canceled"));
        }
        let mut results = self.results.lock().await;
        results.retain(|_, v| v.created.elapsed() < Duration::from_secs(300));
        let set = results
            .get(&request.result_id)
            .cloned()
            .ok_or_else(|| CallError::new("closed", "Query result expired; run the query again"))?;
        drop(results);
        let bytes =
            csv::encode(&set.columns, &set.rows, request.spreadsheet_safe).map_err(invalid)?;
        let job_id = uuid::Uuid::new_v4().to_string();
        let state = Arc::new(Mutex::new(ExportState::Choosing));
        let canceled = Arc::new(AtomicBool::new(false));
        let last_poll = Arc::new(Mutex::new(Instant::now()));
        let existing = self
            .exports
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut active = 0;
        for job in existing {
            if matches!(
                *job.state.lock().await,
                ExportState::Choosing | ExportState::Writing
            ) {
                active += 1;
            }
        }
        if active >= 8 {
            return Err(CallError::new(
                "busy",
                "Finish or cancel an existing CSV export first",
            ));
        }
        let mut exports = self.exports.lock().await;
        exports.retain(|_, j| j.created.elapsed() < Duration::from_secs(600));
        exports.insert(
            job_id.clone(),
            ExportJob {
                state: state.clone(),
                canceled: canceled.clone(),
                last_poll: last_poll.clone(),
                created: Instant::now(),
            },
        );
        drop(exports);
        let name =
            if request.suggested_name.ends_with(".csv") && request.suggested_name.len() <= 120 {
                request.suggested_name
            } else {
                "query-results.csv".into()
            };
        tokio::spawn(async move {
            let picked = rfd::AsyncFileDialog::new()
                .set_file_name(&name)
                .add_filter("CSV file", &["csv"])
                .save_file()
                .await;
            let Some(handle) = picked else {
                *state.lock().await = ExportState::Canceled;
                return;
            };
            let path = handle.path().to_owned();
            let lease_alive = last_poll.lock().await.elapsed() < Duration::from_secs(15);
            let mut current = state.lock().await;
            if canceled.load(Ordering::SeqCst)
                || !lease_alive
                || !matches!(*current, ExportState::Choosing)
            {
                *current = ExportState::Canceled;
                return;
            }
            *current = ExportState::Writing;
            drop(current);
            let result = atomic_write(&path, &bytes).await;
            *state.lock().await = match result {
                Ok(()) => ExportState::Saved(path),
                Err(message) => ExportState::Failed(message),
            };
        });
        Ok(json!({"jobId":job_id}))
    }
}

async fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Destination has no parent directory".to_owned())?
        .to_owned();
    let path = path.to_owned();
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "Could not create temporary export file".to_owned())?;
        temp.write_all(&bytes)
            .and_then(|_| temp.flush())
            .map_err(|_| "Could not write export file".to_owned())?;
        temp.as_file()
            .sync_all()
            .map_err(|_| "Could not flush export file".to_owned())?;
        temp.persist(path)
            .map_err(|_| "Could not replace the selected export file".to_owned())?;
        Ok(())
    })
    .await
    .map_err(|_| "Export task failed".to_owned())?
}

#[async_trait]
impl Adapter for DatabaseAdapter {
    async fn initialize(
        &self,
        configuration: Value,
        context: RequestContext,
    ) -> Result<Vec<ServiceDescriptor>, CallError> {
        let c: Configuration = parse(configuration)?;
        let connect = Database::connect(c);
        tokio::pin!(connect);
        let db = tokio::select! {v=&mut connect=>v.map_err(failed)?,_ = context.canceled()=>return Err(CallError::new("aborted","Database connection canceled"))};
        *self.database.write().await = Some(Arc::new(db));
        Ok(vec![ServiceDescriptor {
            id: SERVICE.into(),
            version: 1,
            methods: [
                "capabilities",
                "schemas",
                "tables",
                "columns",
                "query",
                "page",
                "closeResult",
                "exportBegin",
                "exportStatus",
                "exportCancel",
            ]
            .into_iter()
            .map(|m| format!("{SERVICE}.{m}"))
            .collect(),
        }])
    }
    async fn call(
        &self,
        method: &str,
        params: Value,
        context: RequestContext,
    ) -> Result<Value, CallError> {
        match method {
            "dev.shellcanvas.database.capabilities" => {
                let db = self.db().await?;
                Ok(
                    json!({"provider":db.provider(),"maxRows":10000,"maxPageSize":500,"queryPrivilegeNotice":"SQL runs with the configured database account's privileges."}),
                )
            }
            "dev.shellcanvas.database.schemas" => {
                Ok(json!({"schemas":self.db().await?.schemas().await.map_err(failed)?}))
            }
            "dev.shellcanvas.database.tables" => {
                let p: NameParam = parse(params)?;
                let tables = self.db().await?.tables(&p.schema).await.map_err(failed)?;
                Ok(
                    json!({"tables":tables.into_iter().map(|(name,kind)|json!({"name":name,"kind":kind})).collect::<Vec<_>>()}),
                )
            }
            "dev.shellcanvas.database.columns" => {
                let p: TableParam = parse(params)?;
                let columns = self
                    .db()
                    .await?
                    .columns(&p.schema, &p.table)
                    .await
                    .map_err(failed)?;
                Ok(
                    json!({"columns":columns.into_iter().map(|(name,data_type)|json!({"name":name,"databaseType":data_type})).collect::<Vec<_>>()}),
                )
            }
            "dev.shellcanvas.database.query" => {
                let p: QueryRequest = parse(params)?;
                let db = self.db().await?;
                let query = db.query(&p.sql);
                tokio::pin!(query);
                let set = tokio::select! {v=&mut query=>v.map_err(failed)?,_ = context.canceled()=>return Err(CallError::new("aborted","Query canceled"))};
                let id = uuid::Uuid::new_v4().to_string();
                let mut results = self.results.lock().await;
                results.retain(|_, v| v.created.elapsed() < Duration::from_secs(300));
                if results.len() >= MAX_SESSIONS
                    && let Some(old) = results
                        .iter()
                        .min_by_key(|(_, v)| v.created)
                        .map(|(k, _)| k.clone())
                {
                    results.remove(&old);
                }
                results.insert(id.clone(), Arc::new(set));
                drop(results);
                self.page(PageRequest {
                    result_id: id,
                    offset: 0,
                    page_size: p.page_size,
                })
                .await
            }
            "dev.shellcanvas.database.page" => self.page(parse(params)?).await,
            "dev.shellcanvas.database.closeResult" => {
                let p: PageRequest = parse(params)?;
                self.results.lock().await.remove(&p.result_id);
                Ok(json!({"closed":true}))
            }
            "dev.shellcanvas.database.exportBegin" => {
                self.begin_export(parse(params)?, &context).await
            }
            "dev.shellcanvas.database.exportStatus" => {
                let p: JobParam = parse(params)?;
                let mut exports = self.exports.lock().await;
                exports.retain(|_, j| j.created.elapsed() < Duration::from_secs(600));
                let job = exports
                    .get(&p.job_id)
                    .cloned()
                    .ok_or_else(|| CallError::new("closed", "Export job not found"))?;
                drop(exports);
                *job.last_poll.lock().await = Instant::now();
                let state = job.state.lock().await.clone();
                let terminal = matches!(
                    state,
                    ExportState::Saved(_) | ExportState::Canceled | ExportState::Failed(_)
                );
                let response = match state {
                    ExportState::Choosing => json!({"state":"choosing"}),
                    ExportState::Writing => json!({"state":"writing"}),
                    ExportState::Saved(path) => {
                        json!({"state":"saved","name":path.file_name().and_then(|v|v.to_str()).unwrap_or("export.csv")})
                    }
                    ExportState::Canceled => json!({"state":"canceled"}),
                    ExportState::Failed(message) => json!({"state":"failed","message":message}),
                };
                if terminal {
                    self.exports.lock().await.remove(&p.job_id);
                }
                Ok(response)
            }
            "dev.shellcanvas.database.exportCancel" => {
                let p: JobParam = parse(params)?;
                let job = self.exports.lock().await.get(&p.job_id).cloned();
                let canceled = if let Some(job) = job {
                    let mut state = job.state.lock().await;
                    if matches!(*state, ExportState::Choosing) {
                        job.canceled.store(true, Ordering::SeqCst);
                        *state = ExportState::Canceled;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                Ok(json!({"canceled":canceled}))
            }
            _ => Err(CallError::new(
                "unavailable",
                "Unsupported database operation",
            )),
        }
    }
}

fn main() -> std::io::Result<()> {
    run(DatabaseAdapter::default())
}
