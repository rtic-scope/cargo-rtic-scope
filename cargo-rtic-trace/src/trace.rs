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

        // generate a short descroption on the format
        // "blinky-gbaadf00-dirty-2021-06-16T17:13:16.trace"
        let repo = Self::find_git_repo(artifact.target.src_path.clone())?;
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
        })
    }

    /// Attempts to find a git repository starting from the given path
    /// and walking upwards until / is hit.
    fn find_git_repo(mut path: PathBuf) -> Result<Repository> {
        loop {
            match Repository::open(&path) {
                Ok(repo) => return Ok(repo),
                Err(_) => {
                    if path.pop() {
                        continue;
                    }

                    bail!("Failed to find git repo root");
                }
            }
        }
    }

    /// Samples a timestamp after which the target is immidiately reset.
    pub fn timestamp_reset<F>(&mut self, reset_fun: F) -> Result<()>
    where
        F: FnOnce() -> Result<()>,
    {
        let ts = Local::now();

        reset_fun().context("Failed to reset target")?;

        let json = serde_json::to_string(&ts)?;
        self.file.write_all(json.as_bytes())?;

        Ok(())
    }

    pub fn push(&mut self, byte: u8) -> Result<()> {
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
