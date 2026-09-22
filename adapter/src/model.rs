use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub database_type: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub kind: &'static str,
    pub display: String,
}

#[derive(Clone, Debug)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    pub warnings: Vec<String>,
    pub affected_rows: u64,
    pub truncated: bool,
    pub created: Instant,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub sql: String,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    pub result_id: String,
    pub offset: usize,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<'a> {
    pub result_id: &'a str,
    pub columns: &'a [Column],
    pub rows: &'a [Vec<Cell>],
    pub offset: usize,
    pub total_rows: usize,
    pub has_more: bool,
    pub truncated: bool,
    pub affected_rows: u64,
    pub warnings: &'a [String],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub result_id: String,
    pub suggested_name: String,
    #[serde(default)]
    pub spreadsheet_safe: bool,
}

fn default_page_size() -> usize {
    100
}
