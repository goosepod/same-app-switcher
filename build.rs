//! Compiles assets/app.rc (icon, shortcut dialog, manifest, version info) into the Windows exe
//! using MinGW's windres.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    for f in ["app.rc", "app.ico", "app.manifest"] {
        println!("cargo:rerun-if-changed=assets/{f}");
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.ends_with("windows-gnu") {
        return; // non-Windows checks don't need resources
    }

    let assets = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app-res.o");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
        if cfg!(windows) { "windres".into() } else { "x86_64-w64-mingw32-windres".into() }
    });

    let ok = Command::new(&windres)
        .current_dir(&assets)
        .args(["--input", "app.rc", "--output-format=coff", "--output"])
        .arg(&out)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if ok {
        // Link the object directly so the linker can't drop it as "unused".
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        // The shortcut dialog lives in the resources, so this is fatal.
        panic!("`{windres}` failed or wasn't found (install mingw-w64: ./setup-wsl.sh)");
    }
}
