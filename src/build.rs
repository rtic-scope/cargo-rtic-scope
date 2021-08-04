//! Handle artifact building using cargo.

use std::env;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use cargo_metadata;
pub use cargo_metadata::Artifact;
use cargo_metadata::Message;

pub struct CargoWrapper {
    target_dir: Option<PathBuf>,
    app_metadata: Option<cargo_metadata::Metadata>,
}

/// A functioality wrapper around subproccess calls to cargo in PATH.
impl CargoWrapper {
    fn cmd() -> Command {
        Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
    }

    fn intermediate() -> Self {
        CargoWrapper {
            target_dir: None,
            app_metadata: None,
        }
    }

    /// Creates a new wrapper instance after ensuring that a cargo
    /// executable is available in `PATH`. Can be overridden via the
    /// `CARGO` environment variable.
    pub fn new(crate_root: &Path, opts: Vec<String>) -> Result<(Self, Artifact)> {
        let cargo = Self::intermediate();
        let artifact = cargo.build(crate_root, Some(opts.clone()), "bin")?;

        // Resolve artifact metadata
        let manifest_path = opts
            .iter()
            .position(|ref opt| opt.as_str() == "--manifest-path")
            .and_then(|idx| opts.get(idx + 1))
            .and_then(|arg| Some(PathBuf::from(arg)))
            .unwrap_or(
                find_manifest_path(&artifact)
                    .context("Unable to resolve manifest path of target application")?,
            );
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()
            .context("Failed to read application metadata")?;

        Ok((
            CargoWrapper {
                target_dir: Some(metadata.target_directory.clone().canonicalize()?),
                app_metadata: Some(metadata),
            },
            artifact,
        ))
    }

    pub fn target_dir(&self) -> &PathBuf {
        self.target_dir.as_ref().unwrap()
    }

    pub fn metadata(&self) -> &cargo_metadata::Metadata {
        self.app_metadata.as_ref().unwrap()
    }

    pub fn package(&self) -> Result<&cargo_metadata::Package> {
        self.metadata()
            .root_package()
            .context("Could not find root package")
    }

    /// Calls `cargo build` within the speficied `crate_root` with the
    /// additional `args` build options and returns the singular
    /// `expected_artifact_kind` (`bin`, `lib`, `cdylib`, etc.) if it is
    /// generated.
    pub fn build(
        &self,
        crate_root: &Path,
        opts: Option<Vec<String>>,
        expected_artifact_kind: &str,
    ) -> Result<Artifact> {
        let mut cargo = Self::cmd();
        cargo.arg("build");
        if let Some(opts) = opts {
            cargo.args(opts);
        }

        // NOTE target_dir() panics during intermediate build
        if self.app_metadata.is_some() {
            cargo.arg("--target-dir");
            cargo.arg(self.target_dir());
        }

        cargo.arg("--message-format=json");
        cargo.stdout(Stdio::piped());

        // Dirty fix for evading any eventual .cargo/config in the working
        // directory. We obviously need it when we build the target
        // application, but it breaks libadhoc build.
        if expected_artifact_kind == "cdylib" {
            cargo.current_dir(env::temp_dir()); // XXX what if /.cargo/config?
            cargo.args(
                format!(
                    "--manifest-path {}",
                    crate_root.canonicalize()?.join("Cargo.toml").display()
                )
                .split_whitespace(),
            );
        } else {
            cargo.current_dir(crate_root);
        }

        let mut child = cargo.spawn()?;
        let stdout = BufReader::new(child.stdout.take().expect("Pipe to cargo process failed"));

        // NOTE(collect) ensure we don't block stdout which could
        // prevent the process from exiting
        let messages = Message::parse_stream(stdout).collect::<Vec<_>>();

        let status = child.wait()?;
        if !status.success() {
            bail!(
                "Failed to run cargo: exited with {}; command: {:?}",
                status,
                cargo
            );
        }

        let mut target_artifact: Option<Artifact> = None;
        for message in messages {
            match message? {
                Message::CompilerArtifact(artifact)
                    if artifact.target.kind == [expected_artifact_kind] =>
                {
                    if target_artifact.is_some() {
                        bail!("Can only have one matching artifact but found several");
                    }
                    target_artifact = Some(artifact);
                }
                Message::CompilerMessage(msg) => {
                    if let Some(rendered) = msg.message.rendered {
                        print!("{}", rendered);
                    }
                }
                _ => (),
            }
        }

        if target_artifact.is_none() {
            bail!("Could not determine the wanted artifact");
        }

        Ok(target_artifact.unwrap())
    }
}

fn find_manifest_path(artifact: &cargo_metadata::Artifact) -> Result<PathBuf> {
    let mut path = artifact.executable.clone().unwrap();
    path.pop();

    loop {
        if {
            path.push("Cargo.toml");
            path.exists()
        } {
            return Ok(path);
        } else {
            path.pop(); // remove Cargo.toml
            if path.pop() {
                // move up a directory
                continue;
            }

            bail!("Failed to find manifest");
        }
    }
}
