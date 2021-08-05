use crate::recovery::{Metadata, TaskResolveMaps};
use crate::sinks::Sink;
use crate::TraceData;
use std::fs;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use cargo_metadata::Artifact;
use chrono::prelude::*;
use git2::{DescribeFormatOptions, DescribeOptions, Repository};
use serde_json;

const TRACE_FILE_EXT: &'static str = ".trace";

pub struct FileSink {
    file: fs::File,
}

impl FileSink {
    pub fn generate_trace_file(
        artifact: &Artifact,
        trace_dir: &PathBuf,
        remove_prev_traces: bool,
    ) -> Result<Self> {
        if remove_prev_traces {
            if let Ok(traces) = find_trace_files(trace_dir.to_path_buf()) {
                for trace in traces {
                    fs::remove_file(trace).context("Failed to remove previous trace file")?;
                }
            }
        }

        // generate a short descroption on the format
        // "blinky-gbaadf00-dirty-2021-06-16T17:13:16.trace"
        let repo = find_git_repo(artifact.target.src_path.clone())?;
        let git_shortdesc = repo
            .describe(&DescribeOptions::new().show_commit_oid_as_fallback(true))?
            .format(Some(
                &DescribeFormatOptions::new()
                    .abbreviated_size(7)
                    .dirty_suffix("-dirty"),
            ))?;
        let date = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        let file = trace_dir.join(format!(
            "{}-g{}-{}{}",
            artifact.target.name, git_shortdesc, date, TRACE_FILE_EXT,
        ));

        fs::create_dir_all(trace_dir)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)?;

        Ok(Self { file })
    }

    /// Initializes the sink with metadata: task resolve maps and target
    /// reset timestamp.
    pub fn init<F>(
        &mut self,
        maps: TaskResolveMaps,
        comment: Option<String>,
        reset_fun: F,
    ) -> Result<Metadata>
    where
        F: FnOnce() -> Result<u32>,
    {
        let ts = Local::now();
        let freq = reset_fun()?;

        // Create a trace file header with metadata (maps, reset
        // timestamp, trace clock frequency). Any bytes after this
        // sequence refers to trace packets.
        let metadata = Metadata::new(maps, ts, freq, comment);
        {
            let json = serde_json::to_string(&metadata)?;
            self.file.write_all(json.as_bytes())
        }
        .context("Failed to write metadata do file")?;

        Ok(metadata)
    }
}

impl Sink for FileSink {
    fn drain(&mut self, data: TraceData) -> Result<()> {
        let json = serde_json::to_string(&data)?;
        self.file.write_all(json.as_bytes())?;

        Ok(())
    }

    fn describe(&self) -> String {
        format!("file sink: {:?}", self.file)
    }
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

/// ls `*.trace` in given path.
pub fn find_trace_files(path: PathBuf) -> Result<impl Iterator<Item = PathBuf>> {
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
                    .ends_with(TRACE_FILE_EXT)
            {
                Some(entry.path())
            } else {
                None
            }
        }))
}
