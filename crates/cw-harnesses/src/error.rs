#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Store error: {0}")]
    Store(#[from] cw_store::StoreError),
    #[error("Normalize error: {0}")]
    Normalize(#[from] cw_core::normalizer::NormalizeError),
    #[error("No source transcript on disk recorded for session '{0}'")]
    NoTranscriptSource(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
