use crate::recovery::{Metadata, TaskResolveMaps};
use crate::sinks::{Sink, SinkError};
use crate::TraceData;
use std::fs;

use std::io::Write;
use std::path::PathBuf;

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
    ) -> Result<Self, SinkError> {
        if remove_prev_traces {
            if let Ok(traces) = find_trace_files(trace_dir.to_path_buf()) {
                for trace in traces {
                    fs::remove_file(trace).map_err(|e| {
                        SinkError::SetupIOError(
                            Some("Failed to remove previous trace file".to_string()),
                            e,
                        )
                    })?;
                }
            }
        }

        // generate a short descroption on the format
        // "blinky-gbaadf00-dirty-2021-06-16T17:13:16.trace"
        let repo = find_git_repo(artifact.target.src_path.clone())?;
        let git_shortdesc = repo
            .describe(&DescribeOptions::new().show_commit_oid_as_fallback(true))
            .map_err(|e| SinkError::GitError(e))?
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

        fs::create_dir_all(trace_dir).map_err(|e| {
            SinkError::SetupIOError(
                Some(format!(
                    "Failed to create output trace directory {}",
                    trace_dir.display()
                )),
                e,
            )
        })?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .map_err(|e| {
                SinkError::SetupIOError(
                    Some(format!(
                        "Failed to create output trace file {}",
                        file.display()
                    )),
                    e,
                )
            })?;

        Ok(Self { file })
    }

    /// Initializes the sink with metadata: task resolve maps and target
    /// reset timestamp.
    pub fn init<F>(
        &mut self,
        maps: TaskResolveMaps,
        comment: Option<String>,
        reset_fun: F,
    ) -> Result<Metadata, SinkError>
    where
        F: FnOnce() -> Result<u32, crate::sources::SourceError>,
    {
        let ts = Local::now();
        let freq = reset_fun()?;

        // Create a trace file header with metadata (maps, reset
        // timestamp, trace clock frequency). Any bytes after this
        // sequence refers to trace packets.
        let metadata = Metadata::new(maps, ts, freq, comment);
        {
            let json = serde_json::to_string(&metadata).map_err(|e| SinkError::DrainSerError(e))?;
            self.file.write_all(json.as_bytes())
        }
        .map_err(|e| SinkError::DrainIOError(e))?;

        Ok(metadata)
    }
}

impl Sink for FileSink {
    fn drain(&mut self, data: TraceData) -> Result<(), SinkError> {
        let json = serde_json::to_string(&data).map_err(|e| SinkError::DrainSerError(e))?;
        self.file
            .write_all(json.as_bytes())
            .map_err(|e| SinkError::DrainIOError(e))?;

        Ok(())
    }

    fn describe(&self) -> String {
        format!("file sink: {:?}", self.file)
    }
}

/// Attempts to find a git repository starting from the given path
/// and walking upwards until / is hit.
fn find_git_repo(mut path: PathBuf) -> Result<Repository, SinkError> {
    let start_path = path.clone();
    loop {
        match Repository::open(&path) {
            Ok(repo) => return Ok(repo),
            Err(_) => {
                if path.pop() {
                    continue;
                }

                return Err(SinkError::NoGitRoot(start_path));
            }
        }
    }
}

/// ls `*.trace` in given path.
// TODO move to Source::file?
pub fn find_trace_files(path: PathBuf) -> Result<impl Iterator<Item = PathBuf>, SinkError> {
    Ok(fs::read_dir(path)
        .map_err(|e| {
            SinkError::SetupIOError(Some("Failed to read trace directory".to_string()), e)
        })?
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
