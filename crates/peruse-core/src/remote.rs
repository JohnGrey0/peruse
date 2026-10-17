//! Finds out whether a path is on a network drive, and how much free space a
//! local disk has.
//!
//! DuckDB reads a Parquet file with many small reads at different positions.
//! On a local disk each read costs some microseconds. On a network drive each
//! read also waits for the network, and a scan of a large file then takes much
//! longer than one long read of the same bytes. The module [`crate::cache`]
//! therefore copies a file from a network drive to the local disk one time,
//! and this module tells it which files are on a network drive.
//!
//! | System | The test |
//! |---|---|
//! | Windows | A path `\\server\share`, or a drive letter that `GetDriveTypeW` reports as `DRIVE_REMOTE` |
//! | Linux | The type of the file system in `/proc/self/mountinfo`, such as `nfs4`, `cifs` or `fuse.sshfs` |
//! | macOS | The flag `MNT_LOCAL` of `statfs`, and the name of the file system |
//!
//! This is the one module of Peruse that calls the operating system directly,
//! because the standard library gives neither answer. Each call is small, and
//! a comment above each one says why it is safe. A failed call gives the
//! answer "local", and Peruse then reads the file in place, as it always did.

use std::path::Path;

/// What Peruse asks about the disks.
///
/// The type [`System`] asks the operating system. A test gives a fake that
/// treats a local directory as a network drive.
pub trait Probe {
    /// Gives `true` when the path is on a network drive.
    fn is_remote(&self, path: &Path) -> bool;
    /// Gives the bytes that the user can still write to the disk that holds
    /// the directory, or `None` when the system does not say.
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
}

/// The probe that asks the operating system.
#[derive(Clone, Copy, Debug, Default)]
pub struct System;

impl Probe for System {
    fn is_remote(&self, path: &Path) -> bool {
        sys::is_remote(path)
    }

    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        // The directory can be one that does not exist yet. Ask about the
        // nearest directory above it that does.
        let mut at = Some(dir);
        while let Some(d) = at {
            if d.is_dir() {
                return sys::free_bytes(d);
            }
            at = d.parent();
        }
        None
    }
}

/// Gives `true` for the name of a file system that reads over a network.
///
/// The names are the names that Linux writes in `/proc/self/mountinfo` and
/// that macOS writes in `f_fstypename`. A FUSE file system has the name
/// `fuse.<program>`, and most programs of that kind are local, so only the
/// network programs are in the list.
///
/// The names `9p` and `drvfs` are the drives of Windows inside WSL. A read
/// through them crosses from the virtual machine to Windows, and it is slow in
/// the same way as a network drive.
pub fn is_network_fs(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    const NETWORK: &[&str] = &[
        "nfs",
        "nfs4",
        "cifs",
        "smb",
        "smb2",
        "smb3",
        "smbfs",
        "ncpfs",
        "afs",
        "afpfs",
        "webdav",
        "davfs",
        "9p",
        "drvfs",
        "ceph",
        "glusterfs",
        "lustre",
        "gpfs",
        "beegfs",
        "orangefs",
        "ocfs2",
        "coda",
    ];
    const NETWORK_FUSE: &[&str] = &[
        "sshfs",
        "rclone",
        "s3fs",
        "gcsfuse",
        "goofys",
        "davfs2",
        "curlftpfs",
        "juicefs",
        "glusterfs",
        "cephfs",
        "ceph-fuse",
        "smbnetfs",
        "afpfs",
        "blobfuse",
        "blobfuse2",
        "mountpoint-s3",
    ];
    if let Some(program) = name.strip_prefix("fuse.") {
        return NETWORK_FUSE.contains(&program);
    }
    NETWORK.contains(&name.as_str())
}

/// Gives `true` when the text is a Windows path to a share, such as
/// `\\server\share\data.parquet` or `\\?\UNC\server\share\data.parquet`.
///
/// The forms `\\?\C:\` and `\\.\` name a local disk and a device, and they
/// give `false`. The function also accepts forward slashes, because DuckDB and
/// a user both write them.
pub fn is_unc(text: &str) -> bool {
    let t = text.replace('/', "\\");
    if t.get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("\\\\?\\UNC\\"))
    {
        return true;
    }
    if t.starts_with("\\\\?\\") || t.starts_with("\\\\.\\") {
        return false;
    }
    // Two slashes and then the name of a server.
    t.starts_with("\\\\") && t[2..].chars().next().is_some_and(|c| c != '\\')
}

