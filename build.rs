//! Embeds the application icon and version information into the Windows executable, and builds the
//! Explorer thumbnail provider DLL (`thumbs/`) so the exe can carry it and install it.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=thumbs/src");
    println!("cargo:rerun-if-changed=thumbs/Cargo.toml");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "PDF Print Studio");
    res.set("FileDescription", "PDF Print Studio");
    res.set("LegalCopyright", "Muriel Guedes Pucci Berehulka");
    res.compile().expect("compiling Windows resources");

    // the DLL is always built optimised, in its own target folder so it does not wait on this build's lock
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target = out.join("thumbs-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--manifest-path", "thumbs/Cargo.toml", "--target-dir"])
        .arg(&target)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .status()
        .expect("running cargo for the thumbnail provider");
    assert!(status.success(), "building the thumbnail provider (thumbs/) failed");
    std::fs::copy(target.join("release").join("pdfprintstudio_thumbs.dll"), out.join("thumbs.dll"))
        .expect("copying the thumbnail provider");
}
