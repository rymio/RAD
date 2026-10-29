//! RAD desktop client: a thin, secure shell around the RAD server API.

mod browser;
mod capture;
mod client;
mod commands;
mod config;
mod files;
mod mail;
mod media;
mod events;
mod protocol;
mod rymnet;
mod sandbox;
mod containers;
mod sidex;
mod games;
mod c64;
mod dos;
mod c64dev;
mod c64lab;
mod zx;
mod zxdev;
mod zxlab;
mod secrets;
mod terminal;
mod term_agent;
mod stt;
mod tray;
mod tts;
mod vpn;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub use client::{CmdError, CmdResult};

pub struct App {
    pub client: client::Client,
    pub cfg: Mutex<config::ClientConfig>,
    pub cfg_dir: PathBuf,
    pub last_activity: AtomicI64,
    pub chat_tasks: Mutex<HashMap<i64, tokio::task::AbortHandle>>,
    pub events_task: Mutex<Option<tokio::task::AbortHandle>>,
    /// Standalone mode: the in-process RAD server.
    pub embedded: tokio::sync::Mutex<Option<rad_server::embedded::Embedded>>,
}

/// A helper program's command (assemblers, the DOS engine): on Windows it gets no console
/// window of its own.
pub(crate) fn helper_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl App {
    pub fn config(&self) -> config::ClientConfig {
        self.cfg.lock().unwrap().clone()
    }
    pub fn save_config(&self, c: config::ClientConfig) {
        if let Err(e) = config::save(&self.cfg_dir, &c) {
            tracing::error!("saving config: {e}");
        }
        *self.cfg.lock().unwrap() = c;
    }
    pub fn touch(&self) {
        self.last_activity.store(now_ms(), Ordering::Relaxed);
    }
}

// ------------------------------------------------------------------ windows

/// The macOS menu bar: the standard menus, with Help → About RAD (the app's own About page).
#[cfg(target_os = "macos")]
fn app_menu(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind, HELP_SUBMENU_ID};
    let menu = Menu::default(app)?;
    if let Some(MenuItemKind::Submenu(help)) = menu.get(HELP_SUBMENU_ID) {
        help.append(&MenuItem::with_id(app, "about", "About RAD", true, None::<&str>)?)?;
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, ev| {
        if ev.id().as_ref() == "about" {
            show_main(app);
            let _ = app.emit_to("main", "show-about", ());
        }
    });
    Ok(())
}

pub fn main_window(app: &AppHandle) -> tauri::WebviewWindow {
    if let Some(w) = app.get_webview_window("main") {
        return w;
    }
    let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("RAD")
        .inner_size(1320.0, 860.0)
        .min_inner_size(940.0, 620.0)
        .visible(false);
    #[cfg(target_os = "macos")]
    {
        b = b.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true).transparent(true);
    }
    #[cfg(target_os = "windows")]
    {
        b = b.transparent(true);
    }
    let w = b.build().expect("main window");
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        let _ = apply_vibrancy(&w, NSVisualEffectMaterial::Sidebar, Some(NSVisualEffectState::FollowsWindowActiveState), None);
    }
    #[cfg(target_os = "windows")]
    {
        if window_vibrancy::apply_mica(&w, None).is_err() {
            let _ = window_vibrancy::apply_acrylic(&w, Some((18, 18, 24, 200)));
        }
    }
    let handle = app.clone();
    w.on_window_event(move |ev| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
            let st = handle.state::<App>();
            if st.config().close_to_tray {
                api.prevent_close();
                if let Some(w) = handle.get_webview_window("main") {
                    let _ = w.hide();
                }
                let mut cfg = st.config();
                if !cfg.tray_hint_shown {
                    cfg.tray_hint_shown = true;
                    st.save_config(cfg);
                    notify(&handle, "RAD is still running", "RAD keeps working in the menu bar / system tray. Quit from the tray menu.");
                }
            }
        }
    });
    w
}

pub fn show_main(app: &AppHandle) {
    let w = main_window(app);
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_focus();
}