/// Finds the type of the file system that holds a path, from the text of
/// `/proc/self/mountinfo`.
///
/// Each line of that file describes one mount. The fifth field is the mount
/// point, and the field after the separator `-` is the type. The mount with
/// the longest mount point that holds the path is the one that holds the
/// file. The path must be a complete path with no symbolic link in it.
pub fn mount_type(mountinfo: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let Some(sep) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        let (Some(point), Some(fstype)) = (fields.get(4), fields.get(sep + 1)) else {
            continue;
        };
        let point = unescape_mount(point);
        if !path.starts_with(&point) {
            continue;
        }
        let len = point.len();
        if best.as_ref().is_none_or(|(l, _)| len >= *l) {
            best = Some((len, fstype.to_string()));
        }
    }
    best.map(|(_, t)| t)
}

/// Changes the escape sequences of a mount point back into characters.
///
/// The kernel writes a space as `\040`, a tab as `\011`, a new line as `\012`
/// and a backslash as `\134`.
fn unescape_mount(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let v = (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Gives the complete form of a path, with each symbolic link followed when
/// the file exists.
fn full_path(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(windows)]
mod sys {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    /// The value of `GetDriveTypeW` for a drive on the network.
    const DRIVE_REMOTE: u32 = 4;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }

    /// Gives the text as UTF-16 with a zero at the end, for a call to Windows.
    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn is_remote(path: &Path) -> bool {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let text = abs.to_string_lossy();
        if super::is_unc(&text) {
            return true;
        }
        // A link or a junction can point to a share. The complete form of the
        // path then starts with `\\?\UNC\`. For a drive letter that is mapped
        // to a share, it does too.
        if super::is_unc(&super::full_path(path).to_string_lossy()) {
            return true;
        }
        let t = text.trim_start_matches("\\\\?\\");
        let mut chars = t.chars();
        let (Some(letter), Some(':')) = (chars.next(), chars.next()) else {
            return false;
        };
        if !letter.is_ascii_alphabetic() {
            return false;
        }
        let root = wide(std::ffi::OsStr::new(&format!("{letter}:\\")));
        // SAFETY: `root` is a valid text of UTF-16 that ends with a zero, and
        // it lives until the call returns. The function only reads it.
        unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
    }

    pub fn free_bytes(dir: &Path) -> Option<u64> {
        let name = wide(dir.as_os_str());
        let mut free: u64 = 0;
        // SAFETY: `name` is a valid text of UTF-16 that ends with a zero.
        // `free` is a valid place for one 64-bit number, which is the layout
        // of ULARGE_INTEGER. Windows accepts a null pointer for the two
        // totals that Peruse does not need.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                name.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod sys {
    use std::path::Path;

    pub fn is_remote(path: &Path) -> bool {
        let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return false;
        };
        super::mount_type(&info, &super::full_path(path)).is_some_and(|t| super::is_network_fs(&t))
    }

    pub fn free_bytes(dir: &Path) -> Option<u64> {
        super::unix_free_bytes(dir)
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// The flag of `statfs` for a file system on a local disk.
    const MNT_LOCAL: u64 = 0x0000_1000;

    #[allow(clippy::unnecessary_cast)]
    pub fn is_remote(path: &Path) -> bool {
        let full = super::full_path(path);
        let Ok(c) = std::ffi::CString::new(full.as_os_str().as_bytes()) else {
            return false;
        };
        let mut buf = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        // SAFETY: `c` is a valid text that ends with a zero, and `buf` is a
        // valid place for one `statfs` structure. The function writes only
        // into `buf`.
        let rc = unsafe { libc::statfs(c.as_ptr(), buf.as_mut_ptr()) };
        if rc != 0 {
            return false;
        }
        // SAFETY: the call returned 0, so it filled the structure.
        let s = unsafe { buf.assume_init() };
        if (s.f_flags as u64) & MNT_LOCAL == 0 {
            return true;
        }
        let name: String = s
            .f_fstypename
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8 as char)
            .collect();
        super::is_network_fs(&name)
    }

    pub fn free_bytes(dir: &Path) -> Option<u64> {
        super::unix_free_bytes(dir)
    }
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos"
)))]
mod sys {
    use std::path::Path;

    /// This system has no test. Peruse reads each file in place, as before.
    pub fn is_remote(_path: &Path) -> bool {
        false
    }

    pub fn free_bytes(_dir: &Path) -> Option<u64> {
        #[cfg(unix)]
        return super::unix_free_bytes(_dir);
        #[cfg(not(unix))]
        None
    }
}

