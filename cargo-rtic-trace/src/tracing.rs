use std::fs::{self, File};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use cargo_metadata::Artifact;
use chrono::prelude::Local as Time;
use git2::{DescribeFormatOptions, DescribeOptions, Repository};
use itm_decode::{Decoder, DecoderState};

pub struct Sink {
    file: File,
    decoder: Decoder,
}

impl Sink {
    pub fn generate(artifact: &Artifact, trace_dir: &PathBuf) -> Result<Self> {
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
        let date = Time::now().format("%Y-%m-%dT%H:%M:%S").to_string();
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

    pub fn timestamp(&mut self) -> Result<()> {
        // TODO Add timestamp header to file that marks start of
        // recording.

        Ok(())
    }

    pub fn push(&mut self, byte: u8) -> Result<()> {
        // TODO shared ring-buffer between threads so that main thread
        // spends as much time as possible reading serial.

        self.decoder.push([byte].to_vec());

        // TODO pull all decoded packets, create some struct that holds
        // these with a timestamp, or just add that struct to itm_decode
        // itself.
        loop {
            match self.decoder.pull() {
                Ok(None) => break,
                Ok(Some(packet)) => println!("{:?}", packet),
                Err(e) => {
                    println!("Error: {:?}", e);
                    self.decoder.state = DecoderState::Header;
                }
            }
        }

        // TODO serialize struct and write to file

        // TODO go back to ring-buffer and continue ad infinitum.

        Ok(())
    }
}
