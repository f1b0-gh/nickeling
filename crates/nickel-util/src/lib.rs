//! Shared technical foundations for independent application adapters.
//! Domain models belong to their consumer libraries.
//!
//! ```compile_fail
//! use nickel_util::config::Config;
//! ```
//!
//! ```compile_fail
//! use nickel_util::audit::runtime::StoppedExecution;
//! ```
//!
//! ```compile_fail
//! use nickel_util::artifact::ArtifactRequest;
//! ```
pub mod error;
pub mod evaluation;
pub mod files;
pub mod json;
pub mod process;
pub mod sources;
