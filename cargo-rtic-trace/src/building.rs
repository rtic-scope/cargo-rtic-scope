//! Handle artifact building using cargo.

use std::env;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use cargo_metadata::{Artifact, Message};
use regex::Regex;

pub struct CargoWrapper {
    build_options: Vec<String>,
    target_dir: Option<PathBuf>,
}

/// A functioality wrapper around subproccess calls to cargo in PATH.
impl CargoWrapper {
    /// Checks if cargo exists in PATH and returns it wrapped in a Command.
    fn cmd() -> Result<Command> {
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut cargo = Command::new(cargo);
        let _output = cargo
            .output()
            .with_context(|| format!("Unable to execute {:?}", cargo))?;

        Ok(cargo)
    }

    /// Creates a new wrapper instance after ensuring that a cargo
    /// executable is available in `PATH`. Can be overridden via the
    /// `CARGO` environment variable. Passed `build_options` is expected
    /// to be a set off `cargo build` flags. These are applied in all
    /// `build` calls.
    pub fn new(mut build_options: Vec<String>) -> Result<Self> {
        // Early check if cargo exists. Because PATH is unlikely to
        // change, a Command instance could potentially be passed around
        // instead of recreated whenever one is needed, but it is not
        // possible to reset the arguments of a Command. We may in any
        // case want to consider a small refactor regarding this, when a
        // better solution is found.
        let mut cargo = Self::cmd()?;

        let target_dir =
            if let Some(pos) = build_options.iter().position(|opt| opt == "--target-dir") {
                let path = PathBuf::from(if let Some(path) = build_options.get(pos + 1) {
                    path
                } else {
                    bail!("--target-dir passed, but without argument");
                })
                .canonicalize()?;
                build_options.remove(pos + 1);
                build_options.remove(pos);
                Some(path)
            } else {
                None
            };

        // Only require +nightly toolchain if build cache isn't
        // otherwise set explicitly.
        //
        // TODO fix in production. `cargo rtic-trace` calls
        // /home/tmplt/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo
        // which does not have +nightly.
        if env::var_os("CARGO_TARGET_DIR").is_none()
            && target_dir.is_none()
            && !cargo
                .args("+nightly -Z unstable-features config get --version".split_whitespace())
                .output()
                .unwrap() // safe, output was tested in Self::cmd
                .status
                .success()
        {
            bail!("Neither CARGO_TARGET_DIR nor --target-dir was set. A nightly toolchain is then required to resolve the build cache until <https://github.com/rust-lang/cargo/issues/9301> is stabilized. Install one via rustup install nightly. The following check failed: {:?}", cargo);
        }

        Ok(CargoWrapper {
            build_options,
            target_dir,
        })
    }

    /// Finds the configured build cache (usually a `target/` in the
    /// crate root) as reported by cargo. Any subsequent calls to
    /// `build` will reuse this build cache.
    ///
    /// TODO support sccache?
    pub fn resolve_target_dir(&mut self, artifact: &Artifact) -> Result<()> {
        type ResolveType = Result<Option<String>>;

        if self.target_dir().is_some() {
            return Ok(());
        }

        // first, check env variable which has highest prio
        let via_environment = || -> ResolveType {
            if let Some(val) = env::var_os("CARGO_TARGET_DIR") {
                Ok(Some(val.to_str().unwrap().to_string()))
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
        // precedence. See
        // <https://doc.rust-lang.org/cargo/guide/build-cache.html>.
        //
        // NOTE from local tests, --target-dir have precedence over
        // environmental variables.
        let methods: Vec<Box<dyn Fn() -> ResolveType>> = vec![
            // --target-dir is checked in Self::new
            Box::new(via_environment),
            Box::new(via_cargo_config),
            Box::new(via_artifact_path),
        ];
        for method in methods {
            if let Ok(Some(path)) = method() {
                self.set_target_dir(PathBuf::from(path))?;
                return Ok(());
            }
        }

        bail!("Unable to resolve target directory. Cannot continue.");
    }

    fn set_target_dir(&mut self, target_dir: PathBuf) -> Result<()> {
        self.target_dir = Some(
            target_dir
                .canonicalize()
                .with_context(|| format!("Failed to canonicalize {}", target_dir.display()))?,
        );
        Ok(())
    }

    pub fn target_dir(&self) -> Option<&PathBuf> {
        self.target_dir.as_ref()
    }

    /// Calls `cargo build` within the speficied `crate_root` with the
    /// additional `args` build options and returns the singular
    /// `expected_artifact_kind` (`bin`, `lib`, `cdylib`, etc.) if it is
    /// generated.
    pub fn build(
        &self,
        crate_root: &Path,
        args: String,
        expected_artifact_kind: &str,
    ) -> Result<Artifact> {
        let mut cargo = Self::cmd()?;
        cargo.arg("build");

        assert!(!args.contains("--target-dir"));
        cargo.args(args.split_whitespace());

        assert!(self
            .build_options
            .iter()
            .find(|opt| opt.as_str() == "--target-dir")
            .is_none());
        cargo.args(&self.build_options);

        if let Some(target_dir) = self.target_dir() {
            assert!(target_dir.is_absolute());
            cargo.arg("--target-dir");
            cargo.arg(target_dir);
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

        // TODO replace with
        //
        //     println!("Running: {} {}", cargo.get_program(), cargo.get_args())
        //
        // when feature(command_access) is stabilized. See
        // <https://github.com/rust-lang/rust/issues/44434>.
        //
        // Perhaps we should mimic cargo. cargo install prints a green
        // "Installing". We could have a a green "Building" and then a
        // "Tracing" when we start with that.
        eprintln!("{:?}", cargo);

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
