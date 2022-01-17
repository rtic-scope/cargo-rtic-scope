//! Source which reads raw ITM packets from a serial device after
//! properly configuring it. Commonly used if `probe-rs` cannot read the
//! target device.
use crate::manifest::ManifestProperties;
use crate::sources::{BufferStatus, Source, SourceError};
use crate::TraceData;

use std::fs;
use std::os::unix::io::{AsRawFd, RawFd};

use itm::{Decoder, DecoderOptions, Timestamps, TimestampsConfiguration};
use nix::{
    libc,
    unistd::{sysconf, SysconfVar},
};

mod ioctl {
    use super::libc;
    use nix::ioctl_read_bad;

    ioctl_read_bad!(fionread, libc::FIONREAD, libc::c_int);
}

/// Opens and configures the given `device`.
///
/// Effectively mirrors the behavior of
/// ```
/// $ screen /dev/ttyUSB3 <baud rate>
/// ```
/// assuming that `device` is `/dev/ttyUSB3`.
///
/// TODO ensure POSIX compliance, see termios(3)
/// TODO We are currently using line disciple 0. Is that correct?
pub fn configure(device: &str, baud_rate: u32) -> Result<fs::File, SourceError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .open(&device)
        .map_err(SourceError::SetupIOError)?;

    itm::serial::configure(&file, baud_rate).map_err(|e| SourceError::SetupError(e.to_string()))?;
    Ok(file)
}

pub struct TTYSource {
    fd: RawFd,
    decoder: Timestamps<fs::File>,
}

impl TTYSource {
    pub fn new(device: fs::File, opts: &ManifestProperties) -> Self {
        Self {
            fd: device.as_raw_fd(),
            decoder: Decoder::new(device, DecoderOptions { ignore_eof: true }).timestamps(
                TimestampsConfiguration {
                    clock_frequency: opts.tpiu_freq,
                    lts_prescaler: opts.lts_prescaler,
                    expect_malformed: opts.expect_malformed,
                },
            ),
        }
    }
}

impl Iterator for TTYSource {
    type Item = Result<TraceData, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.decoder
            .next()
            .map(|res| res.map_err(SourceError::DecodeError))
    }
}

impl Source for TTYSource {
    fn avail_buffer(&self) -> BufferStatus {
        let avail_bytes = unsafe {
            let mut fionread: libc::c_int = 0;
            if ioctl::fionread(self.fd, &mut fionread).is_err() {
                return BufferStatus::Unknown;
            } else {
                fionread as i64
            }
        };

        if let Ok(Some(page_size)) = sysconf(SysconfVar::PAGE_SIZE) {
            match page_size - avail_bytes {
                n if n < page_size / 4 => BufferStatus::AvailWarn(n, page_size),
                n => BufferStatus::Avail(n),
            }
        } else {
            BufferStatus::Unknown
        }
    }

    fn describe(&self) -> String {
        format!("TTY (fd: {})", self.fd)
    }
}
