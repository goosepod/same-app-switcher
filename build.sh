#!/usr/bin/env bash
# Build SameAppSwitcher.exe from WSL.   ./build.sh        build into dist/
#                                         ./build.sh run    build, then (re)start it
set -euo pipefail
cd "$(dirname "$0")"

# rustup puts cargo in ~/.cargo/bin, which isn't on PATH until a new login shell. Find it anyway.
if ! command -v cargo >/dev/null 2>&1 && [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo isn't installed in this WSL distro yet. Run ./setup-wsl.sh first (once), then ./build.sh again." >&2
    exit 1
fi

# Keep cargo's build files on the Linux side: /mnt/c is slow for thousands of small files.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/same-app-switcher-target}"
# Link with MinGW, and statically include its runtime so the .exe needs no extra DLLs.
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-C target-feature=+crt-static"

cargo build --release --target x86_64-pc-windows-gnu

# A running .exe can't be overwritten, so stop it first.
taskkill.exe /IM SameAppSwitcher.exe /F >/dev/null 2>&1 || true
taskkill.exe /IM same-app-switcher.exe /F >/dev/null 2>&1 || true   # pre-rename name
sleep 0.5

mkdir -p dist
rm -f dist/same-app-switcher.exe
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-gnu/release/SameAppSwitcher.exe" dist/
echo "Built dist/SameAppSwitcher.exe ($(du -h dist/SameAppSwitcher.exe | cut -f1))"

if [[ "${1:-}" == "run" ]]; then
    cmd.exe /c start "" "$(wslpath -w dist/SameAppSwitcher.exe)" 2>/dev/null
    echo "Started. Look for the two-windows icon in the system tray, then try Alt+\`."
fi
