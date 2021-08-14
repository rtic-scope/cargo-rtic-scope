use crate::sources::{BufferStatus, Source, SourceError};
use crate::TraceData;

use std::fs;
use std::io::Read;

use itm_decode::{Decoder, DecoderOptions};

/// Something data is deserialized from. Always a file.
pub struct RawFileSource {
    file_name: String,
    bytes: std::io::Bytes<fs::File>,
    decoder: Decoder,
}

impl RawFileSource {
    pub fn new(fd: fs::File) -> Self {
        Self {
            file_name: format!("{:?}", fd),
            bytes: fd.bytes(),
            decoder: Decoder::new(DecoderOptions::default()),
        }
    }
}

impl Iterator for RawFileSource {
    type Item = Result<TraceData, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(b) = self.bytes.next() {
            match b {
                Ok(b) => self.decoder.push(&[b]),
                Err(e) => return Some(Err(SourceError::IterIOError(e))),
            };

            match self.decoder.pull_with_timestamp() {
                None => continue,
                Some(packets) => return Some(Ok(packets)),
            }
        }

        None
    }
}

impl Source for RawFileSource {
    fn avail_buffer(&self) -> BufferStatus {
        BufferStatus::NotApplicable
    }

    fn describe(&self) -> String {
        format!("raw file ({:?})", self.file_name)
    }
}
