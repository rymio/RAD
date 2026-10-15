//! Where to find libVLC when linking. `LIBVLC_LIB_DIR` overrides the defaults:
//!   macOS:   /Applications/VLC.app/Contents/MacOS/lib (install VLC, e.g. `brew install --cask vlc`)
//!   Linux:   the system library (`apt install libvlc-dev`)
//!   Windows: the VLC SDK's lib folder must be given with LIBVLC_LIB_DIR.
//! At run time the engine looks for libVLC in `lib/` next to itself (the bundled layout made by
//! scripts/build-media.sh) and — on macOS — falls back to VLC.app.

fn main() {
    println!("cargo:rerun-if-env-changed=LIBVLC_LIB_DIR");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let dir = std::env::var("LIBVLC_LIB_DIR").ok().or_else(|| (target_os == "macos").then(|| "/Applications/VLC.app/Contents/MacOS/lib".to_string()));
    if let Some(d) = &dir {
        println!("cargo:rustc-link-search=native={d}");
    }
    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/lib");
            if let Some(d) = &dir {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{d}");
            }
        }
        "linux" => println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib"),
        _ => {}
    }
}
