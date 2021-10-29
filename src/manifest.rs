use crate::build::CargoWrapper;
use crate::diag;
use crate::ManifestOptions;

use std::convert::TryInto;

use serde::Deserialize;
use serde_json;
use thiserror::Error;

#[derive(Deserialize, Debug)]
struct ManifestPropertiesIntermediate {
    pub pac_name: Option<String>,
    pub pac_features: Option<Vec<String>>,
    pub pac_version: Option<String>,
    pub interrupt_path: Option<String>,
    pub tpiu_freq: Option<u32>,
    pub tpiu_baud: Option<u32>,
}

impl Default for ManifestPropertiesIntermediate {
    fn default() -> Self {
        Self {
            pac_name: None,
            pac_features: None,
            pac_version: None,
            interrupt_path: None,
            tpiu_freq: None,
            tpiu_baud: None,
        }
    }
}

impl ManifestPropertiesIntermediate {
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
        if self.tpiu_freq.is_none() {
            self.tpiu_freq = other.tpiu_freq;
        }
        if self.tpiu_baud.is_none() {
            self.tpiu_baud = other.tpiu_baud;
        }
    }
}

#[derive(Debug)]
pub struct ManifestProperties {
    pub pac_name: String,
    pub pac_version: String,
    pub pac_features: Vec<String>,
    pub interrupt_path: String,
    pub tpiu_freq: u32,
    pub tpiu_baud: u32,
}

#[derive(Error, Debug)]
pub enum ManifestMetadataError {
    #[error("Manifest metadata table could not be read: {0:?}")]
    DeserializationFailed(#[from] serde_json::Error),
    #[error("Manifest metadata is missing PAC name")]
    MissingName,
    #[error("Manifest metadata is missing PAC version")]
    MissingVersion,
    #[error("Manifest metadata is missing PAC interrupt path")]
    MissingInterruptPath,
    #[error("Manifest metadata is missing TPIU frequency")]
    MissingFreq,
    #[error("Manifest metadata is missing TPIU baud rate")]
    MissingBaud,
}

impl diag::DiagnosableError for ManifestMetadataError {
    fn diagnose(&self) -> Vec<String> {
        match self {
            Self::MissingName => vec!["Add `pac_name = \"<your PAC name>\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --pac-name".into()],
            Self::MissingVersion => vec!["Add `pac_version = \"your PAC version\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --pac-version".into()],
            Self::MissingInterruptPath => vec!["Add `interrupt_path = \"path to your PAC's Interrupt enum\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --pac-interrupt-path".into()],
            Self::MissingFreq => vec!["Add `tpiu_freq = \"your TPIU frequency\" to [package.metadata.rtic-scope] in Cargo.toml or specify --tpiu-freq`".into()],
            Self::MissingBaud => vec!["Add `tpiu_baud = \"your TPIU baud rate\" to [package.metadata.rtic-scope] in Cargo.toml or specify --tpiu-baud`".into()],
            _ => vec![],
        }
    }
}

impl TryInto<ManifestProperties> for ManifestPropertiesIntermediate {
    type Error = ManifestMetadataError;

    fn try_into(self) -> Result<ManifestProperties, Self::Error> {
        Ok(ManifestProperties {
            pac_name: self.pac_name.ok_or(Self::Error::MissingName)?,
            pac_version: self.pac_version.ok_or(Self::Error::MissingVersion)?,
            interrupt_path: self
                .interrupt_path
                .ok_or(Self::Error::MissingInterruptPath)?,
            pac_features: self.pac_features.unwrap_or([].to_vec()),
            tpiu_freq: self.tpiu_freq.ok_or(Self::Error::MissingFreq)?,
            tpiu_baud: self.tpiu_baud.ok_or(Self::Error::MissingBaud)?,
        })
    }
}

impl ManifestProperties {
    pub fn new(
        cargo: &CargoWrapper,
        opts: &ManifestOptions,
    ) -> Result<Self, ManifestMetadataError> {
        let package_meta = cargo.package().unwrap().metadata.get("rtic-scope");
        let workspace_meta = cargo.metadata().workspace_metadata.get("rtic-scope");

        use serde_json::from_value;

        // Read from cargo manifest
        let mut int = match (package_meta, workspace_meta) {
            (Some(pkg), Some(wrk)) => {
                let mut pkg: ManifestPropertiesIntermediate = from_value(pkg.to_owned())?;
                let wrk: ManifestPropertiesIntermediate = from_value(wrk.to_owned())?;

                pkg.complete_with(wrk);
                pkg
            }
            (Some(pkg), None) => from_value(pkg.to_owned())?,
            (None, Some(wrk)) => from_value(wrk.to_owned())?,
            _ => ManifestPropertiesIntermediate::default(),
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
        if let Some(freq) = &opts.tpiu_freq {
            int.tpiu_freq = Some(freq.to_owned());
        }
        if let Some(baud) = &opts.tpiu_baud {
            int.tpiu_baud = Some(baud.to_owned());
        }

        int.try_into()
    }
}
