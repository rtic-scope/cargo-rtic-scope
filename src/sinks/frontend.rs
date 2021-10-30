use crate::sinks::{Sink, SinkError};
use crate::TraceData;

use rtic_scope_api as api;
use std::io::Write;

pub struct FrontendSink {
    socket: std::os::unix::net::UnixStream,
}

impl FrontendSink {
    pub fn new(socket: std::os::unix::net::UnixStream) -> Self {
        Self { socket }
    }
}

impl Sink for FrontendSink {
    fn drain(&mut self, _: TraceData, chunk: api::EventChunk) -> Result<(), SinkError> {
        let json = serde_json::to_string(&chunk)?;

        self.socket
            .write_all(json.as_bytes())
            .map_err(SinkError::DrainIOError)
    }

    fn describe(&self) -> String {
        format!("frontend using socket {:?}", self.socket)
    }
}
