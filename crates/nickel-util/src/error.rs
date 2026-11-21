//! Typed technical failures. Application errors belong to consumer adapters.
use std::{ffi::OsString, io, path::PathBuf, process::ExitStatus, time::Duration};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid configuration: {message}")]
    InvalidConfiguration { message: String },
    #[error("Invalid path {path:?}: {reason}")]
    InvalidPath { path: PathBuf, reason: &'static str },
    #[error("Input rejected: {message}")]
    InvalidInput { message: String },
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("{path:?}: {source}")]
    PathIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Non-UTF-8 output: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("NUL byte in path: {0}")]
    Nul(#[from] std::ffi::NulError),
    #[error("Cannot start {program:?}: {source}")]
    ProcessSpawn {
        program: OsString,
        #[source]
        source: io::Error,
    },
    #[error("{program:?} failed ({status}): {stderr}")]
    ProcessExit {
        program: OsString,
        status: ExitStatus,
        stderr: String,
    },
    #[error("{program:?} timed out ({timeout:?})")]
    ProcessTimeout {
        program: OsString,
        timeout: Duration,
    },
    #[error("{stream} output from {program:?} exceeds the {limit}-byte limit")]
    ProcessOutputLimit {
        program: OsString,
        stream: &'static str,
        limit: usize,
    },
    #[error("Missing {stream} pipe for {program:?}")]
    MissingPipe {
        program: OsString,
        stream: &'static str,
    },
    #[error("Nickel evaluation rejected: {source}")]
    Nickel {
        #[source]
        source: Box<Error>,
    },
    #[error("{message}: {source}")]
    Context {
        message: String,
        #[source]
        source: Box<Error>,
    },
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn context(self, message: impl Into<String>) -> Self {
        Self::Context {
            message: message.into(),
            source: Box::new(self),
        }
    }
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Io(_)
            | Self::PathIo { .. }
            | Self::ProcessSpawn { .. }
            | Self::ProcessExit { .. }
            | Self::ProcessTimeout { .. }
            | Self::ProcessOutputLimit { .. }
            | Self::MissingPipe { .. }
            | Self::Utf8(_) => 1,
            Self::Context { source, .. } => source.exit_code(),
            Self::Nickel { source } => match source.as_ref() {
                Self::ProcessExit { .. } => 2,
                _ => source.exit_code(),
            },
            Self::Json(error) if error.is_io() => 1,
            _ => 2,
        }
    }
}
pub fn exit_code(error: &Error) -> i32 {
    error.exit_code()
}
// These macros only construct the module's typed validation error, never panic.
macro_rules! ensure {
 ($condition:expr,$($message:tt)*) => {if !$condition {return Err(invalid(format!($($message)*)));}};
}
pub(crate) use ensure;
