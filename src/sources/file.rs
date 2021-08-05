use crate::recovery::Metadata;
use crate::sources::{BufferStatus, Source};
use crate::TraceData;

use std::fs;
use std::io::BufReader;

use anyhow::{anyhow, bail, Result};
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
    type Item = Result<TraceData>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut stream =
            serde_json::Deserializer::from_reader(&mut self.reader).into_iter::<TraceData>();
        match stream.next() {
            Some(Ok(data)) => Some(Ok(data)),
            Some(Err(e)) => Some(Err(anyhow!("Failed to deserialize data: {:?}", e))),
            None => None,
        }
    }
}

impl Source for FileSource {
    fn avail_buffer(&self) -> BufferStatus {
        BufferStatus::NotApplicable
    }

    fn describe(&self) -> String {
        format!("file ({:?})", self.reader.get_ref())
    }
}
