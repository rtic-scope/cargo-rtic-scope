use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use cargo_metadata::Artifact;
use chrono::prelude::*;
use git2::{DescribeFormatOptions, DescribeOptions, Repository};
use itm_decode::{Decoder, DecoderState, TimestampedTracePackets};
use serde_json;

pub struct Sink {
    file: File,
    decoder: Decoder,
    timestamp: Option<DateTime<chrono::Local>>,
}

impl Sink {
    pub fn generate(
        artifact: &Artifact,
        trace_dir: &PathBuf,
        remove_prev_traces: bool,
    ) -> Result<Self> {
        if remove_prev_traces {
            // TODO only remove .trace files
            fs::remove_dir_all(trace_dir)?;
        }

        // Try to find the git root recursively backwards from the
        // artifact target source path.
        let repo = {
            let mut path = artifact.target.src_path.clone();
            #[allow(unused_assignments)]
            let mut repo = None;
            loop {
                repo = Repository::open(&path).ok();
                if repo.is_some() {
                    break;
                }

                if !path.pop() {
                    bail!("Failed to find a git root");
                }
            }

            repo.unwrap() // safe
        };

        // generate a short descroption on the format
        // "blinky-gbaadf00-dirty-2021-06-16T17:13:16.trace"
        let git_shortdesc = repo
            .describe(&DescribeOptions::new().show_commit_oid_as_fallback(true))?
            .format(Some(
                &DescribeFormatOptions::new()
                    .abbreviated_size(7)
                    .dirty_suffix("-dirty"),
            ))?;
        let date = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        let path = trace_dir.join(format!(
            "{}-g{}-{}.trace",
            artifact.target.name, git_shortdesc, date
        ));

        fs::create_dir_all(trace_dir)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;

        Ok(Sink {
            file,
            decoder: Decoder::new(),
            timestamp: None,
        })
    }

    pub fn sample_reset_timestamp(&mut self) -> Result<()> {
        self.timestamp = Some(Local::now());

        // serialize timestamp to file
        //
        // TODO Make another thread do this work. We want the time
        // between this timestamp and the target reset to be
        // predictable.
        let json = serde_json::to_string(&self.timestamp.unwrap())?;
        self.file.write_all(json.as_bytes())?;

        Ok(())
    }

    pub fn push(&mut self, byte: u8) -> Result<()> {
        // TODO shared ring-buffer between threads so that main thread
        // spends as much time as possible reading serial.

        self.decoder.push([byte].to_vec());

        // decode available packets and serialize to file
        loop {
            // TODO Rewrite itm-decode so that it is possible to check
            // if a packet can be decoded. If one can, we timestamp when
            // we have received a full new packet.
            match self.decoder.pull_with_timestamp() {
                Ok(None) => break,
                Ok(Some(packets)) => {
                    println!("{:?}", packets);
                    let json = serde_json::to_string(&packets)?;
                    self.file.write_all(json.as_bytes())?;
                }
                Err(e) => {
                    println!("Error: {:?}", e);
                    self.decoder.state = DecoderState::Header;
                }
            }
        }

        Ok(())
    }
}
