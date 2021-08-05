use crate::sources::Source;
use crate::TPIUOptions;
use crate::TraceData;

use anyhow::{anyhow, Context, Result};
use itm_decode::{Decoder, DecoderOptions};
use probe_rs::{architecture::arm::SwoConfig, Session};

pub struct ProbeSource {
    session: Session,
    decoder: Decoder,
}

impl ProbeSource {
    pub fn new(mut session: Session, opts: &TPIUOptions) -> Result<Self> {
        // Configure probe and target for tracing
        //
        // NOTE(unwrap) --tpiu-freq is a requirement to enter this
        // function.
        let cfg = SwoConfig::new(opts.clk_freq)
            .set_baud(opts.baud_rate)
            .set_continuous_formatting(false);
        session.setup_swv(0, &cfg)?;

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
    type Item = Result<TraceData>;

    fn next(&mut self) -> Option<Self::Item> {
        // XXX we can get stuck here if read_swo returns data such that
        // the Ok(None) is always pulled from the decoder.
        loop {
            match self.session.read_swo() {
                Ok(bytes) => {
                    self.decoder.push(&bytes);

                    match self.decoder.pull_with_timestamp() {
                        Ok(None) => continue,
                        Ok(Some(packets)) => return Some(Ok(Ok(packets))),
                        Err(malformed) => return Some(Ok(Err(malformed))),
                    }
                }
                Err(e) => return Some(Err(anyhow!("Failed to read SWO bytes: {:?}", e))),
            }
        }
    }
}

impl Source for ProbeSource {
    fn reset_target(&mut self) -> Result<()> {
        let mut core = self.session.core(0)?;
        core.reset().context("Unable to reset target")?;

        Ok(())
    }

    fn describe(&self) -> String {
        format!("probe (attached to {})", self.session.target().name)
    }
}
