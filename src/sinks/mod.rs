use crate::TraceData;

use anyhow::Result;

pub mod file;
pub use file::FileSink;

mod frontend;
pub use frontend::FrontendSink;

pub trait Sink {
    fn drain(&mut self, data: TraceData) -> Result<()>;
    fn describe(&self) -> String;
}
