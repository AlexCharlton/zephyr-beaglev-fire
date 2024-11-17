use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::Command;

pub fn list_removable_drives() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
        use windows::Win32::System::WindowsProgramming::DRIVE_REMOVABLE;

        let mut drives = Vec::new();
        let bitmask = unsafe { GetLogicalDrives() };

        for i in 0..26 {
            if (bitmask & (1 << i)) != 0 {
                let drive_letter = char::from(b'A' + i as u8);
                let path = format!("{}:\\", drive_letter);

                // Convert to wide string for Windows API
                let wide_path: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

                // Check if drive is removable
                let drive_type = unsafe { GetDriveTypeW(PCWSTR::from_raw(wide_path.as_ptr())) };
                if drive_type == DRIVE_REMOVABLE {
                    drives.push(PathBuf::from(path));
                }
            }
        }
        drives
    }

    #[cfg(not(windows))]
    {
        use sysinfo::Disks;

        let mut sys = System::new_all();
        sys.refresh_disks_list();

        sys.disks()
            .iter()
            .filter(|disk| disk.is_removable())
            .map(|disk| disk.mount_point().to_string_lossy().into_owned())
            .collect()
    }
}

pub fn eject_drive(mount_point: &str) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // On Linux, we can use the 'eject' command
        Command::new("eject").arg(mount_point).output().map(|_| ())
    }

    #[cfg(target_os = "macos")]
    {
        // On macOS, we use 'diskutil eject'
        Command::new("diskutil")
            .args(["eject", mount_point])
            .output()
            .map(|_| ())
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, we need to use PowerShell to safely eject
        Command::new("powershell")
            .args([
                "-Command",
                &format!(
                    "($driveEject = New-Object -comObject Shell.Application).Namespace(17).ParseName('{}').InvokeVerb('Eject')",
                    mount_point
                ),
            ])
            .output()
            .map(|_| ())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Ejecting drives is not supported on this operating system",
        ))
    }
}

pub fn flash_image_to_drive(image_path: &str, target_drive: &str) -> io::Result<()> {
    // Open the source image file
    let mut source = File::open(image_path)?;

    // Open the target drive with write permissions
    #[cfg(unix)]
    let target_path = format!("/dev/{}", target_drive);
    #[cfg(windows)]
    let target_path = format!(r"\\.\{}", target_drive); // Raw device access format

    let mut target = File::options().write(true).open(&target_path)?;

    // Seek to the beginning of both files
    source.seek(SeekFrom::Start(0))?;
    target.seek(SeekFrom::Start(0))?;

    // Use a buffer for efficient copying
    let mut buffer = [0; 1024 * 1024]; // 1MB buffer
    loop {
        let bytes_read = source.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        target.write_all(&buffer[..bytes_read])?;
        target.flush()?; // Ensure data is written to disk
    }

    Ok(())
}
