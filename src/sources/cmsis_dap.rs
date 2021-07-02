use crate::sources::Source;

use anyhow::{anyhow, Context, Result};
use itm_decode::{Decoder, DecoderState, TimestampedTracePackets};
use probe_rs::{architecture::arm::SwoConfig, Session};

pub struct DAPSource {
    session: Session,
    decoder: Decoder,
}

impl DAPSource {
    pub fn new(mut session: Session) -> Result<Self> {
        // Configure probe and target for tracing
        //
        // TODO: make this configurable
        let cfg = SwoConfig::new(16_000_000)
            .set_baud(2_000_000)
            .set_continuous_formatting(false);
        session.setup_swv(0, &cfg)?;

        Ok(Self {
            session,
            decoder: Decoder::new(),
        })
    }
}

impl Iterator for DAPSource {
    type Item = Result<TimestampedTracePackets>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Ok(bytes) = self.session.read_swo() {
            self.decoder.push(bytes);

            match self.decoder.pull_with_timestamp() {
                Ok(None) => continue,
                Ok(Some(packets)) => return Some(Ok(packets)),
                Err(e) => {
                    self.decoder.state = DecoderState::Header;
                    return Some(Err(anyhow!(
                        "Failed to decode packets from CMSIS-DAP device: {:?}",
                        e
                    )));
                }
            }
        }

        None
    }
}

impl Source for DAPSource {
    fn reset_target(&mut self) -> Result<()> {
        let mut core = self.session.core(0)?;
        core.reset().context("Unable to reset target")?;

        Ok(())
    }
}
