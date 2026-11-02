//! # Initdata Module
//!
//! This module will do the following things if a proper initdata device with initdata exists.
//! 1. Parse the initdata block device and extract the config files to [`INITDATA_PATH`].
//! 2. Return the initdata and the policy (if any).

// Copyright (c) 2025 Alibaba Cloud
//
// SPDX-License-Identifier: Apache-2.0
//

#[cfg(feature = "init-data")]
use std::{os::unix::fs::FileTypeExt, path::Path};

use anyhow::{bail, Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use base64::{engine::general_purpose::STANDARD, Engine};
use const_format::concatcp;
use kata_types::initdata::InitData;
use sha2::{Digest, Sha256, Sha384, Sha512};
use slog::Logger;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// This is the target directory to store the extracted initdata.
pub const INITDATA_PATH: &str = "/run/confidential-containers/initdata";
pub const UNMEASURED_INITDATA_PATH: &str = "/run/unmeasured-cfg";

const AA_CONFIG_KEY: &str = "aa.toml";
const CDH_CONFIG_KEY: &str = "cdh.toml";
const POLICY_KEY: &str = "policy.rego";
pub const UNMEASURED_INITDATA_CONFIG_KEY: &str = "unmeasured-initdata-cfg.toml";

/// The path of initdata toml
pub const INITDATA_TOML_PATH: &str = concatcp!(INITDATA_PATH, "/initdata.toml");
pub const UNMEASURED_INITDATA_CONFIG_PATH: &str =
    concatcp!(INITDATA_PATH, "/", UNMEASURED_INITDATA_CONFIG_KEY);

/// The path of AA's config file
pub const AA_CONFIG_PATH: &str = concatcp!(INITDATA_PATH, "/aa.toml");
pub const UNMEASURED_AA_CONFIG_PATH: &str =
    concatcp!(UNMEASURED_INITDATA_PATH, "/initdata/aa.toml");

/// The path of CDH's config file
pub const CDH_CONFIG_PATH: &str = concatcp!(INITDATA_PATH, "/cdh.toml");
pub const UNMEASURED_CDH_CONFIG_PATH: &str =
    concatcp!(UNMEASURED_INITDATA_PATH, "/initdata/cdh.toml");

/// Magic number of initdata device
#[cfg(feature = "init-data")]
pub const INITDATA_MAGIC_NUMBER: &[u8] = b"initdata";

/// initdata device with disk type 'vd*'
#[cfg(feature = "init-data")]
const INITDATA_PREFIX_DISK_VDX: &str = "vd";

/// initdata device with disk type 'sd*'
#[cfg(feature = "init-data")]
const INITDATA_PREFIX_DISK_SDX: &str = "sd";

#[cfg(not(feature = "init-data"))]
async fn detect_initdata_device(logger: &Logger) -> Result<Option<String>> {
    debug!(logger, "Initdata is disabled");
    Ok(None)
}

#[cfg(feature = "init-data")]
async fn detect_initdata_device(logger: &Logger) -> Result<Option<String>> {
    let dev_dir = Path::new("/dev");
    let mut read_dir = tokio::fs::read_dir(dev_dir).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let filename = entry.file_name();
        let filename = filename.to_string_lossy();
        debug!(logger, "Initdata check device `{filename}`");

        // Currently there're two disk types supported:
        // virtio-blk (vd*) and virtio-scsi (sd*)
        if !filename.starts_with(INITDATA_PREFIX_DISK_VDX)
            && !filename.starts_with(INITDATA_PREFIX_DISK_SDX)
        {
            continue;
        }

        let path = entry.path();

        debug!(logger, "Initdata find potential device: `{path:?}`");
        let metadata = std::fs::metadata(path.clone())?;
        if !metadata.file_type().is_block_device() {
            continue;
        }

        let mut file = tokio::fs::File::open(&path).await?;
        let mut magic = [0; 8];
        match file.read_exact(&mut magic).await {
            Ok(_) => {
                debug!(
                    logger,
                    "Initdata read device `{filename}` first 8 bytes: {magic:?}"
                );
                if magic == INITDATA_MAGIC_NUMBER {
                    let path = path.as_path().to_string_lossy().to_string();
                    debug!(logger, "Found initdata device {path}");
                    return Ok(Some(path));
                }
            }
            Err(e) => debug!(logger, "Initdata read device `{filename}` failed: {e:?}"),
        }
    }

    Ok(None)
}

pub async fn read_initdata(device_path: &str) -> Result<Vec<u8>> {
    let initdata_devfile = tokio::fs::File::open(device_path).await?;
    let mut buf_reader = tokio::io::BufReader::new(initdata_devfile);
    // skip the magic number "initdata"
    buf_reader.seek(std::io::SeekFrom::Start(8)).await?;

    let mut len_buf = [0u8; 8];
    buf_reader.read_exact(&mut len_buf).await?;
    let length = u64::from_le_bytes(len_buf) as usize;

    let mut buf = vec![0; length];
    buf_reader.read_exact(&mut buf).await?;
    let mut gzip_decoder = GzipDecoder::new(&buf[..]);

    let mut initdata = Vec::new();
    let _ = gzip_decoder.read_to_end(&mut initdata).await?;
    Ok(initdata)
}

pub struct InitdataReturnValue {
    pub _digest: Vec<u8>,
    pub _policy: Option<String>,
}

/// The standalone initdata-processor binary, installed into images that carry it.
const INITDATA_PROCESSOR_PATH: &str = "/usr/bin/initdata-processor";

