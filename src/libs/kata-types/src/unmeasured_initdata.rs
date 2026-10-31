// Copyright (c) Fortanix, Inc.
//
// SPDX-License-Identifier: Apache-2.0
//

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Read};

use crate::initdata::parse_initdata;

/// Currently, unmeasured initdata only supports version 0.1.0.
const UNMEASURED_INITDATA_VERSION: &str = "0.1.0";
/// Measured initdata data key that controls whether unmeasured initdata is consumed.
pub const UNMEASURED_INITDATA_CONFIG_KEY: &str = "unmeasured-initdata-cfg.toml";

/// Trusted measured initdata wrapper controlling unmeasured initdata consumption.
#[derive(Debug, Clone, Deserialize)]
pub struct UnmeasuredInitDataConfig {
    /// Whether the guest should consume unmeasured initdata.
    pub enabled: bool,
}

/// Unmeasured initdata contains file-based configuration supplied by an untrusted host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UnmeasuredInitData {
    /// Version of the unmeasured initdata format.
    version: String,
    /// Data for specific key-value pairs.
    data: HashMap<String, String>,
}

impl UnmeasuredInitData {
    /// Create unmeasured initdata.
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            data: HashMap::new(),
        }
    }

    /// Get unmeasured initdata by key.
    pub fn get_coco_data(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }

    /// Insert a data item.
    pub fn insert_data(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.data.insert(key.into(), value.into());
    }

    /// Get the format version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Get all data items.
    pub fn data(&self) -> &HashMap<String, String> {
        &self.data
    }

    /// Serialize to TOML bytes.
    pub fn to_vec(&self) -> Result<Vec<u8>> {
        Ok(toml::to_string(self)?.into_bytes())
    }

    /// Serialize to a pretty TOML string.
    pub fn to_string(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Validate unmeasured initdata.
    pub fn validate(&self) -> Result<()> {
        if self.version != UNMEASURED_INITDATA_VERSION {
            return Err(anyhow!(
                "unsupported version: {}, expected: {}",
                self.version,
                UNMEASURED_INITDATA_VERSION
            ));
        }

        Ok(())
    }
}

/// Parse and validate unmeasured initdata.
pub fn parse_unmeasured_initdata(initdata_toml: &str) -> Result<UnmeasuredInitData> {
    let initdata: UnmeasuredInitData = toml::from_str(initdata_toml)?;
    initdata.validate()?;
    Ok(initdata)
}

/// Encode unmeasured initdata as an annotation.
pub fn encode_unmeasured_initdata(initdata: &UnmeasuredInitData) -> String {
    let toml_str = toml::to_string_pretty(initdata).unwrap();
    create_encoded_input(&toml_str)
}

/// Decode an unmeasured initdata annotation.
pub fn decode_unmeasured_initdata(annotation: &str) -> Result<UnmeasuredInitData> {
    let initdata_toml = decode_raw_unmeasured_initdata(annotation)?;

    parse_unmeasured_initdata(&initdata_toml).context("parse unmeasured initdata")
}

fn decode_raw_unmeasured_initdata(annotation: &str) -> Result<String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(annotation)
        .context("base64 decode")?;

    let mut decoder = GzDecoder::new(&decoded[..]);
    let mut initdata_toml = String::new();
    decoder
        .read_to_string(&mut initdata_toml)
        .context("gz decoder failed")?;

    Ok(initdata_toml)
}

/// Decodes unmeasured initdata annotation for hypervisor config.
pub fn add_hypervisor_unmeasured_initdata_overrides(initdata_annotation: &str) -> Result<String> {
    if initdata_annotation.is_empty() {
        return Ok("".to_string());
    }

    decode_raw_unmeasured_initdata(initdata_annotation)
        .context("decoding unmeasured initdata annotation failed")
}

