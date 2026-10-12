//! llama.cpp's Metal code uses `@available` checks. When building for an older macOS than the
//! SDK (the app targets 11.0), clang turns them into calls to `__isPlatformVersionAtLeast`, which
//! lives in clang's runtime library — and rustc links with `-nodefaultlibs`, so add it here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::process::Command::new("xcrun").args(["clang", "--print-runtime-dir"]).output();
    match out {
        Ok(o) if o.status.success() => {
            let dir = String::from_utf8_lossy(&o.stdout).trim().to_string();
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
        _ => println!("cargo:warning=couldn't locate clang's runtime library (xcrun clang --print-runtime-dir)"),
    }
}
