use crate::{error::DeviceError, filesystem_ops::FileSystem};
use csi_driver::filesystem::FileSystem as Fs;

use serde_json::Value;
use std::{collections::HashMap, process::Command, str::FromStr, string::String, vec::Vec};
use tracing::{error, warn};
use utils::csi_plugin_name;

// Keys of interest we expect to find in the JSON output generated
// by findmnt.
const TARGET_KEY: &str = "target";
const SOURCE_KEY: &str = "source";
const FSTYPE_KEY: &str = "fstype";

#[derive(Debug)]
pub(crate) struct DeviceMount {
    mount_path: String,
    fstype: FileSystem,
}

impl std::fmt::Display for DeviceMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MountPath: {}, FileSystem: {}",
            self.mount_path, self.fstype
        )
    }
}

impl DeviceMount {
    /// create new DeviceMount
    pub(crate) fn new(mount_path: String, fstype: FileSystem) -> DeviceMount {
        Self { mount_path, fstype }
    }
    /// File system type
    pub(crate) fn fstype(&self) -> FileSystem {
        self.fstype.clone()
    }
    /// Get the mount path.
    pub(crate) fn mount_path(&self) -> String {
        self.mount_path.clone()
    }
}

#[derive(Debug)]
struct Filter<'a> {
    key: &'a str,
    value: &'a str,
}

/// Convert a json value of a key-value pair, to a string,
/// adjusted if required, on the key.
///
/// The source field returned from findmnt can be different for
/// the same source on different systems, for example
///   dev[/nvme0n1], udev[/nvme0n1], tmpfs[/nvme0n1], devtmpfs[/nvme0n1]
/// this function converts those values to the expected /dev/nvme0n1
fn key_adjusted_value(key: &str, value: &Value) -> String {
    lazy_static::lazy_static! {
        static ref RE_UDEVPATH: regex::Regex = regex::Regex::new(
            r"(?x).*\[(?P<device>/.*)\]
        ",
        )
        .unwrap();
    }

    // Do NOT do
    //    let strvalue = value.to_string();
    // that will return a string delimited with quote characters.
    let strvalue: String = match value {
        Value::String(str) => str.to_string(),
        _ => value.to_string(),
    };
    if key == SOURCE_KEY {
        if let Some(caps) = RE_UDEVPATH.captures(&strvalue) {
            return format!("/dev{}", &caps["device"]);
        };
    }
    strvalue
}

const KEYS: &[&str] = &[TARGET_KEY, SOURCE_KEY, FSTYPE_KEY];

/// Convert the json map entry to a hashmap of strings
/// with source, target and fstype key-value pairs only.
/// The source field returned from findmnt is converted
/// to the /dev/xxx form if required.
fn jsonmap_to_hashmap(json_map: &serde_json::Map<String, Value>) -> HashMap<String, String> {
    let mut hmap: HashMap<String, String> = HashMap::new();
    for (key, value) in json_map {
        if KEYS.contains(&key.as_str()) {
            hmap.insert(key.clone(), key_adjusted_value(key, value));
        }
    }
    hmap
}

/// This function recurses over the de-serialised JSON returned by findmnt,
/// finding entries which have key-value pair's matching the
/// filter key-value pair, and populates a vector with those
/// entries as hashmaps of strings.
///
/// The assumptions made on the structure are:
///  1. An object has keys named "target" and "source" for a mount point.
///  2. An object may contain nested arrays of objects.
///
/// The search is deliberately generic (and hence slower) in an attempt to
/// be more robust to future changes in findmnt.
fn filter_findmnt(
    json_val: &serde_json::value::Value,
    filter: &Filter,
    results: &mut Vec<HashMap<String, String>>,
) {
    match json_val {
        Value::Array(json_array) => {
            for jsonvalue in json_array {
                filter_findmnt(jsonvalue, filter, results);
            }
        }
        Value::Object(json_map) => {
            if let Some(value) = json_map.get(filter.key) {
                if filter.value == value || filter.value == key_adjusted_value(filter.key, value) {
                    results.push(jsonmap_to_hashmap(json_map));
                }
            }
            // If the object has arrays, then the assumption is that they are
            // arrays of objects.
            for (_, jsonvalue) in json_map {
                if jsonvalue.is_array() {
                    filter_findmnt(jsonvalue, filter, results);
                }
            }
        }
        jvalue => {
            warn!("Unexpected json type {}", jvalue);
        }
    };
}

