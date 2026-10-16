use anyhow::{Result, bail};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Same sandboxing model as rymsh's own virtual computer: every path is
/// clamped so it can never resolve outside `root`, giving the desktop a
/// safe per-account directory instead of any access to the server's real
/// filesystem.
pub struct VirtualComputer {
    pub root: PathBuf,
}

pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

impl VirtualComputer {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn resolve(&self, input: &str) -> PathBuf {
        let mut result = self.root.clone();
        for component in Path::new(input.trim_start_matches('/')).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir if result != self.root => {
                    result.pop();
                }
                Component::ParentDir => {}
                Component::Normal(part) => result.push(part),
                _ => {}
            }
        }
        result
    }

    /// The real filesystem path an absolute vfs path resolves to -- e.g.
    /// for handing a directory to something that needs a real `Path`,
    /// like `rymnet_site::upload_site`. Same sandboxing as every other
    /// method here; this doesn't grant anything `resolve` didn't already
    /// clamp.
    pub fn real_path(&self, input: &str) -> PathBuf {
        self.resolve(input)
    }

    /// The inverse of `real_path`: given an absolute real filesystem path,
    /// returns the vfs-relative path it corresponds to, if it's actually
    /// inside this vfs's root. Used to turn a `file://` URI captured while
    /// editing (Writer's image references) back into something
    /// `cp`/`write_string` can address.
    pub fn vfs_path_for(&self, real: &Path) -> Option<String> {
        let relative = real.strip_prefix(&self.root).ok()?;
        Some(format!("/{}", relative.to_string_lossy()))
    }

    /// Directory listing with type/size info, sorted directories-first --
    /// what the File Manager app renders both its tree and list panes
    /// from. `input` is always an absolute vfs path (e.g. "/", "/photos").
    pub fn list_entries(&self, input: &str) -> Result<Vec<DirEntry>> {
        let target = self.resolve(input);
        if !target.is_dir() {
            bail!("not a directory: {input}");
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&target)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
            });
        }
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        Ok(entries)
    }

    pub fn mkdir(&self, input: &str) -> Result<()> {
        fs::create_dir_all(self.resolve(input))?;
        Ok(())
    }

    pub fn touch(&self, input: &str) -> Result<()> {
        let target = self.resolve(input);
        if !target.exists() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, b"")?;
        }
        Ok(())
    }

    pub fn rm(&self, input: &str, recursive: bool) -> Result<()> {
        let target = self.resolve(input);
        if target == self.root {
            bail!("refusing to remove the virtual computer root");
        }
        if target.is_dir() {
            if !recursive {
                bail!("{input} is a directory; use recursive delete");
            }
            fs::remove_dir_all(&target)?;
        } else {
            fs::remove_file(&target)?;
        }
        Ok(())
    }

    pub fn cp(&self, src: &str, dst: &str) -> Result<()> {
        let src_path = self.resolve(src);
        let dst_path = self.resolve(dst);
        if src_path.is_dir() {
            for entry in walkdir::WalkDir::new(&src_path) {
                let entry = entry?;
                let relative = entry.path().strip_prefix(&src_path)?;
                let target = dst_path.join(relative);
                if entry.file_type().is_dir() {
                    fs::create_dir_all(&target)?;
                } else {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(entry.path(), &target)?;
                }
            }
            Ok(())
        } else {
            if let Some(parent) = dst_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src_path, &dst_path)?;
            Ok(())
        }
    }

    pub fn mv(&self, src: &str, dst: &str) -> Result<()> {
        let src_path = self.resolve(src);
        let dst_path = self.resolve(dst);
        if let Some(parent) = dst_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&src_path, &dst_path)?;
        Ok(())
    }

    pub fn read_to_string(&self, input: &str) -> Result<String> {
        Ok(fs::read_to_string(self.resolve(input))?)
    }

    pub fn write_string(&self, input: &str, contents: &str) -> Result<()> {
        let target = self.resolve(input);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, contents)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> VirtualComputer {
        let dir = std::env::temp_dir().join(format!("rymwin-vfs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        VirtualComputer::open(dir).unwrap()
    }

    #[test]
    fn crud_round_trip() {
        let vfs = scratch();

        vfs.mkdir("/photos").unwrap();
        vfs.touch("/photos/beach.jpg").unwrap();
        vfs.touch("/notes.txt").unwrap();

        let root = vfs.list_entries("/").unwrap();
        assert_eq!(root.len(), 2);
        assert!(root[0].is_dir && root[0].name == "photos", "dirs sort first");
        assert_eq!(root[1].name, "notes.txt");

        let photos = vfs.list_entries("/photos").unwrap();
        assert_eq!(photos.len(), 1);
        assert_eq!(photos[0].name, "beach.jpg");

        vfs.mv("/notes.txt", "/todo.txt").unwrap();
        assert!(vfs.list_entries("/").unwrap().iter().any(|e| e.name == "todo.txt"));

        vfs.cp("/photos", "/photos-backup").unwrap();
        assert_eq!(vfs.list_entries("/photos-backup").unwrap().len(), 1);

        vfs.rm("/photos-backup", true).unwrap();
        assert!(!vfs.list_entries("/").unwrap().iter().any(|e| e.name == "photos-backup"));

        // sandbox escape must never resolve outside root
        assert!(vfs.list_entries("/../../..").is_err() || vfs.resolve("/../../..") == vfs.root);
    }

    #[test]
    fn refuses_to_remove_root() {
        let vfs = scratch();
        assert!(vfs.rm("/", true).is_err());
    }
}
