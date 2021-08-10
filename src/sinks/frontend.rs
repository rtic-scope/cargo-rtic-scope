use crate::sinks::{Sink, SinkError};
use crate::TraceData;

use rtic_scope_api as api;
use std::io::Write;

use serde_json;

pub struct FrontendSink {
    socket: std::os::unix::net::UnixStream,
}

impl FrontendSink {
    pub fn new(socket: std::os::unix::net::UnixStream) -> Self {
        Self { socket }
    }
}

impl Sink for FrontendSink {
    fn drain(&mut self, data: TraceData, chunk: Option<api::EventChunk>) -> Result<(), SinkError> {
        let json = match (data, chunk) {
            (Err(malformed), None) => serde_json::to_string(&malformed)?,
            (_, Some(chunk)) => serde_json::to_string(&chunk)?,
            _ => unreachable!(),
        };

        self.socket
            .write_all(json.as_bytes())
            .map_err(|e| SinkError::DrainIOError(e))
    }

    fn describe(&self) -> String {
        format!("frontend using socket {:?}", self.socket)
    }
}
