//! LVM helper methods which are useful for setting up test block devices.

use crate::TmpDiskFile;

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
    pub fn suspend(&self) -> anyhow::Result<()> {
        let _ = VolGroup::command(&["dmsetup", "suspend", self.path.as_str()])?;
        Ok(())
    }
    /// Resumes the device for IO.
    pub fn resume(&self) -> anyhow::Result<()> {
        let _ = VolGroup::command(&["dmsetup", "resume", self.path.as_str()])?;
        Ok(())
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

        let dev_loop = Self::command(&["losetup", "--show", "-f", backing_file.path()])?;
        let dev_loop = dev_loop.trim_end().to_string();
        let _ = Self::command(&["pvcreate", dev_loop.as_str()])?;
        let _ = Self::command(&["vgcreate", name, dev_loop.as_str()])?;

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
        let cmd = args.first().unwrap();
        let output = std::process::Command::new("sudo")
            .arg("-E")
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "{cmd} Exit Code: {}\nstdout: {}, stderr: {}",
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

        let _ = Self::command(&["vgremove", self.name.as_str(), "-y"]);
        let _ = Self::command(&["losetup", "-d", self.dev_loop.as_str()]);
    }
}
