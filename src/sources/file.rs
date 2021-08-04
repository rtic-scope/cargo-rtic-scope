use crate::recovery::Metadata;
use crate::sources::{BufferStatus, Source};

use std::fs;
use std::io::BufReader;

use anyhow::{bail, Context, Result};
use itm_decode::TimestampedTracePackets;
use serde_json;

/// Something data is deserialized from. Always a file.
pub struct FileSource {
    reader: BufReader<fs::File>,
    metadata: Metadata,
}

impl FileSource {
    pub fn new(fd: fs::File) -> Result<Self> {
        let mut reader = BufReader::new(fd);
        let metadata = {
            let mut stream =
                serde_json::Deserializer::from_reader(&mut reader).into_iter::<Metadata>();
            if let Some(Ok(metadata)) = stream.next() {
                metadata
            } else {
                bail!("Failed to deserialize metadata header");
            }
        };

        Ok(Self { reader, metadata })
    }

    pub fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }
}

impl Iterator for FileSource {
    type Item = Result<TimestampedTracePackets>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut stream = serde_json::Deserializer::from_reader(&mut self.reader)
            .into_iter::<TimestampedTracePackets>();
        match stream.next() {
            Some(Ok(packets)) => Some(Ok(packets)),
            Some(e) => Some(e.context("Failed to deserialize packet from trace file")),
            None => None,
        }
    }
}

impl Source for FileSource {
    fn reset_target(&mut self) -> Result<()> {
        // not connected to a target
        Ok(())
    }

    fn avail_buffer(&self) -> BufferStatus {
        BufferStatus::NotApplicable
    }

    fn describe(&self) -> String {
        format!("file ({:?})", self.reader.get_ref())
    }
}
