//! Artifact building using a wrapper around a cargo sub-process call.
use crate::diag;

use std::env;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub use cargo_metadata::Artifact;
use cargo_metadata::Message;
use thiserror::Error;

pub struct CargoWrapper {
    target_dir: Option<PathBuf>,
    app_metadata: Option<cargo_metadata::Metadata>,
}

#[derive(Debug, Error)]
pub enum CargoError {
    #[error("Failed to find Cargo.toml while traversing upwards from {}", .0.display())]
    CannotFindManifest(PathBuf),
    #[error("Multiple suitable {0} artifacts were found after `cargo build {}` where one was expected", Self::maybe_opts_to_str(.1))]
    MultipleSuitableArtifacts(String, Option<Vec<String>>),
    #[error("No suitable {0} artifacts were found after `cargo build {}`", Self::maybe_opts_to_str(.1))]
    NoSuitableArtifact(String, Option<Vec<String>>),
    #[error("`cargo build {}` failed with {0}", Self::maybe_opts_to_str(.1))]
    CargoBuildExecFailed(std::process::ExitStatus, Option<Vec<String>>),
    #[error("Failed to execute `cargo metadata`: {0}")]
    CargoMetadataExecFailed(#[from] cargo_metadata::Error),
    #[error("Failed to find root package from `cargo metadata`")]
    CannotFindRootPackage,
    #[error("Failed to canonicalize {0}: {1}")]
    CannotCanonicalize(PathBuf, std::io::Error),
    #[error("Failed to execute cargo: {0}")]
    CargoBuildSpawnWaitError(#[source] std::io::Error),
    #[error("Failed to read stdout message from cargo: {0}")]
    StdoutError(#[source] std::io::Error),
    #[error("Failed to resolve the current directory: {0}")]
    CurrentDirError(#[source] std::io::Error),
}

impl CargoError {
    fn maybe_opts_to_str(opts: &Option<Vec<String>>) -> String {
        opts.as_ref().unwrap_or(&vec![]).join(" ")
    }
}

impl diag::DiagnosableError for CargoError {
    fn diagnose(&self) -> Vec<String> {
        match self {
            CargoError::MultipleSuitableArtifacts(kind, _opts)
            | CargoError::NoSuitableArtifact(kind, _opts) => vec![format!(
                "Modify your call so that only one {}-crate is built. Try --bin or --example.",
                kind
            )],
            CargoError::CargoBuildExecFailed(_, _) => vec!["Cargo errors/warnings are not properly propagated at the moment (see <https://github.com/rtic-scope/cargo-rtic-scope/issues/50>).".to_string(),
            "Manually build your target application with `cargo build` to see eventual errors/warnings.".to_string()],
            _ => vec![],
        }
    }
}

/// A functionality wrapper around subproccess calls to cargo in `PATH`.
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
    pub fn new(crate_root: &Path, opts: Vec<String>) -> Result<(Self, Artifact), CargoError> {
        let cargo = Self::intermediate();
        let artifact = cargo.build(crate_root, Some(opts.clone()), "bin")?;

        // Resolve artifact metadata
        let manifest_path = opts
            .iter()
            .position(|opt| opt.as_str() == "--manifest-path")
            .and_then(|idx| opts.get(idx + 1))
            .map(PathBuf::from)
            .unwrap_or(find_manifest_path(&artifact)?);
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()?;

        Ok((
            CargoWrapper {
                target_dir: Some(metadata.target_directory.clone().canonicalize().map_err(
                    |e| CargoError::CannotCanonicalize(metadata.target_directory.clone().into(), e),
                )?),
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

    pub fn package(&self) -> Result<&cargo_metadata::Package, CargoError> {
        self.metadata()
            .root_package()
            .ok_or(CargoError::CannotFindRootPackage)
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
    ) -> Result<Artifact, CargoError> {
        let mut cargo = Self::cmd();
        cargo.arg("build");
        if let Some(ref opts) = opts {
            cargo.args(opts);
        }

        // NOTE target_dir() panics during intermediate build
        if self.app_metadata.is_some() {
            cargo.arg("--target-dir");
            cargo.arg(self.target_dir());
        }

        cargo.arg("--message-format=json-diagnostic-rendered-ansi");
        cargo.stdout(Stdio::piped());
        cargo.stderr(Stdio::piped());

        // Dirty fix for evading any eventual .cargo/config in the working
        // directory. We obviously need it when we build the target
        // application, but it breaks libadhoc build.
        if expected_artifact_kind == "cdylib" {
            cargo.current_dir(env::temp_dir()); // XXX what if /.cargo/config?
            cargo.args(
                format!(
                    "--manifest-path {}",
                    crate_root
                        .canonicalize()
                        .map_err(|e| CargoError::CannotCanonicalize(crate_root.to_path_buf(), e))?
                        .join("Cargo.toml")
                        .display()
                )
                .split_whitespace(),
            );
        } else {
            cargo.current_dir(crate_root);
        }

        let mut child = cargo
            .spawn()
            .map_err(CargoError::CargoBuildSpawnWaitError)?;
        let stdout = BufReader::new(child.stdout.take().expect("Pipe to cargo process failed"));
        let stderr = BufReader::new(child.stderr.take().expect("Pipe to cargo process failed"));

        let messages = Message::parse_stream(stdout).chain(Message::parse_stream(stderr));

        let mut target_artifact: Option<Artifact> = None;
        for message in messages {
            match message.map_err(CargoError::StdoutError)? {
                Message::CompilerArtifact(artifact)
                    if artifact.target.kind == [expected_artifact_kind] =>
                {
                    if target_artifact.is_some() {
                        return Err(CargoError::MultipleSuitableArtifacts(
                            expected_artifact_kind.to_string(),
                            opts,
                        ));
                    }
                    target_artifact = Some(artifact);
                }
                Message::CompilerMessage(msg) => {
                    if let Some(rendered) = msg.message.rendered {
                        eprint!("{}", rendered);
                    }
                }
                _ => (),
            }
        }

        let status = child.wait().map_err(CargoError::CargoBuildSpawnWaitError)?;

        if !status.success() {
            return Err(CargoError::CargoBuildExecFailed(status, opts));
        }

        if target_artifact.is_none() {
            return Err(CargoError::NoSuitableArtifact(
                expected_artifact_kind.to_string(),
                opts,
            ));
        }

        Ok(target_artifact.unwrap())
    }
}

fn find_manifest_path(artifact: &cargo_metadata::Artifact) -> Result<PathBuf, CargoError> {
    let start_path = || {
        let mut path = artifact.executable.clone().unwrap();
        path.pop();
        path
    };
    let mut path = start_path();

    loop {
        let res = {
            path.push("Cargo.toml");
            path.exists()
        };
        if res {
            return Ok(path.into());
        } else {
            path.pop(); // remove Cargo.toml
            if path.pop() {
                // move up a directory
                continue;
            }

            return Err(CargoError::CannotFindManifest(start_path().into()));
        }
    }
}
