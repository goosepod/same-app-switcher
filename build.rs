//! Compiles assets/app.rc (icon, shortcut dialog, manifest, version info) into the Windows exe
//! using the Windows SDK resource compiler or MinGW's windres.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    for f in ["app.rc", "app.ico", "app.manifest"] {
        println!("cargo:rerun-if-changed=assets/{f}");
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return; // non-Windows checks don't need resources
    }

    let assets = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    if target.ends_with("windows-msvc") {
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.res");
        let status = Command::new("rc.exe")
            .current_dir(&assets)
            .args(["/nologo", "/fo"])
            .arg(&out)
            .arg("app.rc")
            .status()
            .expect("Windows SDK resource compiler not found; run ./build.ps1 or use an x64 Visual Studio developer shell");
        assert!(status.success(), "Windows resource compilation failed");
        println!("cargo:rustc-link-arg-bins={}", out.display());
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app-res.o");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
        if cfg!(windows) {
            "windres".into()
        } else {
            "x86_64-w64-mingw32-windres".into()
        }
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