/// findmnt executable name.
const FIND_MNT: &str = "findmnt";
/// findmnt arguments, we only want source, target and filesystem type fields.
const FIND_MNT_ARGS: [&str; 3] = ["-J", "-o", "SOURCE,TARGET,FSTYPE"];

/// Execute the Linux utility findmnt, collect the json output,
/// invoke the filter function and return the filtered results.
fn findmnt(params: Filter) -> Result<Vec<HashMap<String, String>>, DeviceError> {
    let output = Command::new(FIND_MNT).args(FIND_MNT_ARGS).output()?;
    if output.status.success() {
        let json_str = String::from_utf8(output.stdout)?;
        let json: Value = serde_json::from_str(&json_str)?;
        let mut results: Vec<HashMap<String, String>> = Vec::new();
        filter_findmnt(&json, &params, &mut results);
        Ok(results)
    } else {
        Err(DeviceError::new(String::from_utf8(output.stderr)?.as_str()))
    }
}

/// Use the Linux utility findmnt to find the name of the device mounted at a
/// directory or block special file, if any.
/// mount_path is the path a device is mounted on.
pub(crate) fn get_devicepath(mount_path: &str) -> Result<Option<String>, DeviceError> {
    let tgt_filter = Filter {
        key: TARGET_KEY,
        value: mount_path,
    };
    let sources = findmnt(tgt_filter)?;
    {
        match sources.len() {
            0 => Ok(None),
            1 => {
                if let Some(devicepath) = sources[0].get(SOURCE_KEY) {
                    Ok(Some(devicepath.to_string()))
                } else {
                    Err(DeviceError::new("missing source field"))
                }
            }
            _ => {
                // should be impossible ...
                warn!(
                    "multiple sources mounted on target {:?}->{}",
                    sources, mount_path
                );
                Err(DeviceError::new(
                    format!("multiple devices mounted at {mount_path}").as_str(),
                ))
            }
        }
    }
}

/// Use the Linux utility findmnt to find the mount paths for a block device,
/// if any.
/// device_path is the path to the device for example "/dev/sda1"
pub(crate) fn get_mountpaths(device_path: &str) -> Result<Vec<DeviceMount>, DeviceError> {
    let dev_filter = Filter {
        key: SOURCE_KEY,
        value: device_path,
    };
    match findmnt(dev_filter) {
        Ok(results) => {
            let mut mountpaths: Vec<DeviceMount> = Vec::new();
            for entry in results {
                if let Some(mountpath) = entry.get(TARGET_KEY) {
                    if let Some(fstype) = entry.get(FSTYPE_KEY) {
                        mountpaths.push(DeviceMount::new(
                            mountpath.to_string(),
                            Fs::from_str(fstype)
                                .unwrap_or(Fs::Unsupported(fstype.to_string()))
                                .into(),
                        ))
                    } else {
                        error!("Missing fstype for {}", mountpath);
                        mountpaths.push(DeviceMount::new(
                            mountpath.to_string(),
                            Fs::Unsupported("".to_string()).into(),
                        ))
                    }
                } else {
                    warn!("missing target field {:?}", entry);
                }
            }
            Ok(mountpaths)
        }
        Err(e) => Err(e),
    }
}

const DRIVER_NAME: &str = "driverName";
const VOLUME_HANDLE: &str = "volumeHandle";
const METADATA_FILE: &str = "vol_data.json";
const DEVICE_PATTERN: &str = "nvme";

/// This filter is to be used specifically search for mountpoints in context of k8s
///
/// # Fields
/// * `driver` - Name of the csi driver which provisioned the volume.
/// * `volume_id` - Name of the volume which is being searched for.
/// * `file_name` - Name of the file where kubelet stores the metadata.
/// * `device_pattern` - Pattern to search for a specific type of device, ex nvme.
#[derive(Debug)]
struct FilterCsiMounts<'a> {
    driver: &'a str,
    volume_id: &'a str,
    file_name: &'a str,
    device_pattern: &'a str,
}

