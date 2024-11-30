use nvmeadm::{
    error::NvmeError,
    nvmf_discovery::{disconnect, ConnectArgsBuilder, TrType},
};
use std::{
    collections::HashMap,
    convert::{From, TryFrom},
    path::Path,
    str::FromStr,
};

use csi_driver::PublishParams;
use glob::glob;
use nvmeadm::nvmf_subsystem::Subsystem;
use regex::Regex;
use tracing::warn;
use udev::{Device, Enumerator};
use url::Url;
use uuid::Uuid;

use crate::{
    config::{config, NvmeConfig, NvmeParseParams},
    dev::util::extract_uuid,
    match_dev::match_nvmf_device,
    node::RDMA_CONNECT_CHECK,
    runtime,
};

use super::{Attach, Detach, DeviceError, DeviceName};

lazy_static::lazy_static! {
    static ref DEVICE_REGEX: Regex = Regex::new(r"nvme(\d{1,5})n(\d{1,5})").unwrap();
}

pub(super) struct NvmfAttach {
    host: String,
    port: u16,
    transport: TrType,
    uuid: Uuid,
    nqn: String,
    io_tmo: Option<u32>,
    nr_io_queues: Option<u32>,
    ctrl_loss_tmo: Option<u32>,
    keep_alive_tmo: Option<u32>,
    hostnqn: Option<String>,
    warn_bad: std::sync::atomic::AtomicBool,
}

impl NvmfAttach {
    #[allow(clippy::too_many_arguments)]
    fn new(
        host: String,
        port: u16,
        transport: TrType,
        uuid: Uuid,
        nqn: String,
        nr_io_queues: Option<u32>,
        io_tmo: Option<humantime::Duration>,
        ctrl_loss_tmo: Option<u32>,
        keep_alive_tmo: Option<u32>,
        hostnqn: Option<String>,
    ) -> NvmfAttach {
        NvmfAttach {
            host,
            port,
            transport,
            uuid,
            nqn,
            io_tmo: io_tmo.map(|io_tmo| io_tmo.as_secs().try_into().unwrap_or(u32::MAX)),
            nr_io_queues,
            ctrl_loss_tmo,
            keep_alive_tmo,
            hostnqn,
            warn_bad: std::sync::atomic::AtomicBool::new(true),
        }
    }

    fn get_device(&self) -> Result<Option<Device>, DeviceError> {
        let key: String = format!("uuid.{}", self.uuid);
        let mut enumerator = Enumerator::new()?;

        enumerator.match_subsystem("block")?;
        enumerator.match_property("DEVTYPE", "disk")?;

        let mut first_error = Ok(None);
        for device in enumerator.scan_devices()? {
            match match_device(&device, &key, &self.warn_bad) {
                Ok(name) if name.is_some() => {
                    return Ok(Some(device));
                }
                Err(error) if first_error.is_ok() => {
                    first_error = Err(error);
                }
                _ => {}
            }
        }

        first_error
    }
}

impl TryFrom<&Url> for NvmfAttach {
    type Error = DeviceError;

    fn try_from(url: &Url) -> Result<Self, Self::Error> {
        let host = url
            .host_str()
            .ok_or_else(|| DeviceError::new("missing host"))?;

        let segments: Vec<&str> = url
            .path_segments()
            .ok_or_else(|| DeviceError::new("no path segment"))?
            .collect();

        let uuid = volume_uuid_from_url(url)?;

        let port = url.port().unwrap_or(4420);
        let transport = transport_from_url(url)?;

        let nr_io_queues = config().nvme().nr_io_queues();
        let ctrl_loss_tmo = config().nvme().ctrl_loss_tmo();
        let keep_alive_tmo = config().nvme().keep_alive_tmo();
        let io_tmo = config().nvme().io_tmo();

        let hash_query: HashMap<_, _> = url.query_pairs().collect();
        let hostnqn = hash_query.get("hostnqn").map(ToString::to_string);

        Ok(NvmfAttach::new(
            host.to_string(),
            port,
            transport,
            uuid,
            segments[0].to_string(),
            nr_io_queues,
            io_tmo,
            ctrl_loss_tmo,
            keep_alive_tmo,
            hostnqn,
        ))
    }
}

#[tonic::async_trait]
impl Attach for NvmfAttach {
    async fn parse_parameters(
        &mut self,
        context: &HashMap<String, String>,
    ) -> Result<(), DeviceError> {
        let publish_context = PublishParams::try_from(context)
            .map_err(|error| DeviceError::new(&error.to_string()))?;

        if let Some(val) = publish_context.ctrl_loss_tmo() {
            self.ctrl_loss_tmo = Some(*val);
        }

        // todo: fold the nvme params into a node-specific publish context?
        let nvme_config = NvmeConfig::try_from(context as NvmeParseParams)?;

        if let Some(nr_io_queues) = nvme_config.nr_io_queues() {
            self.nr_io_queues = Some(nr_io_queues);
        }
        if let Some(keep_alive_tmo) = nvme_config.keep_alive_tmo() {
            self.keep_alive_tmo = Some(keep_alive_tmo);
        }
        if self.io_tmo.is_none() {
            if let Some(io_tmo) = publish_context.io_timeout() {
                self.io_tmo = Some(*io_tmo);
            }
        }

        Ok(())
    }

