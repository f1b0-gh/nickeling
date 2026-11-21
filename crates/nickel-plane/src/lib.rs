//! Composition orchestration; domain compilation and execution remain separate.
pub mod backend;
pub mod model;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("Unsupported capability: {0}")]
    Unsupported(String),
    #[error("Ownership or precondition conflict: {0}")]
    Conflict(String),
    #[error("Operation cancelled at a safe boundary")]
    Cancelled,
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Store failure: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("Compilation failed: {0}")]
    Util(#[from] nickel_util::error::Error),
    #[error("Backend operation failed ({0}); private diagnostics withheld")]
    Backend(String),
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn require(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(message.into()))
    }
}

pub mod composition;

pub mod reconcile;
pub mod store;

pub mod adapters;
pub mod targets;
