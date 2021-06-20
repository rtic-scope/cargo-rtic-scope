use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

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
    const TRACE_FILE_EXT: &'static str = ".trace";

    pub fn generate(
        artifact: &Artifact,
        trace_dir: &PathBuf,
        remove_prev_traces: bool,
    ) -> Result<Self> {
        if remove_prev_traces {
            for trace in Self::find_trace_files(trace_dir)? {
                fs::remove_file(trace).context("Failed to remove previous trace file")?;
            }
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
        let file = trace_dir.join(format!(
            "{}-g{}-{}.{}",
            artifact.target.name,
            git_shortdesc,
            date,
            Self::TRACE_FILE_EXT,
        ));

        fs::create_dir_all(trace_dir)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)?;

        Ok(Sink {
            file,
            decoder: Decoder::new(),
        })
    }

    /// ls `*.trace` in given path.
    fn find_trace_files(path: &Path) -> Result<impl Iterator<Item = PathBuf>> {
        Ok(fs::read_dir(path)
            .context("Failed to read trace directory")?
            // we only care about files we can access
            .map(|entry| entry.unwrap())
            // grep *.trace
            .filter_map(|entry| {
                if entry.file_type().unwrap().is_file()
                    && entry
                        .file_name()
                        .to_str()
                        .unwrap()
                        .ends_with(Self::TRACE_FILE_EXT)
                {
                    Some(entry.path())
                } else {
                    None
                }
            }))
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
