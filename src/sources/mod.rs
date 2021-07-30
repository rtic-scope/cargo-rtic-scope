use anyhow::Result;
use itm_decode::TimestampedTracePackets;

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

pub trait Source: Iterator<Item = Result<TimestampedTracePackets>> {
    fn reset_target(&mut self) -> Result<()>;

    /// Reports the available bytes in the input buffer, if able.
    fn avail_buffer(&self) -> BufferStatus {
        BufferStatus::Unknown
    }
}

mod file;
pub use file::FileSource;

pub mod tty;
pub use tty::TTYSource;

mod probe;
pub use probe::ProbeSource;
