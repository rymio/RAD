//! Lays out the runnable engine under `target/bundle` after `cargo build --release`:
//!   macOS:   rad-chromium.app (with the CEF framework and its helper apps)
//!   Linux:   rad-chromium/ (executable, libcef.so, resources, locales)
//!   Windows: rad-chromium/ (rad-chromium.exe, libcef.dll, resources, locales)

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let release = target.join("release");
    let out = target.join("bundle");
    std::fs::create_dir_all(&out).expect("create target/bundle");

    #[cfg(target_os = "macos")]
    let path = {
        use cef::build_util::mac::{bundle, BundleInfo};
        let version = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let info = BundleInfo::new("rad-chromium", "com.rymtech.rad.chromium", "RAD Chromium", "English", version);
        let app = bundle(&out, &release, "rad-chromium", "rad-chromium-helper", None, info).expect("bundle the engine");
        // Never in the Dock: it's a background engine for RAD.
        let plist_path = app.join("Contents/Info.plist");
        let mut plist = plist::Value::from_file(&plist_path).expect("read Info.plist");
        if let Some(d) = plist.as_dictionary_mut() {
            d.insert("LSUIElement".into(), plist::Value::Boolean(true));
        }
        plist.to_file_xml(&plist_path).expect("write Info.plist");
        app
    };
    #[cfg(target_os = "linux")]
    let path = cef::build_util::linux::bundle(&out.join("rad-chromium"), &release, "rad-chromium").expect("bundle the engine");
    #[cfg(target_os = "windows")]
    let path = cef::build_util::win::bundle(&out.join("rad-chromium"), &release, "rad-chromium").expect("bundle the engine");

    println!("{}", path.display());
}
