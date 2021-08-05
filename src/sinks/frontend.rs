use crate::recovery::Metadata;
use crate::sinks::Sink;
use crate::TraceData;

use std::io::Write;

use anyhow::{Context, Result};
use serde_json;

pub struct FrontendSink {
    socket: std::os::unix::net::UnixStream,
    metadata: Metadata,
}

impl FrontendSink {
    pub fn new(socket: std::os::unix::net::UnixStream, metadata: Metadata) -> Self {
        Self { socket, metadata }
    }
}

impl Sink for FrontendSink {
    fn drain(&mut self, data: TraceData) -> Result<()> {
        let json = match data {
            Ok(packets) => serde_json::to_string(
                &self
                    .metadata
                    .build_event_chunk(packets)
                    .context("Failed to build event chunk")?,
            ),
            Err(malformed) => serde_json::to_string(&malformed),
        }?;
        self.socket
            .write_all(json.as_bytes())
            .context("Failed to drain JSON to frontend")
    }

    fn describe(&self) -> String {
        format!("frontend using socket {:?}", self.socket)
    }
}
