use crate::manifest::ManifestProperties;
use crate::sources::{Source, SourceError};
use crate::TraceData;

use itm::{Decoder, DecoderOptions, Timestamps, TimestampsConfiguration};
use probe_rs::{
    architecture::arm::{SwoConfig, SwoReader},
    Session,
};

pub struct ProbeSource<'a> {
    decoder: Timestamps<SwoReader<'a>>,
    target_name: String,
}

impl<'a> ProbeSource<'a> {
    pub fn new(session: &'a mut Session, opts: &ManifestProperties) -> Result<Self, SourceError> {
        // Configure probe and target for tracing
        let cfg = SwoConfig::new(opts.tpiu_freq)
            .set_baud(opts.tpiu_baud)
            .set_continuous_formatting(false);
        session
            .setup_swv(0, &cfg)
            .map_err(SourceError::ProbeError)?;

        Ok(Self {
            target_name: session.target().name.clone(),
            decoder: Decoder::new(session.swo_reader()?, DecoderOptions { ignore_eof: true })
                .timestamps(TimestampsConfiguration {
                    clock_frequency: opts.tpiu_freq,
                    lts_prescaler: opts.lts_prescaler,
                    expect_malformed: opts.expect_malformed,
                }),
        })
    }
}

impl<'a> Iterator for ProbeSource<'a> {
    type Item = Result<TraceData, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.decoder.next() {
            None => None,
            Some(res) => Some(res.map_err(SourceError::DecodeError)),
        }
    }
}

impl<'a> Source for ProbeSource<'a> {
    fn describe(&self) -> String {
        format!("probe (attached to {})", self.target_name)
    }
}
