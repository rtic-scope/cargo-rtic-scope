use anyhow::Result;
use itm_decode::TimestampedTracePackets;

pub mod file;
pub use file::FileSink;

mod frontend;
pub use frontend::FrontendSink;

pub trait Sink {
    fn drain(&mut self, packets: TimestampedTracePackets) -> Result<()>;
    fn describe(&self) -> String;
}
