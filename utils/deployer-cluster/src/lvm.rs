//! LVM helper methods which are useful for setting up test block devices.

use crate::TmpDiskFile;

/// Possible device states are SUSPENDED, ACTIVE, and READ-ONLY.
/// The dmsetup suspend command sets a device state to SUSPENDED.
/// When a device is suspended, all I/O operations to that device stop.
/// The dmsetup resume command restores a device state to ACTIVE.
#[derive(Eq, PartialEq)]
pub enum DMState {
    Suspended,
    Active,
    ReadOnly,
}
impl TryFrom<&str> for DMState {
    type Error = anyhow::Error;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_uppercase().as_str() {
            "SUSPENDED" => Ok(Self::Suspended),
            "ACTIVE" => Ok(Self::Active),
            "READ-ONLY" => Ok(Self::ReadOnly),
            state => Err(anyhow::anyhow!("Unknown state: {state}")),
        }
    }
}
impl std::fmt::Display for DMState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self {
            DMState::Suspended => "Suspended",
            DMState::Active => "Active",
            DMState::ReadOnly => "Read-Only",
        };
        write!(f, "{state}")
    }
}

/// An LVM Logical Volume.
pub struct Lvol {
    name: String,
    path: String,
}
impl Lvol {
    /// Get the host path for the lvol.
    pub fn path(&self) -> &str {
        &self.path
    }
    /// Suspends the device for IO.
    /// # Warning
    /// The device is only suspended once the open count reaches 0.
    /// This means the device might not be suspended after the suspend call completes.
    /// More over, seems the device is also not blocked for new open calls!
    /// You should consider using [Self::suspend_await].
    pub fn suspend(&self) -> anyhow::Result<()> {
        let _ = VolGroup::command(&["dmsetup", "suspend", self.path.as_str()])?;
        Ok(())
    }
    /// Suspends the device for IO and awaits for it to be suspended.
    pub async fn suspend_await(&self) -> anyhow::Result<()> {
        self.suspend()?;
        self.await_state(DMState::Suspended, std::time::Duration::from_secs(10))
            .await?;
        Ok(())
    }
    /// Waits until the device reaches the given state for up to the given timeout.
    pub async fn await_state(
        &self,
        wanted_state: DMState,
        timeout: std::time::Duration,
    ) -> anyhow::Result<()> {
        let start = std::time::Instant::now();
        let mut state = self.dm_state()?;
        loop {
            if state == wanted_state {
                return Ok(());
            }
            if start.elapsed() > timeout {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            state = self.dm_state()?;
        }
        Err(anyhow::anyhow!(
            "Failed to reach {wanted_state}, current: {state}"
        ))
    }
    /// Resumes the device for IO.
    pub fn resume(&self) -> anyhow::Result<()> {
        let _ = VolGroup::command(&["dmsetup", "resume", self.path.as_str()])?;
        Ok(())
    }
    /// Get the dmsetup info.
    pub fn dm_info(&self) -> anyhow::Result<String> {
        let output = VolGroup::command(&["dmsetup", "info", self.path.as_str()])?;
        Ok(output)
    }
    fn dm_state(&self) -> anyhow::Result<DMState> {
        let output =
            VolGroup::command(&["dmsetup", "info", "-C", "-osuspended", self.path.as_str()])?;
        let state = output.lines().skip(1).collect::<String>();
        DMState::try_from(state.as_str())
    }
}
impl Drop for Lvol {
    fn drop(&mut self) {
        println!("Dropping Lvol {}", self.name);
        self.resume().ok();
    }
}

/// An LVM Volume Group.
pub struct VolGroup {
    backing_file: TmpDiskFile,
    dev_loop: String,
    name: String,
}

impl VolGroup {
    /// Creates a new LVM Volume Group.
    pub fn new(name: &str, size: u64) -> Result<Self, anyhow::Error> {
        let backing_file = TmpDiskFile::new(name, size);

        let dev_loop = Self::command(&[
            "losetup",
            "--show",
            "--direct-io=on",
            "-f",
            backing_file.path(),
        ])?;
        let dev_loop = dev_loop.trim_end().to_string();
        let _ = Self::command(&["pvcreate", dev_loop.as_str()])?;
        // We disable auto activation here, otherwise when the systemd activation service runs
        // it may activate all the Lvols, which is not always wanted.
        let _ = Self::command(&["vgcreate", "--setautoactivation=n", name, dev_loop.as_str()])?;

        Ok(Self {
            backing_file,
            dev_loop,
            name: name.to_string(),
        })
    }
    /// Creates a new Lvol for the LVM Volume Group.
    pub fn create_lvol(&self, name: &str, size: u64) -> Result<Lvol, anyhow::Error> {
        let size = format!("{size}B");

        let vg_name = self.name.as_str();
        let _ = Self::command(&["lvcreate", "-L", size.as_str(), "-n", name, vg_name])?;

        Ok(Lvol {
            name: name.to_string(),
            path: format!("/dev/{vg_name}/{name}"),
        })
    }
    /// Run a command with sudo, and the given args.
    /// The output string is returned.
    fn command(args: &[&str]) -> Result<String, anyhow::Error> {
        let mut binding = std::process::Command::new(env!("SUDO"));
        let cmd = binding.arg("-E").args(args);
        let output = cmd.output()?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "{:?} Exit Code: {}\nstdout: {}, stderr: {}",
                cmd.get_args(),
                output.status,
                String::from_utf8(output.stdout).unwrap_or_default(),
                String::from_utf8(output.stderr).unwrap_or_default()
            ));
        }
        let output = String::from_utf8(output.stdout)?;
        Ok(output)
    }
}

impl Drop for VolGroup {
    fn drop(&mut self) {
        println!(
            "Dropping VolGroup {} <== {}",
            self.name,
            self.backing_file.path()
        );

        let now = std::time::Instant::now();
        while now.elapsed() < std::time::Duration::from_secs(2) {
            let remove = Self::command(&["vgremove", "-f", "-y", self.name.as_str()]);
            println!("{remove:?}");
            if remove.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        println!(
            "{:?}",
            Self::command(&["pvremove", "-f", "-y", self.dev_loop.as_str()])
        );
        println!(
            "{:?}",
            Self::command(&["losetup", "-d", self.dev_loop.as_str()])
        );
    }
}
