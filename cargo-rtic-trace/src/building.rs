//! Handle artifact building
//!
//! TODO: properly handle edge-cases. See the original cargo-binutils
//! again.

use std::env;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use cargo_metadata::{Artifact, Message};
use regex::Regex;

pub struct CargoWrapper {
    build_options: Vec<String>,
    target_dir: Option<String>,
}

impl CargoWrapper {
    fn cmd() -> Result<Command> {
        // check if cargo exists
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut cargo = Command::new(cargo);
        let _output = cargo.output().with_context(|| format!("Unable to execute {:?}", cargo))?;

        Ok(cargo)
    }

    pub fn new(build_options: Vec<String>) -> Result<Self> {
        let _ = Self::cmd()?;

        Ok(CargoWrapper {
            build_options,
            target_dir: None,
        })
    }

    /// Finds the configured build cache (usually a `target/` in the crate
    /// root) as reported by cargo.
    ///
    /// TODO support sccache?
    pub fn resolve_target_dir(&mut self, artifact: &Artifact) -> Result<()> {
        type ResolveType = Result<Option<String>>;

        // first, check env variable which has highest prio
        let via_environment = || -> ResolveType {
            if let Some(val) = env::var_os("CARGO_BUILD_DIR") {
                Ok(Some(val.to_str().unwrap().to_string()))
            } else {
                Ok(None)
            }
        };

        // then, check cargo args for --target-dir
        //
        // NOTE(hoisted) closure may not borrow self
        let build_options = &self.build_options;
        let via_cargo_args = || -> ResolveType {
            let mut iter = build_options.iter();
            if let Some(_) = iter.find(|opt| opt.as_str() == "--target-dir") {
                Ok(Some(
                    iter.next()
                        .context("--target-dir passed with no argument")?
                        .to_string(),
                ))
            } else {
                Ok(None)
            }
        };

        // then, check cargo +nightly -Z unstable-options config get
        // build.target-dir
        let via_cargo_config = || -> ResolveType {
            let mut cargo = Self::cmd()?;
            let output = cargo
                .args(
                    "+nightly -Z unstable-options config get --format json-value build.target-dir"
                        .split_whitespace(),
                )
                .output()?;
            if output.status.success() {
                let path = String::from_utf8(output.stdout)
                    .context("build.target-dir is not a valid UTF8 string")?;

                Ok(Some(
                    // trim surrounding quotes
                    Regex::new(r#""(.*)""#)
                        .unwrap()
                        .captures(&path)
                        .context("Unable to parse build.target-dir")?
                        .get(1)
                        .unwrap()
                        .as_str()
                        .into(),
                ))
            } else {
                Ok(None)
            }
        };

        // lastly, if none of the above, return target/ which we find by
        // going backwards over the generated artifact
        let via_artifact_path = || -> ResolveType {
            assert!(artifact.executable.is_some());
            let mut path = artifact.executable.clone().unwrap();
            while path.iter().last().unwrap().to_str().unwrap() != "target" {
                path.pop();
            }
            Ok(Some(path.display().to_string()))
        };

        // Try to resolve the target directory, in order of method
        // priority
        let methods: Vec<Box<dyn Fn() -> ResolveType>> = vec![
            Box::new(via_environment),
            Box::new(via_cargo_args),
            Box::new(via_cargo_config),
            Box::new(via_artifact_path),
        ];
        for method in methods {
            if let Ok(Some(path)) = method() {
                self.target_dir = Some(path);
                return Ok(());
            }
        }

        bail!("Unable to resolve target directory");
    }

    pub fn target_dir(&self) -> Option<PathBuf> {
        self.target_dir.as_ref().map(|p| PathBuf::from(p))
    }

    pub fn build(
        &self,
        crate_root: &Path,
        args: String,
        expected_artifact_kind: &str,
    ) -> Result<Artifact> {
        let mut cargo = Self::cmd()?;
        cargo.arg("build");
        cargo.args(&self.build_options);
        cargo.args(args.split_whitespace());

        if let Some(target_dir) = &self.target_dir {
            cargo.args(format!("--target-dir {}", target_dir).split_whitespace());
        }

        cargo.arg("--message-format=json");
        cargo.stdout(Stdio::piped());

        // Dirty fix for evading any eventual .cargo/config in the working
        // directory. We obviously need it when we build the target
        // application, but it breaks libadhoc build.
        if expected_artifact_kind == "cdylib" {
            cargo.current_dir(env::temp_dir());
            cargo.args(
                format!(
                    "--manifest-path {}",
                    crate_root.join("Cargo.toml").display()
                )
                .split_whitespace(),
            );
        } else {
            cargo.current_dir(crate_root);
        }

        eprintln!("{:?}", cargo);

        let mut child = cargo.spawn()?;
        let stdout = BufReader::new(child.stdout.take().expect("Pipe to cargo process failed"));

        // Note: We call `collect` to ensure we don't block stdout which
        // could prevent the process from exiting
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
