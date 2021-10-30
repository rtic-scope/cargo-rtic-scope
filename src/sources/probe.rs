use crate::sources::{Source, SourceError};
use crate::ManifestOptions;
use crate::TraceData;

use itm_decode::{Decoder, DecoderOptions};
use probe_rs::{architecture::arm::SwoConfig, Session};

pub struct ProbeSource {
    session: Session,
    decoder: Decoder,
}

impl ProbeSource {
    pub fn new(mut session: Session, opts: &ManifestOptions) -> Result<Self, SourceError> {
        // Configure probe and target for tracing
        //
        // NOTE(unwrap) --tpiu-freq is a requirement to enter this
        // function.
        let cfg = SwoConfig::new(opts.tpiu_freq.expect("no TPIU frequency"))
            .set_baud(opts.tpiu_baud.expect("no TPIU baud rate"))
            .set_continuous_formatting(false);
        session
            .setup_swv(0, &cfg)
            .map_err(SourceError::SetupProbeError)?;

        // Enable exception tracing
        // {
        //     let mut core = session.core(0)?;
        //     let components = session.get_arm_components()?;
        //     let mut dwt = Dwt::new(&mut core, find_component(components, PeripheralType::Dwt)?);
        //     dwt.enable_exception_trace()?;
        // }

        Ok(Self {
            session,
            decoder: Decoder::new(DecoderOptions::default()),
        })
    }
}

impl Iterator for ProbeSource {
    type Item = Result<TraceData, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        // XXX we can get stuck here if read_swo returns data such that
        // the Ok(None) is always pulled from the decoder.
        loop {
            match self.session.read_swo() {
                Ok(bytes) => {
                    self.decoder.push(&bytes);

                    match self.decoder.pull_with_timestamp() {
                        None => continue,
                        Some(packets) => return Some(Ok(packets)),
                    }
                }
                Err(e) => return Some(Err(SourceError::IterProbeError(e))),
            }
        }
    }
}

impl Source for ProbeSource {
    fn reset_target(&mut self) -> Result<(), SourceError> {
        let mut core = self.session.core(0).map_err(SourceError::ResetError)?;
        core.reset().map_err(SourceError::ResetError)?;

        Ok(())
    }

    fn describe(&self) -> String {
        format!("probe (attached to {})", self.session.target().name)
    }
}
