use std::convert::TryInto;

use anyhow::{bail, Context, Result};
use itm_decode::{TimestampedTracePackets, TracePacket};

// TODO Use when trait aliases are stabilized <https://github.com/rust-lang/rust/issues/41517>
// trait Source: Iterator<Item = Result<TimestampedTracePackets>>;

mod file;
pub use file::FileSource;

pub mod tty;
pub use tty::TTYSource;

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
