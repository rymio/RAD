use anyhow::{Result, bail};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Same sandboxing model as rymsh's own virtual computer: every path is
/// clamped so it can never resolve outside `root`, giving each web session
/// a safe per-user directory instead of any access to the server's real
/// filesystem.
pub struct VirtualComputer {
    pub root: PathBuf,
    pub cwd: PathBuf,
}

impl VirtualComputer {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let home = root.join("home");
        fs::create_dir_all(&home)?;
        Ok(Self { root, cwd: home })
    }

    pub fn display_path(&self) -> String {
        let rel = self.cwd.strip_prefix(&self.root).unwrap_or(&self.cwd);
        if rel.as_os_str().is_empty() {
            "/".to_string()
        } else {
            format!("/{}", rel.display())
        }
    }

    fn resolve(&self, input: &str) -> PathBuf {
        let base = if input.starts_with('/') {
            self.root.clone()
        } else {
            self.cwd.clone()
        };
        let mut result = base;
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

    pub fn cd(&mut self, input: &str) -> Result<()> {
        let target = self.resolve(input);
        if !target.is_dir() {
            bail!("not a directory: {input}");
        }
        self.cwd = target;
        Ok(())
    }

    pub fn ls(&self, input: Option<&str>) -> Result<Vec<String>> {
        let target = match input {
            Some(input) => self.resolve(input),
            None => self.cwd.clone(),
        };
        if !target.is_dir() {
            bail!("not a directory: {}", input.unwrap_or("."));
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&target)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if metadata.is_dir() {
                entries.push(format!("{name}/"));
            } else {
                entries.push(format!("{name}  ({} bytes)", metadata.len()));
            }
        }
        entries.sort();
        Ok(entries)
    }

    pub fn pwd(&self) -> String {
        self.display_path()
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
                bail!("{input} is a directory; use `rm -r {input}`");
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

    pub fn cat(&self, input: &str) -> Result<String> {
        Ok(fs::read_to_string(self.resolve(input))?)
    }

    pub fn write(&self, input: &str, contents: &str) -> Result<()> {
        let target = self.resolve(input);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, contents)?;
        Ok(())
    }
}
