# SameAppSwitcher

A tiny Windows tray app that recreates macOS's **Cmd+`**: cycle between the windows of the app
you're in, without going to the taskbar.

| Shortcut | Action |
|---|---|
| **Alt + `** | Next window of the current app |
| **Alt + Shift + `** | Previous window |

- Works for **any** app. Windows count as "the same app" when they come from the same executable
  (all `Code.exe` windows, all `firefox.exe` windows, all Explorer windows, ...).
- Hold Alt and tap ` repeatedly to walk through every window. Shift reverses direction.
- Minimized windows are restored. Windows on other virtual desktops are skipped.
- The ` key is the one above Tab, found by its physical position, so it works on non-US layouts.
- **Right-click or double-click** the tray icon for the menu: **Change shortcut...**,
  **Start with Windows**, and **Exit**.
- **Change shortcut...** opens a small dialog: click the box and press any combination
  (e.g. Ctrl+Alt+Q). Tick *Also hold the Windows key* for Win combos. The previous-window
  shortcut is always the same combo plus Shift. The choice is saved in
  `HKCU\Software\SameAppSwitcher`. *Reset* returns to Alt+`.
- Single small exe, no installer. Uses only the Win32 API (`windows-sys`).

## Building (WSL Ubuntu)

```bash
cd /mnt/c/Users/JBMcMichael/Projects/same-app-switcher
./setup-wsl.sh        # once (already done if you set up textexpander): rustup, Windows target, MinGW
./build.sh            # builds dist/SameAppSwitcher.exe
./build.sh run        # same, then restarts the app
```

The build runs in WSL but produces a native Windows exe. `build.rs` uses MinGW's `windres` to embed the icon, the shortcut dialog, the manifest
(sharp on high-DPI screens, modern controls) and version info. Build files go to
`~/.cache/same-app-switcher-target` because building on `/mnt/c` is slow.

## Downloads and installation

GitHub Actions builds Windows x64 downloads on every push to `main`, pull request,
and manual run. Download the build artifact from the **Actions** tab, or use the
files attached to a tagged GitHub Release:

- `SameAppSwitcher-VERSION-windows-x64.exe`: standalone portable executable.
- `SameAppSwitcher-VERSION-windows-x64.zip`: portable executable, README, and MIT license.
- `SameAppSwitcher-VERSION-windows-x64.msi`: current-user installer. Installs into
  `%LOCALAPPDATA%\SameAppSwitcher` and adds a Start menu shortcut. No administrator
  rights are required. Remove it through Windows **Installed apps**.
- `SHA256SUMS.txt`: checksums for all three downloads.

Exit any running copy before installing or upgrading. Launch from the Start menu
after installation; **Start with Windows** remains an opt-in tray setting.
Before uninstalling, turn off **Start with Windows** in the tray menu if enabled.
Uninstall preserves the saved shortcut preference.
These builds are unsigned; Windows may display a SmartScreen warning.

## GitHub releases

1. Update the version in `Cargo.toml`, `Cargo.lock`, `assets/app.rc`, and
   `assets/app.manifest` together, then push the changes to `main`.
2. Create and push the matching tag, for example `git tag v0.2.0` followed by
   `git push origin v0.2.0`.
3. The **Windows build** workflow compiles the app, builds the packages, checks
   MSI installation and removal, and publishes a GitHub Release with all downloads.

The tag must exactly match the Cargo version. Manual workflow runs create artifacts
without publishing a release. The build uses the Windows GNU Rust target and
MinGW (including `windres` for embedded resources), plus WiX 4.0.6 for the MSI.
The WiX `UpgradeCode` and component GUIDs in `installer/Package.wxs` must remain
stable across releases so upgrades recognize existing installations.

For local Windows packaging, install Rust, the `x86_64-pc-windows-gnu` target,
MinGW-w64 (with `gcc` and `windres` on PATH), and the WiX CLI with
`dotnet tool install --global wix --version 4.0.6`. In PowerShell:

```powershell
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = 'gcc'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = '-C target-feature=+crt-static'
cargo build --locked --release --target x86_64-pc-windows-gnu
./scripts/package.ps1
```

## Known limits

- Windows of an app running **as administrator** can't be inspected unless this app is also
  elevated, so they're skipped.
- Store/UWP apps all run inside `ApplicationFrameHost.exe`, so they're grouped together.
- If another program already owns the shortcut, a message says so at startup and the
  shortcut dialog opens so you can pick another.
