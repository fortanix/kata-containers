// Copyright (c) 2026 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//
mod initdata;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use slog::{info, o, warn, Drain, Logger};

use crate::initdata::{locate_device, locate_unmeasured_device, read_initdata};
use kata_types::initdata::InitData;
use kata_types::unmeasured_initdata::UnmeasuredInitData;

const DEV_DIR: &str = "/dev";
const MEASURED_CFG_DIR: &str = "/run/measured-cfg";
const UNMEASURED_CFG_DIR: &str = "/run/unmeasured-cfg";

#[derive(Debug)]
struct InitDataProcessor {
    dev_path: PathBuf,
    config_path: PathBuf,
    unmeasured_config_path: PathBuf,
    logger: Logger,
}

impl InitDataProcessor {
    pub fn new(logger: Logger) -> Self {
        Self {
            dev_path: PathBuf::from(DEV_DIR),
            config_path: PathBuf::from(MEASURED_CFG_DIR),
            unmeasured_config_path: PathBuf::from(UNMEASURED_CFG_DIR),
            logger,
        }
    }

    pub fn with_config_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.config_path = dir.into();
        self
    }

    pub fn with_unmeasured_config_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.unmeasured_config_path = dir.into();
        self
    }

    pub fn with_dev_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dev_path = dir.into();
        self
    }

    /// Writes configurations.
    fn write_config_files(
        &self,
        config_path: &Path,
        raw_initdata: &[u8],
        data: &HashMap<String, String>,
    ) -> Result<()> {
        info!(
            self.logger,
            "Writing configuration files to: {:?}", config_path
        );

        if std::fs::exists(config_path)? {
            std::fs::remove_dir_all(config_path)?;
        }

        let initdata_dir = config_path.join("initdata");
        let initdata_file = initdata_dir.with_extension("toml");

        // Create the config_path.
        std::fs::create_dir_all(&initdata_dir)
            .context(format!("Failed to create config path: {:?}", &initdata_dir,))?;

        // Set directory permissions.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for dir in [config_path, &initdata_dir] {
                let mut perms = std::fs::metadata(dir)?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(dir, perms)?;
            }
        }

        self.write_file(&initdata_file, raw_initdata)
            .context(format!(
                "Failed to write initdata file {:?}",
                &initdata_file
            ))?;

        let mut written_files = 1;

        // Write each configuration item.
        for (key, value) in data {
            let file_path = initdata_dir.join(key);
            let Some(parent) = file_path.parent() else {
                warn!(self.logger, "Skipping invalid key: {}", key);
                continue;
            };

            // Security check: Ensure file path is within the directory.
            let initdata_dir = initdata_dir.canonicalize()?;
            let parent = parent.canonicalize()?;
            if !parent.starts_with(&initdata_dir) {
                warn!(self.logger, "Skipping potentially dangerous key: {}", key);
                continue;
            }
            // TODO(burgerdev): support subdirectories

            self.write_file(&file_path, value.as_bytes())
                .context(format!("Failed to write config file for key: {}", key))?;

            written_files += 1;
        }

        info!(
            self.logger,
            "Successfully wrote {} configuration files", written_files
        );
        Ok(())
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o444) // read-only for all users
            .open(path)
            .context(format!("Failed to open file: {:?}", path))?;

        file.write_all(content)
            .context(format!("Failed to write content to: {:?}", path))?;

        file.sync_all()
            .context(format!("Failed to sync file: {:?}", path))?;

        Ok(())
    }

    /// The complete workflow for processing initdata.
    pub fn process(&self) -> Result<()> {
        info!(self.logger, "Starting initdata processing");

        self.process_measured_initdata()?;
        self.process_unmeasured_initdata()?;

        info!(self.logger, "Initdata processing completed successfully");
        Ok(())
    }

    fn process_measured_initdata(&self) -> Result<()> {
        // 1. Locate and parse initdata.
        let initdata_device_opt = locate_device(&self.dev_path, &self.logger)?;
        let initdata_device = match initdata_device_opt {
            Some(device) => device,
            None => return Ok(()),
        };
        info!(
            self.logger,
            "Reading initdata from device: {:?}", initdata_device
        );
        let initdata_content =
            read_initdata(&initdata_device).context("Failed to read initdata")?;

        let initdata: InitData =
            toml::from_slice(&initdata_content).context("parse initdata failed")?;

        info!(
            self.logger,
            "Successfully parsed initdata with {} entries",
            initdata.data().len()
        );

        // 2. Validate initdata structurally.
        initdata
            .validate()
            .context("structural initdata validation failed")?;

        // TODO(burgerdev): 3. invoke the external initdata-validator.

        // 4. Write config files.
        self.write_config_files(&self.config_path, &initdata_content, initdata.data())?;

        Ok(())
    }

    fn process_unmeasured_initdata(&self) -> Result<()> {
        let initdata_device_opt = locate_unmeasured_device(&self.dev_path, &self.logger)?;
        let initdata_device = match initdata_device_opt {
            Some(device) => device,
            None => return Ok(()),
        };
        info!(
            self.logger,
            "Reading unmeasured initdata from device: {:?}", initdata_device
        );
        let initdata_content =
            read_initdata(&initdata_device).context("Failed to read unmeasured initdata")?;

        let initdata: UnmeasuredInitData =
            toml::from_slice(&initdata_content).context("parse unmeasured initdata failed")?;

        info!(
            self.logger,
            "Successfully parsed unmeasured initdata with {} entries",
            initdata.data().len()
        );

        initdata
            .validate()
            .context("structural unmeasured initdata validation failed")?;

        self.write_config_files(
            &self.unmeasured_config_path,
            &initdata_content,
            initdata.data(),
        )?;

        Ok(())
    }
}

fn create_logger() -> Logger {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    slog::Logger::root(drain, o!())
}

fn main() -> Result<()> {
    let logger = create_logger();

    // Parse command line arguments.
    let args: Vec<String> = std::env::args().collect();
    let mut processor = InitDataProcessor::new(logger);

    // Simple command line argument parsing.
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config-path" => {
                if i + 1 < args.len() {
                    processor = processor.with_config_path(&args[i + 1]);
                    i += 2;
                } else {
                    return Err(anyhow::anyhow!("--config-path requires a path argument"));
                }
            }
            "--unmeasured-config-path" => {
                if i + 1 < args.len() {
                    processor = processor.with_unmeasured_config_path(&args[i + 1]);
                    i += 2;
                } else {
                    return Err(anyhow::anyhow!(
                        "--unmeasured-config-path requires a path argument"
                    ));
                }
            }
            "--dev-path" => {
                if i + 1 < args.len() {
                    processor = processor.with_dev_path(&args[i + 1]);
                    i += 2;
                } else {
                    return Err(anyhow::anyhow!("--dev-path requires a path argument"));
                }
            }
            _ => return Err(anyhow::anyhow!("Unknown argument: {}", args[i])),
        }
    }

    // Execute the processing.
    processor.process().context("Initdata processing failed")
}
