use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

/// Draws the tray glyph (a ring with a four-point spark) so it's crisp on every OS.
fn glyph(size: u32) -> Image<'static> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 / 2.0;
    let r_out = size as f32 * 0.46;
    let r_in = size as f32 * 0.36;
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5 - c;
            let fy = y as f32 + 0.5 - c;
            let d = (fx * fx + fy * fy).sqrt();
            // ring with soft edges
            let ring = ((r_out - d).min(d - r_in) + 0.5).clamp(0.0, 1.0);
            // spark: |x|^0.5 + |y|^0.5 <= k
            let k = (size as f32 * 0.27).sqrt();
            let s = fx.abs().sqrt() + fy.abs().sqrt();
            let spark = ((k - s) * 2.0 + 0.5).clamp(0.0, 1.0);
            let a = ring.max(spark);
            let i = ((y * size + x) * 4) as usize;
            #[cfg(target_os = "macos")]
            let col = 0u8; // template image: black, macOS tints it
            #[cfg(not(target_os = "macos"))]
            let col = 255u8;
            px[i] = col;
            px[i + 1] = col;
            px[i + 2] = col;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    Image::new_owned(px, size, size)
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let quick = MenuItem::with_id(app, "quick", "Quick Ask…", true, None::<&str>)?;
    let shot = MenuItem::with_id(app, "screenshot", "Capture Screenshot", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open RAD", true, None::<&str>)?;
    let lock = MenuItem::with_id(app, "lock", "Lock RAD", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit RAD", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&quick, &shot, &sep, &open, &settings, &lock, &sep2, &quit])?;

    TrayIconBuilder::with_id("rad")
        .icon(glyph(44))
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("RAD")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "quick" => crate::toggle_quick(app),
            "screenshot" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    match crate::capture::capture_and_upload(&app, None).await {
                        Ok(Some(_)) => crate::notify(&app, "Screenshot saved", "Added to your RAD project and indexing now."),
                        Ok(None) => {}
                        Err(e) => crate::notify(&app, "Screenshot failed", &e.message),
                    }
                });
            }
            "open" => crate::show_main(app),
            "settings" => {
                crate::show_main(app);
                let _ = tauri::Emitter::emit(app, "rad-navigate", "settings");
            }
            "lock" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { crate::commands::do_lock(&app, false).await });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                crate::toggle_quick(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
