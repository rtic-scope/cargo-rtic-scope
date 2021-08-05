use crate::build::CargoWrapper;
use crate::PACOptions;

use std::convert::TryInto;

use anyhow::Result;
use serde::Deserialize;
use serde_json;
use thiserror::Error;

#[derive(Deserialize, Debug)]
struct PACPropertiesIntermediate {
    #[serde(rename = "pac")]
    pub name: Option<String>,
    #[serde(rename = "pac_features")]
    pub features: Option<Vec<String>>,
    pub interrupt_path: Option<String>,
}

impl Default for PACPropertiesIntermediate {
    fn default() -> Self {
        Self {
            name: None,
            features: None,
            interrupt_path: None,
        }
    }
}

impl PACPropertiesIntermediate {
    pub fn complete_with(&mut self, other: Self) {
        if self.name.is_none() {
            self.name = other.name;
        }
        if self.interrupt_path.is_none() {
            self.interrupt_path = other.interrupt_path;
        }
        match (self.features.as_mut(), other.features) {
            (None, Some(feats)) => self.features = Some(feats),
            (Some(feats), Some(mut ofeats)) => feats.append(&mut ofeats),
            _ => (),
        }
    }
}

#[derive(Debug)]
pub struct PACProperties {
    pub name: String,
    pub features: Vec<String>,
    pub interrupt_path: String,
}

#[derive(Error, Debug)]
pub enum PACMetadataError {
    #[error("Manifest metadata table could not be read")]
    DeserializationFailed,
    #[error("Manifest metadata is missing PAC name")]
    MissingName,
    #[error("Manifest metadata is missing PAC interrupt path")]
    MissingInterruptPath,
}

impl TryInto<PACProperties> for PACPropertiesIntermediate {
    type Error = PACMetadataError;

    fn try_into(self) -> Result<PACProperties, Self::Error> {
        Ok(PACProperties {
            name: self.name.ok_or(Self::Error::MissingName)?,
            interrupt_path: self
                .interrupt_path
                .ok_or(Self::Error::MissingInterruptPath)?,
            features: self.features.unwrap_or([].to_vec()),
        })
    }
}

impl PACProperties {
    pub fn new(cargo: &CargoWrapper, opts: &PACOptions) -> Result<Self, PACMetadataError> {
        let package_meta = cargo.package().unwrap().metadata.get("rtic-scope");
        let workspace_meta = cargo.metadata().workspace_metadata.get("rtic-scope");

        // Read from cargo manifest
        let mut int = match (package_meta, workspace_meta) {
            (Some(pkg), Some(wrk)) => {
                let mut pkg: PACPropertiesIntermediate = serde_json::from_value(pkg.to_owned())
                    .map_err(|_| PACMetadataError::DeserializationFailed)?;
                let wrk: PACPropertiesIntermediate = serde_json::from_value(wrk.to_owned())
                    .map_err(|_| PACMetadataError::DeserializationFailed)?;

                pkg.complete_with(wrk);
                pkg
            }
            (Some(pkg), None) => serde_json::from_value(pkg.to_owned())
                .map_err(|_| PACMetadataError::DeserializationFailed)?,
            (None, Some(wrk)) => serde_json::from_value(wrk.to_owned())
                .map_err(|_| PACMetadataError::DeserializationFailed)?,
            _ => PACPropertiesIntermediate::default(),
        };

        // Complete/override with opts
        if let Some(pac) = &opts.name {
            int.name = Some(pac.to_owned());
        }
        if let Some(intp) = &opts.interrupt_path {
            int.interrupt_path = Some(intp.to_owned());
        }
        if let Some(feats) = &opts.features {
            int.features
                .get_or_insert(feats.clone())
                .append(&mut feats.clone());
        }

        int.try_into()
    }
}