/// Finds CSI mount points using `findmnt` and filters based on the given criteria.
fn find_csi_mount(filter: FilterCsiMounts) -> Result<Vec<HashMap<String, String>>, DeviceError> {
    let output = Command::new(FIND_MNT).args(FIND_MNT_ARGS).output()?;

    if !output.status.success() {
        return Err(DeviceError::new(String::from_utf8(output.stderr)?.as_str()));
    }

    let json_str = String::from_utf8(output.stdout)?;
    let json: Value = serde_json::from_str(&json_str)?;

    let mut results: Vec<HashMap<String, String>> = Vec::new();
    filter_csi_findmnt(&json, &filter, &mut results);

    Ok(results)
}

/// Filters JSON output from `findmnt` for matching CSI mount points as per the filter.
fn filter_csi_findmnt(
    json_val: &Value,
    filter: &FilterCsiMounts,
    results: &mut Vec<HashMap<String, String>>,
) {
    match json_val {
        Value::Array(json_array) => {
            for jsonvalue in json_array {
                filter_csi_findmnt(jsonvalue, filter, results);
            }
        }
        Value::Object(json_map) => {
            if let Some(source_value) = json_map.get(SOURCE_KEY) {
                let source_str = key_adjusted_value(SOURCE_KEY, source_value);
                if source_str.contains(filter.device_pattern) {
                    if let Some(target_value) = json_map.get(TARGET_KEY) {
                        let target_str = target_value.as_str().unwrap_or_default();

                        if let Some(parent_path) = std::path::Path::new(target_str).parent() {
                            let vol_data_path = parent_path.join(filter.file_name);
                            let vol_data_path_str = vol_data_path.to_string_lossy();

                            if let Ok(vol_data) = read_vol_data_json(&vol_data_path_str) {
                                if vol_data.get(DRIVER_NAME)
                                    == Some(&Value::String(filter.driver.to_string()))
                                    && vol_data.get(VOLUME_HANDLE)
                                        == Some(&Value::String(filter.volume_id.to_string()))
                                {
                                    results.push(jsonmap_to_hashmap(json_map));
                                }
                            }
                        }
                    }
                }
            }

            for (_, jsonvalue) in json_map {
                if jsonvalue.is_array() || jsonvalue.is_object() {
                    filter_csi_findmnt(jsonvalue, filter, results);
                }
            }
        }
        _ => (),
    };
}

/// Reads and parses a file into a JSON object.
fn read_vol_data_json(path: &str) -> Result<serde_json::Map<String, Value>, DeviceError> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let json: serde_json::Map<String, Value> = serde_json::from_reader(reader)?;
    Ok(json)
}

/// Retrieves mount paths for a given CSI volume ID by parsing the metadata file.
pub(crate) async fn get_csi_mountpaths(volume_id: &str) -> Result<Vec<DeviceMount>, DeviceError> {
    let filter = FilterCsiMounts {
        driver: &csi_plugin_name(),
        volume_id,
        file_name: METADATA_FILE,
        device_pattern: DEVICE_PATTERN,
    };
    match find_csi_mount(filter) {
        Ok(results) => {
            let mut mountpaths: Vec<DeviceMount> = Vec::new();
            for entry in results {
                if let Some(mountpath) = entry.get(TARGET_KEY) {
                    if let Some(fstype) = entry.get(FSTYPE_KEY) {
                        mountpaths.push(DeviceMount::new(
                            mountpath.to_string(),
                            Fs::from_str(fstype)
                                .unwrap_or(Fs::Unsupported(fstype.to_string()))
                                .into(),
                        ))
                    } else {
                        warn!(volume.id=%volume_id, "Missing fstype for {mountpath}");
                        mountpaths.push(DeviceMount::new(
                            mountpath.to_string(),
                            Fs::Unsupported("".to_string()).into(),
                        ))
                    }
                } else {
                    warn!(volume.id=%volume_id, ?entry, "Missing target field");
                }
            }
            Ok(mountpaths)
        }
        Err(e) => Err(e),
    }
}
