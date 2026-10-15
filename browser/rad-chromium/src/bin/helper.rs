//! macOS helper: Chromium's GPU, renderer, network and other sub-processes run from helper
//! apps inside the engine's bundle (`rad-chromium Helper (Renderer).app` …), all this binary.

fn main() {
    #[cfg(target_os = "macos")]
    {
        use cef::{args::Args, *};
        let args = Args::new();
        let _sandbox = {
            let mut sandbox = cef::sandbox::Sandbox::new();
            sandbox.initialize(args.as_main_args());
            sandbox
        };
        let _loader = {
            let loader = library_loader::LibraryLoader::new(&std::env::current_exe().expect("exe path"), true);
            assert!(loader.load(), "couldn't load the Chromium Embedded Framework");
            loader
        };
        let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
        let code = execute_process(Some(args.as_main_args()), None::<&mut App>, std::ptr::null_mut());
        std::process::exit(code.max(0));
    }
    #[cfg(not(target_os = "macos"))]
    eprintln!("rad-chromium-helper is only used on macOS; elsewhere rad-chromium is its own helper.");
}