pub fn quick_window(app: &AppHandle) -> tauri::WebviewWindow {
    if let Some(w) = app.get_webview_window("quick") {
        return w;
    }
    let mut b = WebviewWindowBuilder::new(app, "quick", WebviewUrl::App("index.html?view=quick".into()))
        .title("Quick Ask")
        .inner_size(760.0, 560.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .center()
        .visible(false);
    #[cfg(not(target_os = "linux"))]
    {
        b = b.transparent(true);
    }
    let w = b.build().expect("quick window");
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        let _ = apply_vibrancy(&w, NSVisualEffectMaterial::HudWindow, Some(NSVisualEffectState::Active), Some(18.0));
    }
    #[cfg(target_os = "windows")]
    {
        let _ = window_vibrancy::apply_acrylic(&w, Some((22, 22, 30, 210)));
    }
    let wc = w.clone();
    w.on_window_event(move |ev| {
        if let tauri::WindowEvent::Focused(false) = ev {
            let _ = wc.hide();
        }
    });
    w
}

pub fn toggle_quick(app: &AppHandle) {
    let w = quick_window(app);
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
    } else {
        let _ = w.center();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("rad-quick-open", ());
    }
}

pub fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

// ------------------------------------------------------------------ hotkeys

pub fn register_hotkeys(app: &AppHandle) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let cfg = app.state::<App>().config();
    for (keys, name) in [(&cfg.hotkey_quick_ask, "quick"), (&cfg.hotkey_screenshot, "screenshot"), (&cfg.hotkey_lock, "lock")] {
        if keys.trim().is_empty() {
            continue;
        }
        let name = name.to_string();
        let res = gs.on_shortcut(keys.as_str(), move |app, _sc, ev| {
            if ev.state() != ShortcutState::Pressed {
                return;
            }
            match name.as_str() {
                "quick" => toggle_quick(app),
                "screenshot" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        match capture::capture_and_upload(&app, None).await {
                            Ok(Some(_)) => notify(&app, "Screenshot saved", "Added to your RAD project and indexing now."),
                            Ok(None) => {}
                            Err(e) => notify(&app, "Screenshot failed", &e.message),
                        }
                    });
                }
                "lock" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move { commands::do_lock(&app, false).await });
                }
                _ => {}
            }
        });
        if let Err(e) = res {
            tracing::warn!("could not register hotkey {keys}: {e}");
        }
    }
}

// ------------------------------------------------------------------ idle / sleep auto-lock

fn spawn_idle_watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_tick = now_ms();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let now = now_ms();
            let st = app.state::<App>();
            let cfg = st.config();
            let slept = now - last_tick > 90_000;
            last_tick = now;
            if !st.client.has_session() {
                continue;
            }
            let idle = now - st.last_activity.load(Ordering::Relaxed);
            let idle_lock = cfg.auto_lock_minutes > 0 && idle > cfg.auto_lock_minutes as i64 * 60_000;
            if idle_lock || (slept && cfg.lock_on_sleep) {
                tracing::info!("auto-locking (idle={idle_lock}, slept={slept})");
                commands::do_lock(&app, false).await;
            }
        }
    });
}

