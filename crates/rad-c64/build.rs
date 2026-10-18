// Builds chips' C64 emulation (chips/, zlib licence) with RAD's shim (shim.c).
fn main() {
    println!("cargo:rerun-if-changed=shim.c");
    println!("cargo:rerun-if-changed=chips");
    cc::Build::new()
        .file("shim.c")
        // chips needs C11 (stdalign, compound literals); MSVC only offers them in C11 mode.
        .std("c11")
        .include(".")
        .opt_level(3)
        .warnings(false)
        .compile("radc64");
}
