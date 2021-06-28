use crate::recovery::Metadata;

use std::fs;
use std::io::{BufReader, Read};

use anyhow::{anyhow, bail, Context, Result};
use itm_decode::{Decoder, DecoderState, TimestampedTracePackets};
use serde_json;

// TODO Use when trait aliases are stabilized <https://github.com/rust-lang/rust/issues/41517>
// trait Source: Iterator<Item = Result<TimestampedTracePackets>>;

/// Something data is deserialized from. Always a file.
pub struct FileSource {
    reader: BufReader<fs::File>,
    metadata: Metadata,
}

pub struct TtySource {
    bytes: std::io::Bytes<fs::File>,
    decoder: Decoder,
}

impl TtySource {
    pub fn new(device: fs::File) -> Self {
        Self {
            bytes: device.bytes(),
            decoder: Decoder::new(),
        }
    }
}

impl Iterator for TtySource {
    type Item = Result<TimestampedTracePackets>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(b) = self.bytes.next() {
            match b {
                Ok(b) => self.decoder.push([b].to_vec()),
                Err(e) => {
                    return Some(Err(anyhow!(
                        "Failed to read byte from serial device: {:?}",
                        e
                    )))
                }
            };

            match self.decoder.pull_with_timestamp() {
                Ok(None) => continue,
                Ok(Some(packets)) => return Some(Ok(packets)),
                Err(e) => {
                    self.decoder.state = DecoderState::Header;
                    return Some(Err(anyhow!(
                        "Failed to decode packets from serial: {:?}",
                        e
                    )));
                }
            }
        }

        None
    }
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