/// Returns whether measured initdata enables unmeasured initdata consumption.
pub fn unmeasured_initdata_enabled(initdata_toml: &str) -> Result<bool> {
    if initdata_toml.is_empty() {
        return Ok(false);
    }

    let initdata = parse_initdata(initdata_toml).context("parse measured initdata")?;
    let Some(config) = initdata.data().get(UNMEASURED_INITDATA_CONFIG_KEY) else {
        return Ok(false);
    };

    let config: UnmeasuredInitDataConfig =
        toml::from_str(config).context("parse unmeasured initdata config")?;
    Ok(config.enabled)
}

use std::io::Write;

fn create_encoded_input(content: &str) -> String {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(content.as_bytes()).unwrap();
    let compressed = encoder.finish().unwrap();
    base64::engine::general_purpose::STANDARD.encode(&compressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unmeasured_initdata_files() {
        let input = r#"version = "0.1.0"

[data]
"aa.toml" = '''customer_id = "customer-a"'''
"cdh.toml" = '''region = "us-east-1"'''
"#;

        let initdata = parse_unmeasured_initdata(input).unwrap();
        assert_eq!(initdata.version(), "0.1.0");
        assert_eq!(initdata.data().len(), 2);
        assert_eq!(
            initdata.get_coco_data("aa.toml").unwrap(),
            "customer_id = \"customer-a\""
        );
    }

    #[test]
    fn construct_and_serialize_unmeasured_initdata() {
        let mut initdata = UnmeasuredInitData::new("0.1.0");
        initdata.insert_data("aa.toml", "customer_id = 'customer-a'");

        let serialized = initdata.to_string().unwrap();
        let parsed = parse_unmeasured_initdata(&serialized).unwrap();
        assert_eq!(parsed.data(), initdata.data());

        let serialized = initdata.to_vec().unwrap();
        let parsed = parse_unmeasured_initdata(std::str::from_utf8(&serialized).unwrap()).unwrap();
        assert_eq!(parsed.data(), initdata.data());

        let encoded = encode_unmeasured_initdata(&initdata);
        let decoded = decode_unmeasured_initdata(&encoded).unwrap();
        assert_eq!(decoded.version(), initdata.version());
        assert_eq!(decoded.data(), initdata.data());

        let decoded_raw = add_hypervisor_unmeasured_initdata_overrides(&encoded).unwrap();
        let decoded = parse_unmeasured_initdata(&decoded_raw).unwrap();
        assert_eq!(decoded.version(), initdata.version());
        assert_eq!(decoded.data(), initdata.data());
    }

    #[test]
    fn reject_invalid_encoded_unmeasured_initdata() {
        assert!(decode_unmeasured_initdata("not-base64").is_err());

        let encoded = base64::engine::general_purpose::STANDARD.encode("not gzip");
        assert!(decode_unmeasured_initdata(&encoded).is_err());
    }

    #[test]
    fn validate_version() {
        assert!(UnmeasuredInitData::new("0.1.0").validate().is_ok());
        assert!(UnmeasuredInitData::new("0.2.0").validate().is_err());
    }

    #[test]
    fn keys_are_opaque_to_the_data_type() {
        let input = "version = '0.1.0'\n[data]\n'../aa.toml' = ''\n";
        assert!(parse_unmeasured_initdata(input).is_ok());
    }

    #[test]
    fn measured_initdata_enables_unmeasured_initdata() {
        let enabled = format!(
            r#"version = "0.1.0"
algorithm = "sha256"

[data]
"{}" = '''
enabled = true
'''
"#,
            UNMEASURED_INITDATA_CONFIG_KEY
        );
        assert!(unmeasured_initdata_enabled(&enabled).unwrap());

        let disabled = enabled.replace("enabled = true", "enabled = false");
        assert!(!unmeasured_initdata_enabled(&disabled).unwrap());

        let missing = r#"version = "0.1.0"
algorithm = "sha256"

[data]
"aa.toml" = ""
"#;
        assert!(!unmeasured_initdata_enabled(missing).unwrap());
        assert!(!unmeasured_initdata_enabled("").unwrap());
    }
}
