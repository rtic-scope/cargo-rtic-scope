use crate::sources::Source;
use crate::TPIUOptions;

use anyhow::{anyhow, Context, Result};
use itm_decode::{Decoder, DecoderState, TimestampedTracePackets};
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
        let cfg = SwoConfig::new(opts.clk_freq.unwrap())
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
            decoder: Decoder::new(),
        })
    }
}

impl Iterator for ProbeSource {
    type Item = Result<TimestampedTracePackets>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Ok(bytes) = self.session.read_swo() {
            self.decoder.push(bytes);

            match self.decoder.pull_with_timestamp() {
                Ok(None) => continue,
                Ok(Some(packets)) => return Some(Ok(packets)),
                Err(e) => {
                    self.decoder.state = DecoderState::Header;
                    return Some(Err(anyhow!("Failed to decode packets from probe: {:?}", e)));
                }
            }
        }

        None
    }
}

impl Source for ProbeSource {
    fn reset_target(&mut self) -> Result<()> {
        let mut core = self.session.core(0)?;
        core.reset().context("Unable to reset target")?;

        Ok(())
    }
}
