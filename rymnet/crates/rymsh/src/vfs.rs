use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

const WELCOME_TEXT: &str = "Welcome to your Rymnet virtual computer.\n\n\
This is local storage for your rymsh session. Try:\n  \
ls, cd, pwd, mkdir, touch, rm, cp, mv, cat, nano <file>\n  \
lynx <domain.rym>  (browse a published rymnet site)\n  \
login <username> <password>  (fetch your synced files from the network)\n  \
help\n";

/// A sandboxed home directory rooted at `root`. All paths are resolved
/// relative to `root` and clamped so `cd ..` can never escape it, giving the
/// shell a safe "virtual computer" instead of free rein over the real disk.
pub struct VirtualComputer {
    pub root: PathBuf,
    pub cwd: PathBuf,
}

impl VirtualComputer {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("create virtual computer root {}", root.display()))?;
        let home = root.join("home");
        fs::create_dir_all(&home).with_context(|| format!("create {}", home.display()))?;
        let welcome = home.join("welcome.txt");
        if !welcome.exists() {
            fs::write(&welcome, WELCOME_TEXT)?;
        }
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
        for entry in fs::read_dir(&target).with_context(|| format!("read {}", target.display()))? {
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
        let target = self.resolve(input);
        fs::create_dir_all(&target).with_context(|| format!("mkdir {}", target.display()))
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
            fs::remove_dir_all(&target).with_context(|| format!("rm -r {}", target.display()))
        } else {
            fs::remove_file(&target).with_context(|| format!("rm {}", target.display()))
        }
    }

    pub fn cp(&self, src: &str, dst: &str) -> Result<()> {
        let src_path = self.resolve(src);
        let dst_path = self.resolve(dst);
        if src_path.is_dir() {
            for entry in WalkDir::new(&src_path) {
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
            fs::copy(&src_path, &dst_path)
                .with_context(|| format!("cp {src} {dst}"))
                .map(|_| ())
        }
    }

    pub fn mv(&self, src: &str, dst: &str) -> Result<()> {
        let src_path = self.resolve(src);
        let dst_path = self.resolve(dst);
        if let Some(parent) = dst_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&src_path, &dst_path).with_context(|| format!("mv {src} {dst}"))
    }

    pub fn cat(&self, input: &str) -> Result<String> {
        let target = self.resolve(input);
        fs::read_to_string(&target).with_context(|| format!("cat {}", target.display()))
    }

    pub fn real_path(&self, input: &str) -> PathBuf {
        self.resolve(input)
    }
}
