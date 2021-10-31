use crate::recovery::Metadata;
use crate::sources::{BufferStatus, Source, SourceError};
use crate::TraceData;

use std::fs;
use std::io::BufReader;

/// Something data is deserialized from. Always a file.
pub struct FileSource {
    reader: BufReader<fs::File>,
    metadata: Metadata,
}

impl FileSource {
    pub fn new(fd: fs::File) -> Result<Self, SourceError> {
        let mut reader = BufReader::new(fd);
        let metadata = {
            let mut stream =
                serde_json::Deserializer::from_reader(&mut reader).into_iter::<Metadata>();
            if let Some(Ok(metadata)) = stream.next() {
                metadata
            } else {
                return Err(SourceError::SetupError(
                    "Failed to deserialize metadata header".to_string(),
                ));
            }
        };

        Ok(Self { reader, metadata })
    }

    pub fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }
}

impl Iterator for FileSource {
    type Item = Result<TraceData, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut stream =
            serde_json::Deserializer::from_reader(&mut self.reader).into_iter::<TraceData>();
        match stream.next() {
            Some(Ok(data)) => Some(Ok(data)),
            Some(Err(e)) => Some(Err(SourceError::IterDeserError(e))),
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
