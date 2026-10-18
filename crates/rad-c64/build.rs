// Builds chips' C64 emulation (chips/, zlib licence) with RAD's shim (shim.c).
fn main() {
    println!("cargo:rerun-if-changed=shim.c");
    println!("cargo:rerun-if-changed=chips");
    cc::Build::new()
        .file("shim.c")
        .include(".")
        .opt_level(3)
        .warnings(false)
        .compile("radc64");
}