/// Gives the free bytes of the disk that holds the directory, with `statvfs`.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn unix_free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut buf = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a valid text that ends with a zero, and `buf` is a valid
    // place for one `statvfs` structure. The function writes only into `buf`.
    let rc = unsafe { libc::statvfs(c.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: the call returned 0, so it filled the structure.
    let s = unsafe { buf.assume_init() };
    Some((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_to_a_share_is_on_the_network() {
        assert!(is_unc(r"\\porsche\apps\data.parquet"));
        assert!(is_unc("//porsche/apps/data.parquet"));
        assert!(is_unc(r"\\?\UNC\porsche\apps\data.parquet"));
        assert!(is_unc(r"\\?\unc\porsche\apps"));
        // A local disk in the long form, and a device, are not shares.
        assert!(!is_unc(r"\\?\C:\data\x.parquet"));
        assert!(!is_unc(r"\\.\PhysicalDrive0"));
        assert!(!is_unc(r"C:\data\x.parquet"));
        assert!(!is_unc("/home/me/x.parquet"));
        assert!(!is_unc(r"\\"));
        assert!(!is_unc(r"\\\x"));
    }

    #[test]
    fn the_names_of_network_file_systems() {
        for n in [
            "nfs",
            "nfs4",
            "cifs",
            "smb3",
            "smbfs",
            "fuse.sshfs",
            "fuse.rclone",
            "9p",
            "afpfs",
            "webdav",
            "NFS4",
        ] {
            assert!(is_network_fs(n), "{n}");
        }
        // A FUSE program that reads a local disk is not a network.
        for n in [
            "ext4",
            "xfs",
            "btrfs",
            "apfs",
            "tmpfs",
            "overlay",
            "fuse.ntfs-3g",
            "fuseblk",
            "fuse.gocryptfs",
            "zfs",
        ] {
            assert!(!is_network_fs(n), "{n}");
        }
    }

    /// Lines in the form that Linux writes. The share has a space in its
    /// name, which the kernel writes as `\040`.
    const MOUNTINFO: &str = "\
22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
40 22 0:35 / /mnt/team\\040share rw,relatime shared:20 - cifs //nas/team rw,vers=3.1.1
41 22 0:36 / /mnt/team\\040share/local rw shared:21 - ext4 /dev/sdb1 rw
42 22 0:37 / /home/me/remote rw,nosuid shared:22 - fuse.sshfs me@host:/data rw
43 22 0:38 / /srv/nfs rw shared:23 - nfs4 server:/export rw";

    #[test]
    fn the_longest_mount_point_that_holds_the_path_gives_the_type() {
        let t = |p: &str| mount_type(MOUNTINFO, Path::new(p));
        assert_eq!(t("/home/me/data.csv").as_deref(), Some("ext4"));
        assert_eq!(t("/mnt/team share/q3.parquet").as_deref(), Some("cifs"));
        // A local disk inside the share wins, because its mount point is longer.
        assert_eq!(t("/mnt/team share/local/x.csv").as_deref(), Some("ext4"));
        assert_eq!(t("/home/me/remote/x.csv").as_deref(), Some("fuse.sshfs"));
        assert_eq!(t("/srv/nfs/a/b.parquet").as_deref(), Some("nfs4"));
        // The test compares whole parts of the path, and not letters: the
        // directory `/srv/nfsbackup` is not inside `/srv/nfs`.
        assert_eq!(t("/srv/nfsbackup/x").as_deref(), Some("ext4"));
    }

    #[test]
    fn a_mount_point_with_escapes_reads_back() {
        assert_eq!(unescape_mount(r"/mnt/a\040b\134c"), "/mnt/a b\\c");
        assert_eq!(unescape_mount(r"/plain"), "/plain");
        // A backslash with no three digits after it stays as it is.
        assert_eq!(unescape_mount(r"/odd\04"), r"/odd\04");
    }

    #[test]
    fn the_system_gives_an_answer_for_the_temporary_directory() {
        let tmp = std::env::temp_dir();
        assert!(
            !System.is_remote(&tmp),
            "the temporary directory is on a local disk"
        );
        let free = System.free_bytes(&tmp);
        assert!(free.is_some_and(|f| f > 0), "{free:?}");
        // A directory that does not exist yet gives the free space of the
        // directory above it.
        assert!(
            System
                .free_bytes(&tmp.join("peruse-no-such-dir").join("deeper"))
                .is_some()
        );
    }

    /// The drive `A:` of the machine that this module was written on is a
    /// share. On each other machine the test does nothing.
    #[cfg(windows)]
    #[test]
    fn a_mapped_drive_letter_is_on_the_network_when_the_machine_has_one() {
        let a = Path::new(r"A:\projects");
        if a.is_dir() && full_path(a).to_string_lossy().contains("UNC") {
            assert!(System.is_remote(a));
            assert!(System.is_remote(&a.join("no-such-file.parquet")));
        }
        assert!(!System.is_remote(Path::new(r"C:\Windows")));
    }
}
