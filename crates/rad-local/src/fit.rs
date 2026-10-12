//! "Will it fit?" checks before a model is downloaded or loaded.

use std::path::Path;
use std::sync::OnceLock;

use sysinfo::{Disks, System};

use crate::{LocalError, ModelSpec, Result};

/// Room for llama.cpp's compute buffers and batches on top of weights and KV cache.
const OVERHEAD: u64 = 384 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitLevel {
    /// Uses at most half of this machine's memory.
    Ok,
    /// Runs, but leaves little room for other apps.
    Tight,
    /// Would push the machine into swap.
    TooBig,
}

impl FitLevel {
    pub fn of(needed: u64, total: u64) -> FitLevel {
        if total == 0 || needed <= total / 2 {
            FitLevel::Ok
        } else if needed <= total / 4 * 3 {
            FitLevel::Tight
        } else {
            FitLevel::TooBig
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FitLevel::Ok => "ok",
            FitLevel::Tight => "tight",
            FitLevel::TooBig => "too_big",
        }
    }
}

/// Memory a model needs once loaded: weights (+10%), its KV cache at RAD's context size, overhead.
pub fn memory_needed(spec: &ModelSpec) -> u64 {
    spec.bytes + spec.bytes / 10 + spec.kv_bytes_per_token * spec.ctx as u64 + OVERHEAD
}

/// Physical memory of this machine (0 if unknown).
pub fn total_memory() -> u64 {
    static TOTAL: OnceLock<u64> = OnceLock::new();
    *TOTAL.get_or_init(|| {
        let mut sys = System::new();
        sys.refresh_memory();
        sys.total_memory()
    })
}

/// Refuses to load a model this machine can't hold; warns when memory is merely busy right now.
pub fn check_memory(spec: &ModelSpec) -> Result<()> {
    let needed = memory_needed(spec);
    let total = total_memory();
    if FitLevel::of(needed, total) == FitLevel::TooBig {
        return Err(LocalError::DoesNotFit(format!(
            "{} needs about {} of memory, too much for this machine ({} total) — choose a smaller model",
            spec.name,
            human(needed),
            human(total)
        )));
    }
    let mut sys = System::new();
    sys.refresh_memory();
    let available = sys.available_memory();
    if available != 0 && available < needed {
        tracing::warn!("{} needs about {} but only {} is free right now — expect slowdowns", spec.name, human(needed), human(available));
    }
    Ok(())
}

/// Free space on the disk holding `dir` (the longest matching mount point wins).
pub fn disk_free(dir: &Path) -> Option<u64> {
    let dir = std::path::absolute(dir).ok()?;
    let disks = Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

pub fn human(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= 1024.0 * MB {
        format!("{:.1} GB", b / (1024.0 * MB))
    } else {
        format!("{:.0} MB", b / MB)
    }
}
