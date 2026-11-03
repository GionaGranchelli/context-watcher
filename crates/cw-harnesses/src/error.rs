#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error(
        "Unsupported OMP session ID: including the omp_ prefix, use at most 128 ASCII letters, digits, underscores or hyphens with a nonempty native ID"
    )]
    InvalidOmpSessionId,
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
