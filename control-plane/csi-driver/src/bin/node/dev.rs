//! Definition of traits required for attaching and detaching devices.
//! Note that (unfortunately) the attach and detach operations are not
//! quite symmetric. This is because it is not possible to discover all
//! the information contained in a device URI from the entries in udev.
//! Hence there are separate traits for attach and detach that each
//! device type must implement.
//!
//! Attaching a device is performed as follows:
//! ```ignore
//!     let uri = "iscsi://192.168.0.20:3260/iqn.2019-05.com.org:11111111-0000-0000-0000-000000000000/0";
//!     let device = Device::parse(uri)?;
//!     if let Some(path) = device.find().await? {
//!         // device already attached
//!     } else {
//!         // attach the device
//!         device.attach().await?;
//!         // wait for it to show up in udev and obtain the path
//!         let path = Device::wait_for_device(device, timeout, 10).await?;
//!     }
//! ```
//!
//! Detaching a device is performed via:
//! ```ignore
//!     let uuid = Uuid::parse_str(&volume_id)?;
//!     if let Some(device) = Device::lookup(&uuid).await? {
//!         device.detach().await?;
//!     }
//! ```

use std::{
    collections::HashMap,
    convert::TryFrom,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::sleep;
use udev::Enumerator;
use url::Url;
use uuid::Uuid;

mod iscsi;
mod nbd;
pub(crate) mod nvmf;
mod util;

use utils::constants::nvme_target_nqn_prefix;

pub(crate) use crate::error::DeviceError;
use crate::match_dev;

pub(crate) type DeviceName = String;

#[tonic::async_trait]
pub(crate) trait Attach: Sync + Send {
    async fn parse_parameters(
        &mut self,
        context: &HashMap<String, String>,
    ) -> Result<(), DeviceError>;
    async fn attach(&self) -> Result<(), DeviceError>;
    async fn find(&self) -> Result<Option<DeviceName>, DeviceError>;
    /// Fixup parameters which cannot be set during attach, eg IO timeout
    async fn fixup(&self) -> Result<(), DeviceError>;
}

#[tonic::async_trait]
pub(crate) trait Detach: Sync + Send {
    async fn detach(&self) -> Result<(), DeviceError>;
    fn devname(&self) -> DeviceName;
    fn devnqn(&self) -> &str;
}

pub(crate) struct Device;

impl Device {
    /// Main dispatch function for parsing URIs in order
    /// to obtain a device implementing the Attach trait.
    pub(crate) fn parse(uri: &str) -> Result<Box<dyn Attach>, DeviceError> {
        let url = Url::parse(uri).map_err(|error| error.to_string())?;
        match url.scheme() {
            "file" => Ok(Box::new(nbd::Nbd::try_from(&url)?)),
            "iscsi" => Ok(Box::new(iscsi::IscsiAttach::try_from(&url)?)),
            "nvmf" | "nvmf+tcp" | "nvmf+rdma+tcp" => {
                Ok(Box::new(nvmf::NvmfAttach::try_from(&url)?))
            }
            "nbd" => Ok(Box::new(nbd::Nbd::try_from(&url)?)),
            scheme => Err(DeviceError::from(format!(
                "unsupported device scheme: {scheme}"
            ))),
        }
    }

    /// Lookup an existing device in udev matching the given UUID
    /// to obtain a device implementing the Detach trait.
    pub(crate) async fn lookup(uuid: &Uuid) -> Result<Option<Box<dyn Detach>>, DeviceError> {
        let nvmf_key: String = format!("uuid.{uuid}");

        let mut enumerator = Enumerator::new()?;

        enumerator.match_subsystem("block")?;
        enumerator.match_property("DEVTYPE", "disk")?;

        for device in enumerator.scan_devices()? {
            if let Some((devname, path)) = match_dev::match_iscsi_device(&device) {
                let value = iscsi::IscsiDetach::from_path(devname.to_string(), path)?;

                if value.uuid() == uuid {
                    return Ok(Some(Box::new(value)));
                }

                continue;
            }

            if let Some(devname) = match_dev::match_nvmf_device(&device, &nvmf_key) {
                let nqn = if std::env::var("MOAC").is_ok() {
                    format!("{}:nexus-{uuid}", nvme_target_nqn_prefix())
                } else {
                    format!("{}:{uuid}", nvme_target_nqn_prefix())
                };
                return Ok(Some(Box::new(nvmf::NvmfDetach::new(
                    devname.to_string(),
                    nqn,
                ))));
            }
        }

        Ok(None)
    }

    /// Wait for a device to show up in udev
    /// once attach() has been called.
    pub(crate) async fn wait_for_device(
        device: &dyn Attach,
        timeout: Duration,
        retries: u32,
    ) -> Result<DeviceName, DeviceError> {
        for _ in 0..=retries {
            if let Some(devname) = device.find().await? {
                return Ok(devname);
            }
            sleep(timeout).await;
        }
        Err(DeviceError::new("device attach timeout"))
    }
}

/// Get the block device capacity size from the sysfs block count.
/// Arg can be /dev/device-name style dev-path or just the device name itself.
pub(crate) fn sysfs_dev_size<N: AsRef<Path>>(device: N) -> Result<usize, DeviceError> {
    // Linux sectors are 512 byte long. This is fixed.
    // Ref: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/types.h?id=v5.15#n117
    const LINUX_SECTOR_SIZE: usize = 512;

    let dev_name = device
        .as_ref()
        .iter()
        // Works for both /dev/<device> and just <device>.
        .last()
        .ok_or(DeviceError::new(
            "cannot find the sysfs size for device: invalid name or path",
        ))?;

    let sysfs_dir = PathBuf::from("/sys/class/block").join(dev_name);

    let size: usize = sysfs::parse_value(sysfs_dir.as_path(), "size")?;

    Ok(size * LINUX_SECTOR_SIZE)
}