// ------------------------------------------------------------------ entry point

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,llama-cpp-2=warn".into()))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            show_main(app);
            let files: Vec<String> = argv.iter().skip(1).filter(|a| !a.starts_with('-') && std::path::Path::new(a).is_file()).cloned().collect();
            if !files.is_empty() {
                let _ = app.emit("rad-open-files", files);
            }
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["quick"])
                .with_state_flags(tauri_plugin_window_state::StateFlags::SIZE | tauri_plugin_window_state::StateFlags::POSITION | tauri_plugin_window_state::StateFlags::MAXIMIZED)
                .build(),
        )
        .register_asynchronous_uri_scheme_protocol("rad", protocol::handle)
        .setup(|app| {
            let cfg_dir = app.path().app_config_dir().expect("config dir");
            let cfg = config::load(&cfg_dir);
            let client = client::Client::new();
            if cfg.mode != "local" {
                if let Some(url) = &cfg.server_url {
                    client.set_server(Some(url.clone()), secrets::get(&cfg_dir, url));
                }
            }
            #[cfg(target_os = "macos")]
            if cfg.hide_dock_icon {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            app.manage(App {
                client,
                cfg: Mutex::new(cfg.clone()),
                cfg_dir,
                last_activity: AtomicI64::new(now_ms()),
                chat_tasks: Mutex::new(HashMap::new()),
                events_task: Mutex::new(None),
                embedded: tokio::sync::Mutex::new(None),
            });
            app.manage(browser::Browsers::default());
            app.manage(terminal::Terminals::default());
            app.manage(term_agent::Agents::default());
            app.manage(tts::Tts::default());
            app.manage(stt::Stt::default());
            app.manage(sandbox::Sandboxes::default());
            app.manage(containers::Containers::default());
            app.manage(sidex::Sidex::default());
            app.manage(games::Games::default());
            app.manage(zx::Zx::default());
            app.manage(c64::C64State::default());
            app.manage(dos::Dos::default());
            app.manage(c64lab::C64Lab::default());
            app.manage(zxlab::ZxLab::default());
            app.manage(mail::Mail::default());
            app.manage(media::Media::default());
            app.manage(rymnet::Rymnet::default());
            app.manage(vpn::Vpn::default());
            vpn::restore(app.handle());
            let handle = app.handle().clone();
            tray::build(&handle)?;
            #[cfg(target_os = "macos")]
            app_menu(&handle)?;
            register_hotkeys(&handle);
            spawn_idle_watch(handle.clone());

            let hidden = std::env::args().any(|a| a == "--hidden");
            let w = main_window(&handle);
            if !hidden {
                let _ = w.show();
                let _ = w.set_focus();
            }
            // Pre-create the quick window so the hotkey feels instant.
            let _ = quick_window(&handle);
            if !cfg.launched_before {
                let mut c = cfg.clone();
                c.launched_before = true;
                handle.state::<App>().save_config(c);
                use tauri_plugin_autostart::ManagerExt;
                let _ = handle.autolaunch().enable();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::platform,
            commands::connect,
            commands::start_local,
            commands::status,
            commands::pair,
            commands::forget_server,
            commands::init_vault,
            commands::unlock,
            commands::unlock_recovery,
            commands::lock,
            commands::api,
            commands::chat_send,
            commands::chat_stop,
            commands::pull_model,
            commands::upload_paths,
            commands::upload_raw,
            commands::capture_screenshot,
            commands::export_document,
            commands::save_text_file,
            commands::vault_relocate,
            commands::activity,
            commands::open_main,
            commands::hide_quick,
            commands::toggle_quick_cmd,
            browser::browser_start,
            browser::browser_stop,
            browser::browser_send,
            browser::browser_frame,
            browser::browser_eval,
            browser::browser_scrape,
            browser::browser_snapshot,
            browser::browser_save_screenshot,
            browser::browser_save_page,
            terminal::term_open,
            terminal::term_list,
            terminal::term_attach,
            terminal::term_detach,
            terminal::term_write,
            terminal::term_resize,
            terminal::term_close,
            terminal::sandbox_exec,
            term_agent::agent_exec,
            term_agent::agent_cancel,
            term_agent::agent_set_sudo,
            term_agent::ssh_upload,
            term_agent::ssh_download,
            files::fs_places,
            files::fs_list,
            files::fs_preview,
            files::fs_open,
            files::fs_reveal,
            mail::mail_test,
            mail::mail_set_password,
            mail::mail_has_password,
            mail::mail_folders,
            mail::mail_list,
            mail::mail_read,
            mail::mail_set_flags,
            media::media_start,
            media::media_stop,
            media::media_send,
            media::media_frame,
            media::media_vault_url,
            rymnet::rymnet_status,
            rymnet::rymwin_paste,
            rymnet::rymnet_temp_dir,
            rymnet::rymnet_initiate,
            rymnet::rymnet_remove_peer,
            rymnet::rymnet_stop_daemon,
            rymnet::rymnet_guide,
            vpn::vpn_get,
            vpn::vpn_set,
            vpn::vpn_save,
            vpn::vpn_generate_key,
            vpn::vpn_import,
            vpn::vpn_import_file,
            vpn::vpn_check_ip,
            rymnet::rymnet_exec,
            tts::tts_status,
            tts::tts_download,
            tts::tts_remove,
            tts::tts_speak,
            tts::tts_add_voice,
            tts::tts_delete_voice,
            tts::tts_record_start,
            tts::tts_record_stop,
            tts::tts_recording,
            stt::stt_status,
            stt::stt_download,
            stt::stt_remove,
            stt::stt_listen,
            stt::stt_stop,
            stt::stt_pause,
            stt::stt_finish,
            sandbox::sandbox_status,
            sandbox::sandbox_create,
            sandbox::sandbox_reset,
            sandbox::sandbox_delete,
            sandbox::sandbox_update,
            sandbox::sandbox_send,
            sandbox::sandbox_shared_dir,
            containers::container_status,
            containers::container_create,
            containers::container_start,
            containers::container_stop,
            containers::container_reset,
            containers::container_delete,
            containers::container_update,
            containers::container_exec,
            containers::container_run,
            containers::container_cancel,
            containers::container_read,
            containers::container_write,
            containers::container_list,
            containers::container_shared_dir,
            sidex::sidex_status,
            commands::db_api_info,
            games::game_build,
            games::game_run,
            games::game_stop,
            games::game_status,
            games::game_docs,
            games::game_read_files,
            dos::dos_status,
            dos::dos_start,
            dos::dos_frame,
            dos::dos_send,
            dos::dos_stop,
            dos::dos_files,
            dos::dos_programs,
            dos::dos_import,
            dos::dos_delete,
            dos::dos_mkdir,
            c64::c64_attach,
            c64::c64_keys,
            c64::c64_joystick,
            c64::c64_joystick_port,
            c64::c64_load,
            c64::c64_insert_disk,
            c64::c64_save_state,
            c64::c64_control,
            c64::c64_memory,
            c64::c64_poke,
            c64::c64_lock,
            c64::c64_power_off,
            c64::c64_set_roms,
            c64::c64_read_roms,
            c64::c64_find_roms,
            c64dev::c64_tools,
            c64dev::c64_build,
            c64dev::c64_check,
            c64lab::c64lab_open,
            c64lab::c64lab_listing,
            c64lab::c64lab_xrefs,
            c64lab::c64lab_memory,
            c64lab::c64lab_source,
            c64lab::c64lab_assemble_patch,
            c64lab::c64lab_build,
            c64lab::c64lab_close,
            zx::zx_attach,
            zx::zx_keys,
            zx::zx_joystick,
            zx::zx_load,
            zx::zx_save_state,
            zx::zx_control,
            zx::zx_power_off,
            zxdev::zx_tools,
            zxdev::zx_build,
            zxdev::zx_check,
            zx::zx_memory,
            zx::zx_poke,
            zx::zx_lock,
            zx::zx_stats,
            zxlab::zxlab_open,
            zxlab::zxlab_listing,
            zxlab::zxlab_xrefs,
            zxlab::zxlab_memory,
            zxlab::zxlab_tape_block,
            zxlab::zxlab_source,
            zxlab::zxlab_assemble_patch,
            zxlab::zxlab_build,
            zxlab::zxlab_close,
            sidex::sidex_open,
            sidex::sidex_forget,
        ])
        .build(tauri::generate_context!())
        .expect("error while building RAD")
        .run(|app, ev| {
            if let tauri::RunEvent::Reopen { .. } = ev {
                show_main(app);
            }
            if let tauri::RunEvent::Exit = ev {
                // Drop the session — and in standalone mode lock the vault and checkpoint it.
                app.state::<browser::Browsers>().stop();
                app.state::<media::Media>().stop();
                app.state::<rymnet::Rymnet>().stop();
                app.state::<terminal::Terminals>().close_all();
                // Containers power off cleanly (their files are on their drives).
                app.state::<containers::Containers>().stop_all();
                app.state::<games::Games>().stop_all();
                app.state::<zx::Zx>().stop();
                app.state::<c64::C64State>().stop();
                app.state::<dos::Dos>().stop();
                app.state::<tts::Tts>().stop();
                app.state::<stt::Stt>().stop();
                let st = app.state::<App>();
                st.client.set_session(None);
                let emb = tauri::async_runtime::block_on(async { st.embedded.lock().await.take() });
                if let Some(e) = emb {
                    tauri::async_runtime::block_on(e.stop());
                }
                rad_server::shutdown_models();
            }
        });
}
