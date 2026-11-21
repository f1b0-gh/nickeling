//! Typed technical failures. Application errors belong to consumer adapters.
use std::{ffi::OsString, io, path::PathBuf, process::ExitStatus, time::Duration};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Configuration invalide : {message}")]
    InvalidConfiguration { message: String },
    #[error("Chemin invalide {path:?} : {reason}")]
    InvalidPath { path: PathBuf, reason: &'static str },
    #[error("Entrée refusée : {message}")]
    InvalidInput { message: String },
    #[error("Erreur d’E/S : {0}")]
    Io(#[from] io::Error),
    #[error("{path:?} : {source}")]
    PathIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("JSON invalide : {0}")]
    Json(#[from] serde_json::Error),
    #[error("Sortie non UTF-8 : {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("Octet nul dans un chemin : {0}")]
    Nul(#[from] std::ffi::NulError),
    #[error("Impossible de lancer {program:?} : {source}")]
    ProcessSpawn {
        program: OsString,
        #[source]
        source: io::Error,
    },
    #[error("Échec {program:?} ({status}) : {stderr}")]
    ProcessExit {
        program: OsString,
        status: ExitStatus,
        stderr: String,
    },
    #[error("Délai de {program:?} dépassé ({timeout:?})")]
    ProcessTimeout {
        program: OsString,
        timeout: Duration,
    },
    #[error("Sortie {stream} de {program:?} trop volumineuse (limite {limit} octets)")]
    ProcessOutputLimit {
        program: OsString,
        stream: &'static str,
        limit: usize,
    },
    #[error("Pipe {stream} absent pour {program:?}")]
    MissingPipe {
        program: OsString,
        stream: &'static str,
    },
    #[error("Évaluation Nickel refusée : {source}")]
    Nickel {
        #[source]
        source: Box<Error>,
    },
    #[error("{message} : {source}")]
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
