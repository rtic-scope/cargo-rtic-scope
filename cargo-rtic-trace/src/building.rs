//! Handle artifact building
//!
//! TODO: properly handle edge-cases. See the original cargo-binutils
//! again.

use std::env;
use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Result};
use cargo_metadata::{Artifact, Message};

/// Ad-hoc build of target binary. Adapted from
/// <https://github.com/rust-embedded/cargo-binutils/blob/115e26e7640337450b609d0d1d14619a1c370c7a/src/lib.rs#L402>.
pub fn cargo_build(crate_directory: &Path, args: &[&str], kind: &str) -> Result<Artifact> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cargo = Command::new(cargo);
    cargo.arg("build");
    cargo.args(args);

    cargo.arg("--message-format=json");
    cargo.stdout(Stdio::piped());

    // Dirty fix for evading any eventual .cargo/config in the working
    // directory. We obviously need it when we build the target
    // application, but it breaks libadhoc build.
    if kind == "cdylib" {
        env::set_current_dir(env::temp_dir())?;
        cargo.args(&[
            "--manifest-path",
            crate_directory.join("Cargo.toml").to_str().unwrap(),
        ]);
    } else {
        assert_eq!(env::current_dir()?, crate_directory);
    }

    eprintln!("{:?}", cargo);

    let mut child = cargo.spawn()?;
    let stdout = BufReader::new(child.stdout.take().expect("Pipe to cargo process failed"));

    // Note: We call `collect` to ensure we don't block stdout which could prevent the process from exiting
    let messages = Message::parse_stream(stdout).collect::<Vec<_>>();

    let status = child.wait()?;
    if !status.success() {
        bail!("Failed to parse crate metadata");
    }

    let mut target_artifact: Option<Artifact> = None;
    for message in messages {
        match message? {
            Message::CompilerArtifact(artifact) if artifact.target.kind == [kind] => {
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
