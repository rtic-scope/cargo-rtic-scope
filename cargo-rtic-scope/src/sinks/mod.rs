//! A sink to which [`TraceData`] and [`api::EventChunk`]s are for
//! online and post-mortem analysis.
use crate::diag;
use crate::TraceData;

use rtic_scope_api as api;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SinkError {
    #[error("Failed to setup sink during I/O:{} {1}", { if let Some(s) = .0 {
        format!(" {}:", s)
    } else {
        "".to_string()
    }})]
    SetupIOError(Option<String>, #[source] std::io::Error),
    #[error("Failed to find git repo while traversing upwards from {}", .0.display())]
    NoGitRoot(std::path::PathBuf),
    #[error("Failed to read git repository of artifact: {0}")]
    GitError(#[from] git2::Error),
    #[error("Failed to serialize trace data: {0}")]
    DrainSerError(#[from] serde_json::Error),
    #[error("Failed to drain trace data on I/O: {0}")]
    DrainIOError(#[source] std::io::Error),
    #[error("Failed to reset target device: {0}")]
    ResetError(#[from] probe_rs::Error),
    #[error("Failed to setup sink because the source failed: {0}")]
    SourceError(#[from] crate::sources::SourceError),
}

impl diag::DiagnosableError for SinkError {}

pub mod file;
pub use file::FileSink;

mod frontend;
pub use frontend::FrontendSink;

pub trait Sink {
    fn drain(&mut self, data: TraceData, chunk: api::EventChunk) -> Result<(), SinkError>;
    fn describe(&self) -> String;
}
