use thiserror::Error;

#[derive(Debug, Error)]
pub enum GcsError {
    #[error("Gcloud error: {0}")]
    GcloudError(#[from] reqwest::Error),

    #[error("File path invalid")]
    FilePathInvalid,

    // bigquery error
    #[error("BigQuery error: {0}")]
    BigQueryError(#[from] gcp_bigquery_client::error::BQError),

    // TableSchema is invalid
    #[error("TableSchema is not account")]
    TableSchemaNotAccount,

    // TableDataInsertAllResponse is invalid
    #[error("Failed to upsert account to BigQuery")]
    BigQueryUpsertError,

    #[error("Value is None")]
    None,

    #[error("Returned empty rows")]
    EmptyRows,

    #[error("Returned empty columns")]
    EmptyColumns,

    #[error("Column not found {0}")]
    ColumnMissing(String),

    #[error("Column value missing {0}")]
    ColumnValueMissing(String),

    #[error("Slot not found")]
    SlotNotFound,
}

pub type GcsResult<T> = Result<T, GcsError>;
