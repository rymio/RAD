//! Screenshots straight into the vault (no file written to disk).

use std::io::Cursor;

use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::client::{CmdError, CmdResult};
use crate::App;

#[cfg(target_os = "macos")]
async fn grab() -> CmdResult<Option<Vec<u8>>> {
    // Interactive region selection with the system tool, delivered via the clipboard.
    let before = arboard::Clipboard::new().ok().and_then(|mut c| c.get_image().ok()).map(|i| blake3::hash(&i.bytes));
    let status = tokio::process::Command::new("/usr/sbin/screencapture")
        .args(["-i", "-c", "-x"])
        .status()
        .await
        .map_err(|e| CmdError::new("capture", e.to_string()))?;
    if !status.success() {
        return Ok(None);
    }
    let img = tokio::task::spawn_blocking(|| arboard::Clipboard::new().ok().and_then(|mut c| c.get_image().ok()))
        .await
        .map_err(|e| CmdError::new("capture", e.to_string()))?;
    let Some(img) = img else { return Ok(None) };
    if Some(blake3::hash(&img.bytes)) == before {
        return Ok(None); // cancelled: clipboard unchanged
    }
    encode(img.width as u32, img.height as u32, img.bytes.into_owned()).map(Some)
}

#[cfg(not(target_os = "macos"))]
async fn grab() -> CmdResult<Option<Vec<u8>>> {
    tokio::task::spawn_blocking(|| {
        let monitors = xcap::Monitor::all().map_err(|e| CmdError::new("capture", e.to_string()))?;
        let m = monitors
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .or(monitors.first())
            .ok_or_else(|| CmdError::new("capture", "no display found"))?;
        let img = m.capture_image().map_err(|e| CmdError::new("capture", e.to_string()))?;
        let (w, h) = (img.width(), img.height());
        encode(w, h, img.into_raw()).map(Some)
    })
    .await
    .map_err(|e| CmdError::new("capture", e.to_string()))?
}

fn encode(w: u32, h: u32, rgba: Vec<u8>) -> CmdResult<Vec<u8>> {
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| CmdError::new("capture", "bad image data"))?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| CmdError::new("capture", e.to_string()))?;
    Ok(out.into_inner())
}

/// Captures and uploads. Returns `None` when the user cancelled the selection.
pub async fn capture_and_upload(app: &AppHandle, project_id: Option<i64>) -> CmdResult<Option<Value>> {
    let st = app.state::<App>();
    if !st.client.has_session() {
        crate::show_main(app);
        return Err(CmdError::new("session_required", "Unlock RAD first"));
    }
    let project = match project_id.or(st.config().screenshot_project_id) {
        Some(p) => p,
        None => {
            let projects = st.client.json(reqwest::Method::GET, "/v1/projects", None).await?;
            projects
                .as_array()
                .and_then(|a| a.iter().find(|p| p["archived"] == false))
                .and_then(|p| p["id"].as_i64())
                .ok_or_else(|| CmdError::new("no_project", "Create a project first"))?
        }
    };
    let Some(png) = grab().await? else { return Ok(None) };
    let name = format!("Screenshot {}.png", chrono_like_now());
    let v = crate::commands::upload_bytes(app, project, &name, "screenshot", png).await?;
    Ok(Some(v))
}

fn chrono_like_now() -> String {
    // Local time without pulling in chrono: YYYY-MM-DD at HH.MM.SS (UTC offset ignored if unknown).
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}.{:02}.{:02}", rem / 3600, (rem / 60) % 60, rem % 60)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
