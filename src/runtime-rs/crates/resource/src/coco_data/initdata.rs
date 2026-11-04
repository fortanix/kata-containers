// Copyright (c) 2025 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::{io::ErrorKind, path::Path};

use anyhow::{Context, Result};
use hypervisor::{utils::uses_native_ccw_bus, BlockConfigModern, BlockDeviceAio};
use kata_types::{
    config::hypervisor::{Hypervisor as HypervisorConfig, VIRTIO_BLK_CCW, VIRTIO_BLK_PCI},
    prefix_with_rootless_dir,
    unmeasured_initdata::{parse_unmeasured_initdata, unmeasured_initdata_enabled},
};

use super::initdata_block;

/// The path /run/kata-containers/shared/initdata, combined with the sandbox ID,
/// will form the default directory for storing the initdata image.
pub const DEFAULT_KATA_SHARED_INIT_DATA_PATH: &str = "/run/kata-containers/shared/initdata";

/// kata initdata image
pub const KATA_INIT_DATA_IMAGE: &str = "initdata.image";

/// Unmeasured data shares the sandbox's initdata directory, but has its own image.
pub const KATA_UNMEASURED_INIT_DATA_IMAGE: &str = "unmeasured-initdata.image";

/// InitDataConfig which is a tuple of Block Device Config and its digest of the encoded
/// string included in the disk. And, both of them will come up at the same time.
#[derive(Clone, Debug)]
pub struct InitDataConfig(pub BlockConfigModern, pub String);

/// The path /run/kata-containers/shared/initdata, combined with the sandbox ID,
/// will form the directory for storing the initdata image.
/// The directory will be prefixed with the rootless directory when running in rootless mode
pub fn kata_shared_init_data_path() -> String {
    prefix_with_rootless_dir(DEFAULT_KATA_SHARED_INIT_DATA_PATH)
}

/// Prepare a separate, read-only disk without changing the measured initdata digest.
pub fn prepare_unmeasured_initdata(
    config: &HypervisorConfig,
    image_path: &Path,
) -> Result<Option<BlockConfigModern>> {
    let data = &config.security_info.unmeasured_initdata;
    if data.is_empty() || !unmeasured_initdata_enabled(&config.security_info.initdata)? {
        return Ok(None);
    }
    parse_unmeasured_initdata(data).context("validate unmeasured initdata")?;
    initdata_block::push_unmeasured_data(image_path, data)?;
    Ok(Some(BlockConfigModern {
        path_on_host: image_path.display().to_string(),
        serial_override: "unmeasured-initdata".to_string(),
        is_readonly: true,
        // The processor discovers virtio-blk by serial or by scanning /dev/vd*.
        // A container-rootfs driver such as virtio-pmem would be undiscoverable.
        driver_option: if uses_native_ccw_bus() {
            VIRTIO_BLK_CCW.into()
        } else {
            VIRTIO_BLK_PCI.into()
        },
        blkdev_aio: BlockDeviceAio::Native,
        num_queues: config.blockdev_info.num_queues,
        queue_size: config.blockdev_info.queue_size,
        ..Default::default()
    }))
}

/// Remove both initialization disks once the sandbox VMM has stopped.
pub fn cleanup_initdata(sid: &str) -> Result<()> {
    let path = Path::new(&kata_shared_init_data_path()).join(sid);
    cleanup_initdata_dir(&path)
}

fn cleanup_initdata_dir(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("remove initdata directory {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmeasured_cleanup_removes_only_the_sandbox_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        let other = dir.path().join("other");
        std::fs::create_dir(&sandbox).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::write(sandbox.join(KATA_INIT_DATA_IMAGE), "measured").unwrap();
        std::fs::write(sandbox.join(KATA_UNMEASURED_INIT_DATA_IMAGE), "unmeasured").unwrap();
        cleanup_initdata_dir(&sandbox).unwrap();
        assert!(!sandbox.exists());
        assert!(other.exists());
        cleanup_initdata_dir(&sandbox).unwrap();
    }

    #[test]
    fn unmeasured_disk_requires_measured_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KATA_UNMEASURED_INIT_DATA_IMAGE);
        let mut config = HypervisorConfig::default();
        config.security_info.unmeasured_initdata = "version = \"0.1.0\"\n[data]\n".into();
        config.blockdev_info.block_device_driver = "virtio-pmem".into();
        assert!(prepare_unmeasured_initdata(&config, &path)
            .unwrap()
            .is_none());
        assert!(!path.exists());
        for enabled in [false, true] {
            config.security_info.initdata = format!(
                r#"version = "0.1.0"
algorithm = "sha256"
[data]
"unmeasured-initdata-cfg.toml" = 'enabled = {enabled}'
"#
            );
            let block = prepare_unmeasured_initdata(&config, &path).unwrap();
            assert_eq!(block.is_some(), enabled);
            assert_eq!(path.exists(), enabled);
            if let Some(block) = block {
                assert!(block.is_readonly);
                assert_eq!(
                    block.driver_option,
                    if uses_native_ccw_bus() {
                        VIRTIO_BLK_CCW
                    } else {
                        VIRTIO_BLK_PCI
                    }
                );
                assert_eq!(block.serial_override, "unmeasured-initdata");
                assert_eq!(&std::fs::read(&path).unwrap()[..8], b"unmsrdat");
                config.security_info.unmeasured_initdata = "invalid".into();
                assert!(prepare_unmeasured_initdata(&config, &path).is_err());
            }
        }
    }
}
