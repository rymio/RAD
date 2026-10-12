//! Device-local settings (never content): server URL, hotkeys, window behaviour, theme.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// "server" (connect to a rad-server) or "local" (standalone: vault on this computer).
    pub mode: String,
    /// Standalone mode: the vault file this app opens.
    pub local_vault: Option<String>,
    pub recent_vaults: Vec<String>,
    pub server_url: Option<String>,
    pub server_name: Option<String>,
    pub device_name: String,
    /// Minutes of inactivity before this device locks (0 = never).
    pub auto_lock_minutes: u32,
    pub lock_on_sleep: bool,
    pub close_to_tray: bool,
    pub hide_dock_icon: bool,
    pub hotkey_quick_ask: String,
    pub hotkey_screenshot: String,
    pub hotkey_lock: String,
    pub screenshot_project_id: Option<i64>,
    pub theme: String,
    pub accent: String,
    pub tray_hint_shown: bool,
    pub launched_before: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            mode: "server".into(),
            local_vault: None,
            recent_vaults: Vec::new(),
            server_url: None,
            server_name: None,
            device_name: hostname::get().ok().and_then(|h| h.into_string().ok()).unwrap_or_else(|| "My computer".into()),
            auto_lock_minutes: 30,
            lock_on_sleep: true,
            close_to_tray: true,
            hide_dock_icon: false,
            hotkey_quick_ask: "Alt+Space".into(),
            hotkey_screenshot: "Alt+Shift+S".into(),
            hotkey_lock: "Alt+Shift+L".into(),
            screenshot_project_id: None,
            theme: "system".into(),
            accent: "#7c6cf2".into(),
            tray_hint_shown: false,
            launched_before: false,
        }
    }
}

pub fn path(dir: &std::path::Path) -> PathBuf {
    dir.join("rad-client.json")
}

pub fn load(dir: &std::path::Path) -> ClientConfig {
    std::fs::read(path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(dir: &std::path::Path, cfg: &ClientConfig) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("rad-client.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(cfg)?)?;
    std::fs::rename(tmp, path(dir))
}
