use crate::recovery::Metadata;
use crate::sinks::Sink;

use std::io::Write;

use anyhow::{Context, Result};
use itm_decode::TimestampedTracePackets;
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
    fn drain(&mut self, packets: TimestampedTracePackets) -> Result<()> {
        match self.metadata.resolve_event_chunk(packets.clone()) {
            Ok(packets) => {
                let json = serde_json::to_string(&packets)?;
                self.socket.write_all(json.as_bytes())
            }
            .context("Failed to forward api::EventChunk to frontend"),
            Err(e) => {
                eprintln!(
                    "Failed to resolve chunk from {:?}. Reason: {}. Ignoring...",
                    packets, e
                );
                Ok(())
            }
        }
    }

    fn describe(&self) -> String {
        format!("frontend using socket {:?}", self.socket)
    }
}
