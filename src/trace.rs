use crate::parse::TaskResolveMaps;

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use cargo_metadata::Artifact;
use chrono::prelude::*;
use git2::{DescribeFormatOptions, DescribeOptions, Repository};
use itm_decode::{Decoder, DecoderState, TimestampedTracePackets};
use serde::ser::{SerializeSeq, Serializer};
use serde_json;

const TRACE_FILE_EXT: &'static str = ".trace";

// TODO Use when trait aliases are stabilized <https://github.com/rust-lang/rust/issues/41517>
// trait Source: Iterator<Item = Result<TimestampedTracePackets>>;

/// Something data is deserialized from. Always a file.
pub struct FileSource {
    reader: BufReader<fs::File>,
    maps: TaskResolveMaps,
    _timestamp: chrono::DateTime<Local>,
}

pub struct TtySource {
    bytes: std::io::Bytes<fs::File>,
    decoder: Decoder,
}

impl TtySource {
    pub fn new(device: fs::File) -> Self {
        Self {
            bytes: device.bytes(),
            decoder: Decoder::new(),
        }
    }
}

impl Iterator for TtySource {
    type Item = Result<TimestampedTracePackets>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(b) = self.bytes.next() {
            match b {
                Ok(b) => self.decoder.push([b].to_vec()),
                Err(e) => {
                    return Some(Err(anyhow!(
                        "Failed to read byte from serial device: {:?}",
                        e
                    )))
                }
            };

            match self.decoder.pull_with_timestamp() {
                Ok(None) => continue,
                Ok(Some(packets)) => return Some(Ok(packets)),
                Err(e) => {
                    self.decoder.state = DecoderState::Header;
                    return Some(Err(anyhow!(
                        "Failed to decode packets from serial: {:?}",
                        e
                    )));
                }
            }
        }

        None
    }
}

impl FileSource {
    pub fn new(fd: fs::File) -> Result<Self> {
        use serde_json::Value;

        let mut reader = BufReader::new(fd);

        let mut read_metadata = || {
            let mut stream =
                serde_json::Deserializer::from_reader(&mut reader).into_iter::<Value>();
            if let Some(Ok(Value::Array(metadata))) = stream.next() {
                let maps: TaskResolveMaps = serde_json::from_value(
                    metadata
                        .iter()
                        .nth(0) // XXX magic
                        .context("Resolve map object missing")?
                        .clone(),
                )
                .context("Failed to deserialize resolve map object")?;

                let timestamp: chrono::DateTime<Local> = serde_json::from_value(
                    metadata
                        .iter()
                        .nth(1) // XXX magic
                        .context("Reset timestamp string missing")?
                        .clone(),
                )
                .context("Failed to deserialize reset timestamp string")?;

                Ok((maps, timestamp))
            } else {
                bail!("Expected metadata header missing from trace file");
            }
        };
        let (maps, timestamp) = read_metadata().context("Failed to deserialize metadata header")?;

        Ok(Self {
            reader,
            maps,
            _timestamp: timestamp,
        })
    }

    pub fn copy_maps(&self) -> TaskResolveMaps {
        self.maps.clone()
    }
}

impl Iterator for FileSource {
    type Item = Result<TimestampedTracePackets>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut stream = serde_json::Deserializer::from_reader(&mut self.reader)
            .into_iter::<TimestampedTracePackets>();
        match stream.next() {
            Some(Ok(packets)) => Some(Ok(packets)),
            Some(e) => Some(e.context("Failed to deserialize packet from trace file")),
            None => None,
        }
    }
}

pub trait Sink {
    fn drain(&mut self, packets: TimestampedTracePackets) -> Result<()>;
    fn describe(&self) -> String;
}

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
            for trace in find_trace_files(trace_dir)? {
                fs::remove_file(trace).context("Failed to remove previous trace file")?;
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
    pub fn init<F>(&mut self, maps: &TaskResolveMaps, reset_fun: F) -> Result<()>
    where
        F: FnOnce() -> Result<()>,
    {
        // Create a trace file header with metadata (maps, reset
        // timestamp). Any bytes after this sequence refers to trace
        // packets.
        //
        // A trace file will then contain: [maps, timestamp], [packets,
        // ...]
        let mut ser = serde_json::Serializer::new(&mut self.file);
        let mut seq = ser.serialize_seq(Some(2))?;
        {
            seq.serialize_element(maps)?;

            let ts = Local::now();
            reset_fun().context("Failed to reset target")?;

            seq.serialize_element(&ts)?;
            seq.end()?;
        }

        Ok(())
    }
}

impl Sink for FileSink {
    fn drain(&mut self, packets: TimestampedTracePackets) -> Result<()> {
        let json = serde_json::to_string(&packets)?;
        self.file.write_all(json.as_bytes())?;

        Ok(())
    }

    fn describe(&self) -> String {
        format!("file output: {:?}", self.file)
    }
}

pub struct FrontendSink {
    socket: std::os::unix::net::UnixStream,
    maps: TaskResolveMaps,
}

impl FrontendSink {
    pub fn new(socket: std::os::unix::net::UnixStream, maps: TaskResolveMaps) -> Self {
        Self { socket, maps }
    }
}

impl Sink for FrontendSink {
    fn drain(&mut self, packets: TimestampedTracePackets) -> Result<()> {
        match self.maps.resolve_tasks(packets.clone()) {
            Ok(packets) => {
                let json = serde_json::to_string(&packets)?;
                self.socket.write_all(json.as_bytes())?;

                Ok(())
            }
            // TODO move this handling up to main
            Err(e) => {
                eprintln!(
                    "Failed to translate tasks for packets: {:?}. Reason: {}. Ignoring...",
                    packets, e
                );
                Ok(())
            }
        }
    }

    fn describe(&self) -> String {
        format!("frontend using socket {:?}", self.socket)
    }
}

/// ls `*.trace` in given path.
pub fn find_trace_files(path: &Path) -> Result<impl Iterator<Item = PathBuf>> {
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
