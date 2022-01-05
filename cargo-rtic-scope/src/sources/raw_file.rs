//! Source which reads raw ITM packets from a file.
use crate::manifest::ManifestProperties;
use crate::sources::{BufferStatus, Source, SourceError};
use crate::TraceData;

use std::fs;

use itm::{Decoder, DecoderOptions, Timestamps, TimestampsConfiguration};

/// Something data is deserialized from. Always a file.
pub struct RawFileSource {
    file_name: String,
    decoder: Timestamps<std::fs::File>,
}

impl RawFileSource {
    pub fn new(file: fs::File, opts: &ManifestProperties) -> Self {
        Self {
            file_name: format!("{:?}", file),
            decoder: Decoder::new(file, DecoderOptions { ignore_eof: true }).timestamps(
                TimestampsConfiguration {
                    clock_frequency: opts.tpiu_freq,
                    lts_prescaler: opts.lts_prescaler,
                    expect_malformed: opts.expect_malformed,
                },
            ),
        }
    }
}

impl Iterator for RawFileSource {
    type Item = Result<TraceData, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.decoder
            .next()
            .map(|res| res.map_err(SourceError::DecodeError))
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