    async fn attach(&self) -> Result<(), DeviceError> {
        // Get the subsystem, if not found issue a connect.
        match Subsystem::get(
            self.host.as_str(),
            &self.port,
            self.transport,
            self.nqn.as_str(),
        ) {
            Ok(subsystem) => {
                tracing::debug!(?subsystem, "Subsystem already present, skipping connect");
                Ok(())
            }
            Err(NvmeError::SubsystemNotFound { .. }) => {
                // The default reconnect delay in linux kernel is set to 10s. Use the
                // same default value unless the timeout is less or equal to 10.
                let reconnect_delay = match self.io_tmo {
                    Some(io_timeout) => {
                        if io_timeout <= 10 {
                            Some(1)
                        } else {
                            Some(10)
                        }
                    }
                    None => None,
                };
                let ca = ConnectArgsBuilder::default()
                    .traddr(&self.host)
                    .transport(self.transport)
                    .trsvcid(self.port.to_string())
                    .nqn(&self.nqn)
                    .ctrl_loss_tmo(self.ctrl_loss_tmo)
                    .reconnect_delay(reconnect_delay)
                    .nr_io_queues(self.nr_io_queues)
                    .hostnqn(self.hostnqn.clone())
                    .keep_alive_tmo(self.keep_alive_tmo)
                    .build()?;

                runtime::spawn_blocking(move || {
                    match ca.connect() {
                        // Should we remove this arm?
                        Err(NvmeError::ConnectInProgress) => Ok(()),
                        Err(err) => Err(err.into()),
                        Ok(_) => Ok(()),
                    }
                })
                .await
                .map_err(|error| DeviceError::from(error.to_string()))?
            }
            Err(err) => Err(err.into()),
        }
    }

    async fn find(&self) -> Result<Option<DeviceName>, DeviceError> {
        self.get_device().map(|device_maybe| match device_maybe {
            Some(device) => device
                .property_value("DEVNAME")
                .map(|path| path.to_str().unwrap().into()),
            None => None,
        })
    }

