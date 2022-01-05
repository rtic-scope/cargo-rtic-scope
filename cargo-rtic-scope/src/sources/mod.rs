//! A source from which trace information is read. This [`TraceData`] is
//! mapped to RTIC tasks and forwarded to configured sinks (files and
//! frontends).
use crate::diag;
use crate::TraceData;

use thiserror::Error;

#[derive(Debug)]
pub enum BufferStatus {
    /// The given amount of bytes that are available in the buffer.
    Avail(i64),
    /// The given amount of bytes that are available in the buffer and
    /// the full buffer size. The buffer is 3/4 filles and the user
    /// should be waned that data is not read quickly enough.
    AvailWarn(i64, i64),
    /// Available buffer size could not be found.
    Unknown,
    /// Input buffer size is not a concern for this source.
    NotApplicable,
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Error)]
pub enum SourceError {
    #[error("Failed to setup source: {0}")]
    SetupError(String),
    #[error("Failed to setup source during I/O: {0}")]
    SetupIOError(#[source] std::io::Error),
    #[error("Failed to setup source probe: {0}")]
    ProbeError(#[from] probe_rs::Error),
    #[error("Failed to deserialize trace data from source: {0}")]
    IterDeserError(#[from] serde_json::Error),
    #[error("Failed to read trace data from file: {0}")]
    IterIOError(#[source] std::io::Error),
    #[error("Failed to read trace data from probe: {0}")]
    IterProbeError(#[source] probe_rs::Error),
    #[error("Failed to reset target device: {0}")]
    ResetError(#[source] probe_rs::Error),
    #[error("Failed to decode ITM packets: {0}")]
    DecodeError(#[from] itm::DecoderError),
}

impl diag::DiagnosableError for SourceError {}

pub trait Source: Iterator<Item = Result<TraceData, SourceError>> + std::marker::Send {
    fn reset_target(&mut self, _reset_halt: bool) -> Result<(), SourceError> {
        Ok(())
    }

    /// Reports the available bytes in the input buffer, if able.
    fn avail_buffer(&self) -> BufferStatus {
        BufferStatus::Unknown
    }

    fn describe(&self) -> String;
}

mod file;
pub use file::FileSource;

pub mod tty;
pub use tty::TTYSource;

mod probe;
pub use probe::ProbeSource;

mod raw_file;
pub use raw_file::RawFileSource;
