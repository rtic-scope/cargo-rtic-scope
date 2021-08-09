use crate::recovery::Metadata;
use crate::sinks::{Sink, SinkError};
use crate::TraceData;

use rtic_scope_api as api;
use std::io::Write;

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
    fn drain(&mut self, data: TraceData) -> Result<(), SinkError> {
        // serialize to JSON, and record any unmappable packets
        let mut unmappable = vec![];
        let json = match data {
            Ok(packets) => {
                let chunk = self.metadata.build_event_chunk(packets);
                unmappable.append(
                    &mut chunk
                        .events
                        .iter()
                        .filter_map(|e| {
                            if let api::EventType::Unknown(e, w) = e {
                                Some((e.to_owned(), w.to_owned()))
                            } else {
                                None
                            }
                        })
                        .collect(),
                );

                serde_json::to_string(&chunk)?
            }
            Err(malformed) => serde_json::to_string(&malformed)?,
        };
        // XXX: unmappable packets are not reported if I/O fails
        self.socket
            .write_all(json.as_bytes())
            .map_err(|e| SinkError::DrainIOError(e))?;

        if !unmappable.is_empty() {
            Err(SinkError::ResolveError(unmappable))
        } else {
            Ok(())
        }
    }

    fn describe(&self) -> String {
        format!("frontend using socket {:?}", self.socket)
    }
}
