use crate::build::CargoWrapper;
use crate::diag;
use crate::PACOptions;

use std::convert::TryInto;

use serde::Deserialize;
use serde_json;
use thiserror::Error;

#[derive(Deserialize, Debug)]
struct PACPropertiesIntermediate {
    pub pac_name: Option<String>,
    pub pac_features: Option<Vec<String>>,
    pub pac_version: Option<String>,
    pub interrupt_path: Option<String>,
}

impl Default for PACPropertiesIntermediate {
    fn default() -> Self {
        Self {
            pac_name: None,
            pac_features: None,
            pac_version: None,
            interrupt_path: None,
        }
    }
}

impl PACPropertiesIntermediate {
    pub fn complete_with(&mut self, other: Self) {
        if self.pac_name.is_none() {
            self.pac_name = other.pac_name;
        }
        if self.pac_version.is_none() {
            self.pac_version = other.pac_version;
        }
        if self.interrupt_path.is_none() {
            self.interrupt_path = other.interrupt_path;
        }
        if self.pac_features.is_none() {
            self.pac_features = other.pac_features;
        }
    }
}

#[derive(Debug)]
pub struct PACProperties {
    pub pac_name: String,
    pub pac_version: String,
    pub pac_features: Vec<String>,
    pub interrupt_path: String,
}

#[derive(Error, Debug)]
pub enum PACMetadataError {
    #[error("Manifest metadata table could not be read: {0:?}")]
    DeserializationFailed(#[from] serde_json::Error),
    #[error("Manifest metadata is missing PAC name")]
    MissingName,
    #[error("Manifest metadata is missing PAC version")]
    MissingVersion,
    #[error("Manifest metadata is missing PAC interrupt path")]
    MissingInterruptPath,
}

impl diag::DiagnosableError for PACMetadataError {
    fn diagnose(&self) -> Vec<String> {
        match self {
            Self::MissingName => vec!["Add `pac_name = \"<your PAC name>\"` to [package.metadata.rtic-scope] in Cargo.toml".into()],
            Self::MissingVersion => vec!["Add `pac_version = \"your PAC version\"` to [package.metadata.rtic-scope] in Cargo.toml".into()],
            Self::MissingInterruptPath => vec!["Add `interrupt_path = \"path to your PAC's Interrupt enum\"` to [package.metadata.rtic-scope] in Cargo.toml".into()],
            _ => vec![],
        }
    }
}

impl TryInto<PACProperties> for PACPropertiesIntermediate {
    type Error = PACMetadataError;

    fn try_into(self) -> Result<PACProperties, Self::Error> {
        Ok(PACProperties {
            pac_name: self.pac_name.ok_or(Self::Error::MissingName)?,
            pac_version: self.pac_version.ok_or(Self::Error::MissingVersion)?,
            interrupt_path: self
                .interrupt_path
                .ok_or(Self::Error::MissingInterruptPath)?,
            pac_features: self.pac_features.unwrap_or([].to_vec()),
        })
    }
}

impl PACProperties {
    pub fn new(cargo: &CargoWrapper, opts: &PACOptions) -> Result<Self, PACMetadataError> {
        let package_meta = cargo.package().unwrap().metadata.get("rtic-scope");
        let workspace_meta = cargo.metadata().workspace_metadata.get("rtic-scope");

        use serde_json::from_value;

        // Read from cargo manifest
        let mut int = match (package_meta, workspace_meta) {
            (Some(pkg), Some(wrk)) => {
                let mut pkg: PACPropertiesIntermediate = from_value(pkg.to_owned())?;
                let wrk: PACPropertiesIntermediate = from_value(wrk.to_owned())?;

                pkg.complete_with(wrk);
                pkg
            }
            (Some(pkg), None) => from_value(pkg.to_owned())?,
            (None, Some(wrk)) => from_value(wrk.to_owned())?,
            _ => PACPropertiesIntermediate::default(),
        };

        // Complete/override with opts
        if let Some(pac) = &opts.pac_name {
            int.pac_name = Some(pac.to_owned());
        }
        if let Some(pac_version) = &opts.pac_version {
            int.pac_version = Some(pac_version.to_owned());
        }
        if let Some(intp) = &opts.interrupt_path {
            int.interrupt_path = Some(intp.to_owned());
        }
        if let Some(feats) = &opts.pac_features {
            int.pac_features = Some(feats.to_owned());
        }

        int.try_into()
    }
}
