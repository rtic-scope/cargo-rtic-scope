use crate::recovery::TaskResolveMaps;

use std::fs;
use std::io::{BufReader, Read};

use anyhow::{anyhow, bail, Context, Result};
use chrono::prelude::*;
use itm_decode::{Decoder, DecoderState, TimestampedTracePackets};
use serde_json;

// TODO Use when trait aliases are stabilized <https://github.com/rust-lang/rust/issues/41517>
// trait Source: Iterator<Item = Result<TimestampedTracePackets>>;

/// Something data is deserialized from. Always a file.
pub struct FileSource {
    reader: BufReader<fs::File>,
    maps: TaskResolveMaps,
    _timestamp: chrono::DateTime<Local>,
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
        use serde_json::Value;

        let mut reader = BufReader::new(fd);

        let mut read_metadata = || {
            let mut stream =
                serde_json::Deserializer::from_reader(&mut reader).into_iter::<Value>();
            if let Some(Ok(Value::Array(metadata))) = stream.next() {
                let maps: TaskResolveMaps = serde_json::from_value(
                    metadata
                        .iter()
                        .nth(0) // XXX magic
                        .context("Resolve map object missing")?
                        .clone(),
                )
                .context("Failed to deserialize resolve map object")?;

                let timestamp: chrono::DateTime<Local> = serde_json::from_value(
                    metadata
                        .iter()
                        .nth(1) // XXX magic
                        .context("Reset timestamp string missing")?
                        .clone(),
                )
                .context("Failed to deserialize reset timestamp string")?;

                Ok((maps, timestamp))
            } else {
                bail!("Expected metadata header missing from trace file");
            }
        };
        let (maps, timestamp) = read_metadata().context("Failed to deserialize metadata header")?;

        Ok(Self {
            reader,
            maps,
            _timestamp: timestamp,
        })
    }

    pub fn copy_maps(&self) -> TaskResolveMaps {
        self.maps.clone()
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