/// Upper bound for one processor run; same as TimeoutStartSec of its systemd unit.
const INITDATA_PROCESSOR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Run the initdata-processor from the agent on images without systemd (the NVIDIA GPU images
/// use NVRC as init), where nothing else can order it before the agent.
///
/// Skipped when the binary is not installed, or when `ran_marker` exists: the processor's
/// systemd unit creates [`UNMEASURED_INITDATA_PATH`] via `RuntimeDirectory=`, so its presence
/// means the unit already ran. A processor failure is fatal, like the unit's `Requires=` on
/// kata-agent.service.
async fn run_initdata_processor(
    logger: &Logger,
    processor: &std::path::Path,
    ran_marker: &std::path::Path,
) -> Result<()> {
    if !processor.exists() {
        debug!(logger, "initdata-processor not installed, skipping");
        return Ok(());
    }
    if ran_marker.exists() {
        debug!(logger, "initdata-processor already ran (systemd), skipping");
        return Ok(());
    }

    info!(logger, "running initdata-processor"; "path" => processor.display());
    let mut child = tokio::process::Command::new(processor)
        .kill_on_drop(true)
        .spawn()
        .context("spawn initdata-processor failed")?;
    let status = tokio::time::timeout(INITDATA_PROCESSOR_TIMEOUT, child.wait())
        .await
        .context("initdata-processor timed out")?
        .context("wait for initdata-processor failed")?;
    if !status.success() {
        bail!("initdata-processor failed: {status}");
    }
    Ok(())
}

pub async fn initialize_initdata(logger: &Logger) -> Result<Option<InitdataReturnValue>> {
    let logger = logger.new(o!("subsystem" => "initdata"));

    if cfg!(feature = "init-data") {
        run_initdata_processor(
            &logger,
            std::path::Path::new(INITDATA_PROCESSOR_PATH),
            std::path::Path::new(UNMEASURED_INITDATA_PATH),
        )
        .await?;
    }

    let Some(initdata_device) = detect_initdata_device(&logger).await? else {
        info!(
            logger,
            "Initdata device not found, skip initdata initialization"
        );
        return Ok(None);
    };

    tokio::fs::create_dir_all(INITDATA_PATH)
        .await
        .inspect_err(|e| error!(logger, "Failed to create initdata dir: {e:?}"))?;

    let initdata_content = read_initdata(&initdata_device)
        .await
        .inspect_err(|e| error!(logger, "Failed to read initdata: {e:?}"))?;

    let initdata: InitData =
        toml::from_slice(&initdata_content).context("parse initdata failed")?;
    info!(logger, "Initdata version: {}", initdata.version());
    initdata.validate()?;

    tokio::fs::write(INITDATA_TOML_PATH, &initdata_content)
        .await
        .context("write initdata toml failed")?;

    let _digest = match initdata.algorithm() {
        "sha256" => Sha256::digest(&initdata_content).to_vec(),
        "sha384" => Sha384::digest(&initdata_content).to_vec(),
        "sha512" => Sha512::digest(&initdata_content).to_vec(),
        others => bail!("Unsupported hash algorithm {others}"),
    };

    if let Some(config) = initdata.get_coco_data(AA_CONFIG_KEY) {
        tokio::fs::write(AA_CONFIG_PATH, config)
            .await
            .context("write aa config failed")?;
        info!(logger, "write AA config from initdata");
    }

    if let Some(config) = initdata.get_coco_data(CDH_CONFIG_KEY) {
        tokio::fs::write(CDH_CONFIG_PATH, config)
            .await
            .context("write cdh config failed")?;
        info!(logger, "write CDH config from initdata");
    }

    if let Some(config) = initdata.get_coco_data(UNMEASURED_INITDATA_CONFIG_KEY) {
        tokio::fs::write(UNMEASURED_INITDATA_CONFIG_PATH, config)
            .await
            .context("write unmeasured initdata config failed")?;
        info!(
            logger,
            "write unmeasured initdata config from measured initdata"
        );
    }

    debug!(logger, "Initdata digest: {}", STANDARD.encode(&_digest));

    let res = InitdataReturnValue {
        _digest,
        _policy: initdata.get_coco_data(POLICY_KEY).cloned(),
    };

    Ok(Some(res))
}

#[cfg(test)]
mod tests {
    use crate::initdata::read_initdata;

    const INITDATA_IMG_PATH: &str = "testdata/initdata.img";
    const INITDATA_PLAINTEXT: &[u8] = b"some content";

    #[tokio::test]
    async fn parse_initdata() {
        let initdata = read_initdata(INITDATA_IMG_PATH).await.unwrap();
        assert_eq!(initdata, INITDATA_PLAINTEXT);
    }

    mod processor {
        use crate::initdata::run_initdata_processor;
        use std::path::Path;

        fn logger() -> slog::Logger {
            slog::Logger::root(slog::Discard, slog::o!())
        }

        #[tokio::test]
        async fn skipped_when_binary_missing() {
            let dir = tempfile::tempdir().unwrap();
            let missing = dir.path().join("initdata-processor");
            run_initdata_processor(&logger(), &missing, &dir.path().join("marker"))
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn skipped_when_systemd_already_ran() {
            // /bin/false would fail the run, so success proves the processor was not executed.
            let dir = tempfile::tempdir().unwrap();
            run_initdata_processor(&logger(), Path::new("/bin/false"), dir.path())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn runs_when_installed_and_not_yet_ran() {
            let dir = tempfile::tempdir().unwrap();
            run_initdata_processor(
                &logger(),
                Path::new("/bin/true"),
                &dir.path().join("marker"),
            )
            .await
            .unwrap();
        }

        #[tokio::test]
        async fn failure_is_fatal() {
            let dir = tempfile::tempdir().unwrap();
            let err = run_initdata_processor(
                &logger(),
                Path::new("/bin/false"),
                &dir.path().join("marker"),
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("initdata-processor failed"));
        }
    }
}
