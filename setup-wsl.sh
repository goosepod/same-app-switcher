#!/usr/bin/env bash
# One-time setup of the Rust -> Windows build toolchain inside WSL Ubuntu.
# (Shared with textexpander — if you already ran its setup-wsl.sh, skip this.)
set -euo pipefail

sudo apt-get update
sudo apt-get install -y build-essential curl mingw-w64

if ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

rustup target add x86_64-pc-windows-gnu

echo
echo "Done. Open a new shell (or run: source ~/.cargo/env), then ./build.sh"