    async fn fixup(&self) -> Result<(), DeviceError> {
        let Some(io_timeout) = self.io_tmo else {
            return Ok(());
        };

        let device = self
            .get_device()?
            .ok_or_else(|| DeviceError::new("NVMe device not found"))?;
        let dev_name = device.sysname().to_str().unwrap();
        let captures = DEVICE_REGEX.captures(dev_name).ok_or_else(|| {
            DeviceError::new(&format!(
                "NVMe device \"{}\" does not match \"{}\"",
                dev_name, *DEVICE_REGEX,
            ))
        })?;
        let major = captures.get(1).unwrap().as_str();
        let nid = captures.get(2).unwrap().as_str();

        let pattern = format!("/sys/class/block/nvme{major}c*n{nid}/queue");
        let glob = glob(&pattern).unwrap();
        let result = glob
            .into_iter()
            .map(|glob_result| {
                match glob_result {
                    Ok(path) => {
                        let path_str = path.display();
                        // If the timeout was higher than nexus's timeout then IOs could
                        // error out earlier than they should. Therefore we should make sure
                        // that timeouts in the nexus are set to a very high value.
                        tracing::debug!("Setting IO timeout on \"{path_str}\" to {io_timeout}s",);
                        sysfs::write_value(&path, "io_timeout", 1000 * io_timeout).map_err(
                            |error| {
                                tracing::error!(%error, path=%path_str, "Failed to set io_timeout to {io_timeout}s");
                                error.into()
                            },
                        )
                    }
                    Err(error) => {
                        // This should never happen as we should always have permissions to list.
                        tracing::error!(%error, "Unable to collect sysfs for /dev/nvme{major}");
                        Err(DeviceError::new(error.to_string().as_str()))
                    }
                }
            })
            .collect::<Result<Vec<()>, DeviceError>>();
        match result {
            Ok(r) if r.is_empty() => Err(DeviceError::new(&format!(
                "look up of sysfs device directory \"{pattern}\" found 0 entries",
            ))),
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

pub(super) struct NvmfDetach {
    name: DeviceName,
    nqn: String,
}

impl NvmfDetach {
    pub(super) fn new(name: DeviceName, nqn: String) -> NvmfDetach {
        NvmfDetach { name, nqn }
    }
}

#[tonic::async_trait]
impl Detach for NvmfDetach {
    async fn detach(&self) -> Result<(), DeviceError> {
        let nqn = self.nqn.clone();
        runtime::spawn_blocking(move || match disconnect(&nqn) {
            Ok(0) => Err(DeviceError::from(format!(
                "nvmf disconnect {} failed: no device found",
                nqn
            ))),
            Err(error) => Err(error.into()),
            Ok(_) => Ok(()),
        })
        .await
        .map_err(|error| DeviceError::from(error.to_string()))?
    }

    fn devname(&self) -> DeviceName {
        self.name.clone()
    }

    fn devnqn(&self) -> &str {
        &self.nqn
    }
}

/// Get the sysfs block device queue path for the given udev::Device.
fn block_dev_q(device: &Device) -> Result<String, DeviceError> {
    let dev_name = device.sysname().to_str().unwrap();
    let captures = DEVICE_REGEX.captures(dev_name).ok_or_else(|| {
        DeviceError::new(&format!(
            "NVMe device \"{}\" does not match \"{}\"",
            dev_name, *DEVICE_REGEX,
        ))
    })?;
    let major = captures.get(1).unwrap().as_str();
    let nid = captures.get(2).unwrap().as_str();
    Ok(format!("/sys/class/block/nvme{major}c*n{nid}/queue"))
}

/// Check if the given device is a valid NVMf device.
/// # NOTE
/// In older kernels when a device with an existing mount is lost, the nvmf controller
/// is lost, but the block device remains, in a broken state.
/// On newer kernels, the block device is also gone.
pub(crate) fn match_device<'a>(
    device: &'a Device,
    key: &str,
    warn_bad: &std::sync::atomic::AtomicBool,
) -> Result<Option<&'a str>, DeviceError> {
    let Some(devname) = match_nvmf_device(device, key) else {
        return Ok(None);
    };

    let glob = glob(&block_dev_q(device)?).unwrap();
    if !glob.into_iter().any(|glob_result| glob_result.is_ok()) {
        if warn_bad.load(std::sync::atomic::Ordering::Relaxed) {
            let name = device.sysname().to_string_lossy();
            warn!("Block device {name} for volume {key} has no controller!");
            // todo: shoot-down the stale mounts?
            warn_bad.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        return Ok(None);
    }

    Ok(Some(devname))
}

/// Check for the presence of nvme tcp kernel module.
pub(crate) fn check_nvme_tcp_module() -> Result<(), std::io::Error> {
    let path = "/sys/module/nvme_tcp";
    std::fs::metadata(path)?;
    Ok(())
}

/// Check for the presence of nvme tcp kernel module.
/// TODO: Handle the case where this(and for that matter nvme_tcp too)
/// could be builtin module.
#[allow(unused)]
pub(crate) fn check_nvme_rdma_module() -> Result<(), std::io::Error> {
    let path = "/sys/module/nvme_rdma";
    std::fs::metadata(path)?;
    Ok(())
}

/// Set the nvme_core module IO timeout
/// (note, this is a system-wide parameter)
pub(crate) fn set_nvmecore_iotimeout(io_timeout_secs: u32) -> Result<(), std::io::Error> {
    let path = Path::new("/sys/module/nvme_core/parameters");
    tracing::debug!(
        "Setting nvme_core IO timeout on \"{path}\" to {io_timeout_secs}s",
        path = path.to_string_lossy(),
    );
    sysfs::write_value(path, "io_timeout", io_timeout_secs)?;
    Ok(())
}

/// Extract uuid from Url string.
pub(crate) fn volume_uuid_from_url_str(url: &str) -> Result<Uuid, DeviceError> {
    let url = Url::parse(url).map_err(|error| error.to_string())?;
    volume_uuid_from_url(&url)
}
/// Extract uuid from Url.
pub(crate) fn volume_uuid_from_url(url: &Url) -> Result<Uuid, DeviceError> {
    let segments: Vec<&str> = url
        .path_segments()
        .ok_or_else(|| DeviceError::new("no path segment"))?
        .collect();

    if segments.is_empty() || (segments.len() == 1 && segments[0].is_empty()) {
        return Err(DeviceError::new("no path segment"));
    }

    if segments.len() > 1 {
        return Err(DeviceError::new("too many path segments"));
    }

    let components: Vec<&str> = segments[0].split(':').collect();

    if components.len() != 2 {
        return Err(DeviceError::new("invalid NQN"));
    }

    extract_uuid(components[1]).map_err(|error| DeviceError::from(format!("invalid UUID: {error}")))
}

/// Extract nvmf fabric transport from Url.
pub(crate) fn transport_from_url(url: &Url) -> Result<TrType, DeviceError> {
    // Shouldn't expect nvmf:// scheme here in reality. However, if control plane is
    // interacting with an old io-engine then old style uri scheme will be received.
    // Default to tcp for handling that case.
    let default_xprt = TrType::tcp.to_string();
    let xprt = url
        .scheme()
        .split('+')
        .nth(1)
        .unwrap_or(default_xprt.as_str());

    let ret_xprt = TrType::from_str(xprt).map_err(|e| DeviceError::new(format!("{e:?}").as_str()));
    let connect_cap_check = RDMA_CONNECT_CHECK.get().unwrap_or(&(false, false));

    if !connect_cap_check.0 {
        ret_xprt
    } else {
        match ret_xprt {
            Ok(t) if t == TrType::rdma && !connect_cap_check.1 => {
                warn!("rdma incapable node, connecting over tcp");
                Ok(TrType::tcp)
            }
            _else => _else,
        }
    }
}
