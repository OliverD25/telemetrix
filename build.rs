//! Windows only:
//! - load rarely used system DLLs on first use instead of at start.
//!   sysinfo's import table names them all, so without this Windows maps
//!   every one into the process even when telemetrix never calls it (COM
//!   and PDH are only needed for sensors and are probed in a child
//!   process). Each unused DLL costs working set; see README "Memory".
//! - embed `telemetrix.manifest`, which selects the segment heap. The
//!   default heap kept more and more freed memory committed during long
//!   runs (about 1 MB an hour with the default plugins, while the bytes in
//!   use stayed flat); the segment heap gives it back.

const DELAY_LOADED: &[&str] = &[
    "ole32.dll",
    "oleaut32.dll",
    "pdh.dll",
    "powrprof.dll",
    "user32.dll",
    "ws2_32.dll",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !(windows && msvc) {
        return;
    }
    for dll in DELAY_LOADED {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
    println!("cargo:rerun-if-changed=telemetrix.manifest");
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = std::path::Path::new(&dir).join("telemetrix.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
