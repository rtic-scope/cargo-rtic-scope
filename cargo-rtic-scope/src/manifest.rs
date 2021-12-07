use crate::build::CargoWrapper;
use crate::diag;
use crate::ManifestOptions;

use std::convert::TryInto;

use cortex_m::peripheral::itm::LocalTimestampOptions;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Deserialize, Debug, Default)]
struct ManifestPropertiesIntermediate {
    pub pac_name: Option<String>,
    pub pac_features: Option<Vec<String>>,
    pub pac_version: Option<String>,
    pub interrupt_path: Option<String>,
    pub tpiu_freq: Option<u32>,
    pub tpiu_baud: Option<u32>,
    pub lts_prescaler: Option<u8>,
    pub dwt_enter_id: Option<usize>,
    pub dwt_exit_id: Option<usize>,
    pub expect_malformed: Option<bool>,
}

impl ManifestPropertiesIntermediate {
    pub fn complete_with(&mut self, other: Self) {
        macro_rules! complete {
            ($($f:ident),+) => {{
                $(
                    if self.$f.is_none() {
                        self.$f = other.$f;
                    }
                )+
            }}
        }
        complete!(
            pac_name,
            pac_version,
            pac_features,
            interrupt_path,
            tpiu_freq,
            tpiu_baud,
            lts_prescaler,
            dwt_enter_id,
            dwt_exit_id,
            expect_malformed
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProperties {
    pub pac_name: String,
    pub pac_version: String,
    pub pac_features: Vec<String>,
    pub interrupt_path: String,
    pub tpiu_freq: u32,
    pub tpiu_baud: u32,
    pub lts_prescaler: LocalTimestampOptions,
    pub dwt_enter_id: usize,
    pub dwt_exit_id: usize,
    pub expect_malformed: bool,
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
    #[error("Manifest metadata is missing LTS prescaler")]
    MissingLTSPrescaler,
    #[error("Manifest metadata is missing the DWT unit ID for entering/exiting software tasks")]
    MissingDWTUnit,
    #[error("Manifest metadata is missing conditional whether malformed packets are expected")]
    MissingExpectMalformed,
}

impl diag::DiagnosableError for ManifestMetadataError {
    fn diagnose(&self) -> Vec<String> {
        match self {
            Self::MissingName => vec!["Add `pac_name = \"<your PAC name>\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --pac-name".into()],
            Self::MissingVersion => vec!["Add `pac_version = \"your PAC version\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --pac-version".into()],
            Self::MissingInterruptPath => vec!["Add `interrupt_path = \"path to your PAC's Interrupt enum\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --pac-interrupt-path".into()],
            Self::MissingFreq => vec!["Add `tpiu_freq = \"your TPIU frequency\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --tpiu-freq".into()],
            Self::MissingBaud => vec!["Add `tpiu_baud = \"your TPIU baud rate\"` to [package.metadata.rtic-scope] in Cargo.toml or specify --tpiu-baud".into()],
            Self::MissingLTSPrescaler => vec!["Add `lts_prescaler = <your LTS prescaler value (accepted values: 1, 4, 16, 64)>` to [package.metadata.rtic-scope] in Cargo.toml".into()],
            Self::MissingDWTUnit => vec!["Add `dwt_enter_id = \"your enter DWT unit ID\"` and `dwt_exit_id = \"your exit DWT unit ID\"` to [package.metadata.rtic-scope] in Cargo.toml".into()],
            Self::MissingExpectMalformed => vec!["Add `expect_malformed = <whether malformed packets are expected>` to [package.metadata.rtic-scope] in Cargo.toml".into()],
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
            pac_features: self.pac_features.unwrap_or_else(|| [].to_vec()),
            tpiu_freq: self.tpiu_freq.ok_or(Self::Error::MissingFreq)?,
            tpiu_baud: self.tpiu_baud.ok_or(Self::Error::MissingBaud)?,
            lts_prescaler: self
                .lts_prescaler
                .ok_or(Self::Error::MissingLTSPrescaler)?
                .try_into()
                .map_err(|_| Self::Error::MissingLTSPrescaler)?,
            dwt_enter_id: self.dwt_enter_id.ok_or(Self::Error::MissingDWTUnit)?,
            dwt_exit_id: self.dwt_exit_id.ok_or(Self::Error::MissingDWTUnit)?,
            expect_malformed: self
                .expect_malformed
                .ok_or(Self::Error::MissingExpectMalformed)?,
        })
    }
}

impl ManifestProperties {
    pub fn new(
        cargo: &CargoWrapper,
        opts: Option<&ManifestOptions>,
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

        if let Some(opts) = opts {
            macro_rules! maybe_override {
                ($($f:ident),+) => {{
                    $(
                        if let Some($f) = &opts.$f {
                            int.$f = Some($f.to_owned());
                        }
                    )+
                }}
            }
            maybe_override!(
                pac_name,
                pac_version,
                pac_features,
                interrupt_path,
                tpiu_freq,
                tpiu_baud
            );
        }

        int.try_into()
    }
}
