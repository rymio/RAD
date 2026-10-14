//! Where a vault file lives, and whether several computers can safely open it at once.
//!
//! SQLite (and so the vault) relies on file locks to let several processes share one file.
//! Network shares (SMB, NFS, AFP) pass those locks to the server, so several computers can
//! use the vault together. Sync services (iCloud Drive, Dropbox, OneDrive, Google Drive…)
//! don't: each computer edits its own copy and the service later picks one or makes a
//! "conflicted copy". WebDAV has no locks at all.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationKind {
    /// A disk in this computer (or an external drive).
    Local,
    /// A network share with working file locks: several computers may use the vault at once.
    Network,
    /// A sync service or a lockless network filesystem: one computer at a time only.
    Synced,
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub kind: LocationKind,
    /// For people: "iCloud Drive", "Network share (smbfs)", "This computer"…
    pub label: String,
}

/// Folder names used by sync clients, matched against each path component.
const SYNC_FOLDERS: &[(&str, &str)] = &[
    ("Mobile Documents", "iCloud Drive"),
    ("iCloud Drive", "iCloud Drive"),
    ("iCloudDrive", "iCloud Drive"),
    ("CloudStorage", "a cloud storage folder"),
    ("Dropbox", "Dropbox"),
    ("Google Drive", "Google Drive"),
    ("My Drive", "Google Drive"),
    ("Box", "Box"),
    ("Box Sync", "Box"),
    ("pCloud Drive", "pCloud"),
    ("MEGA", "MEGA"),
    ("Nextcloud", "Nextcloud"),
    ("ownCloud", "ownCloud"),
    ("Tresorit", "Tresorit"),
];

pub fn classify(path: &Path) -> Location {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    for comp in abs.components() {
        let c = comp.as_os_str().to_string_lossy();
        if c.starts_with("OneDrive") {
            return synced("OneDrive");
        }
        if let Some((_, name)) = SYNC_FOLDERS.iter().find(|(f, _)| c == *f) {
            // ~/Library/CloudStorage/<Provider-account> names the provider.
            if c == "CloudStorage" {
                return synced(&provider_from_cloudstorage(&abs).unwrap_or_else(|| name.to_string()));
            }
            return synced(name);
        }
    }
    match fs_type(abs.parent().unwrap_or(&abs)) {
        Some(t) if matches!(t.as_str(), "webdav" | "davfs" | "fuse.davfs2") => synced("WebDAV (no file locking)"),
        Some(t) if matches!(t.as_str(), "smbfs" | "cifs" | "smb2" | "smb3" | "nfs" | "nfs4" | "afpfs" | "remote") => {
            Location { kind: LocationKind::Network, label: format!("Network share ({t})") }
        }
        _ => Location { kind: LocationKind::Local, label: "This computer".into() },
    }
}

fn synced(name: &str) -> Location {
    Location { kind: LocationKind::Synced, label: name.to_string() }
}

fn provider_from_cloudstorage(abs: &Path) -> Option<String> {
    let mut it = abs.components().map(|c| c.as_os_str().to_string_lossy().into_owned());
    it.find(|c| c == "CloudStorage")?;
    let next = it.next()?;
    let p = next.split('-').next().unwrap_or(&next);
    Some(match p {
        "GoogleDrive" => "Google Drive".into(),
        "OneDrive" => "OneDrive".into(),
        other => other.to_string(),
    })
}

/// The filesystem type of the volume holding `dir`, lower-case (e.g. "apfs", "smbfs", "nfs").
#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
fn fs_type(dir: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(st.f_fstypename.as_ptr()) };
    Some(name.to_string_lossy().to_lowercase())
}

#[cfg(target_os = "linux")]
fn fs_type(dir: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(
        match st.f_type as u32 {
            0x6969 => "nfs",
            0x517B => "smbfs",
            0xFF53_4D42 => "cifs",
            0xFE53_4D42 => "smb2",
            0x65735546 => "fuse",
            _ => "local",
        }
        .to_string(),
    )
}

#[cfg(windows)]
fn fs_type(dir: &Path) -> Option<String> {
    let s = dir.to_string_lossy();
    if s.starts_with(r"\\") && !s.starts_with(r"\\?\") || s.starts_with(r"\\?\UNC\") {
        return Some("remote".into());
    }
    // A mapped network drive (e.g. Z:).
    let root: Vec<u16> = format!("{}\\", &s.get(..2)?).encode_utf16().chain(std::iter::once(0)).collect();
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOTE: u32 = 4;
    (unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE).then(|| "remote".into())
}

#[cfg(not(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd", target_os = "linux", windows)))]
fn fs_type(_dir: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_folders_are_recognised() {
        let k = |p: &str| classify(Path::new(p)).kind;
        assert_eq!(k("/Users/a/Library/Mobile Documents/com~apple~CloudDocs/RAD/x.radvault"), LocationKind::Synced);
        assert_eq!(classify(Path::new("/Users/a/Library/Mobile Documents/com~apple~CloudDocs/x.radvault")).label, "iCloud Drive");
        assert_eq!(classify(Path::new("/Users/a/Library/CloudStorage/GoogleDrive-a@b.c/My Drive/x.radvault")).label, "Google Drive");
        assert_eq!(k("/Users/a/Dropbox/x.radvault"), LocationKind::Synced);
        assert_eq!(k("C:/Users/a/OneDrive - Contoso/x.radvault"), LocationKind::Synced);
        assert_eq!(k("/tmp/x.radvault"), LocationKind::Local);
    }
}
