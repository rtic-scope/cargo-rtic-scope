use std::convert::TryInto;

use anyhow::{bail, Context, Result};
use itm_decode::{TimestampedTracePackets, TracePacket};

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

pub fn wait_for_trace_clk_freq(
    mut source: impl Iterator<Item = Result<TimestampedTracePackets>>,
) -> Result<u32> {
    while let Some(packets) = source.next() {
        let packets = packets.context("Failed to read trace packets from source")?;

        for packet in packets.packets {
            if let TracePacket::DataTraceValue {
                access_type, value, ..
            } = packet
            {
                if access_type == itm_decode::MemoryAccessType::Write
                    && value.len() == 4
                    && value.iter().any(|b| *b != 0)
                {
                    // NOTE(unwrap) len already checked
                    return Ok(u32::from_le_bytes(value.try_into().unwrap()));
                }
            }
        }
    }

    bail!("EOF reached prematurely");
}
