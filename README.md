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
- Single small portable exe, with an optional MSI installer. Uses only the Win32 API (`windows-sys`).

## Building on Windows

WSL and MinGW are not required. Install [Rust](https://rustup.rs/) and
[Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with
the **Desktop development with C++** workload and Windows SDK. Open PowerShell:

```powershell
./build.ps1          # build dist/SameAppSwitcher.exe
./build.ps1 -Run     # build and start the portable app
./build.ps1 -Package # also create versioned EXE, ZIP, MSI, and checksums
```

The script finds the Visual Studio tools automatically, builds the native x64
MSVC Rust target, and uses the Windows SDK resource compiler to embed the icon,
shortcut dialog, manifest, and version information. The C runtime is linked
statically, so portable users do not need a Visual C++ redistributable.

Packaging additionally requires the [.NET SDK](https://dotnet.microsoft.com/download)
and WiX CLI: `dotnet tool install --global wix --version 4.0.6`.
For newer .NET runtimes, set `$env:DOTNET_ROLL_FORWARD = 'Major'` before packaging.
The existing `setup-wsl.sh` and `build.sh` remain optional WSL cross-build helpers
for the portable executable.

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
Uninstall removes startup only when the entry points at this installation.
Upgrades preserve the startup setting, and a portable copy's startup entry is
left alone. Uninstall preserves the saved shortcut preference.
These builds are unsigned; Windows may display a SmartScreen warning.

## GitHub releases

1. Update the version in `Cargo.toml`, `Cargo.lock`, `assets/app.rc`, and
   `assets/app.manifest` together, then push the changes to `main`.
2. Create and push the matching tag, for example `git tag v0.2.0` followed by
   `git push origin v0.2.0`.
3. The **Windows build** workflow compiles the app, builds the packages, checks
   MSI installation and removal, and publishes a GitHub Release with all downloads.

The tag must exactly match the Cargo version. Manual workflow runs create artifacts
without publishing a release. The build uses the native Windows MSVC Rust target,
Visual Studio Build Tools, the Windows SDK, and WiX 4.0.6 for the MSI. CI verifies
installation, upgrade, startup preservation and cleanup, and uninstallation
on a disposable Windows runner.
The WiX `UpgradeCode` and component GUIDs in `installer/Package.wxs` must remain
stable across releases so upgrades recognize existing installations.

## Known limits

- Windows of an app running **as administrator** can't be inspected unless this app is also
  elevated, so they're skipped.
- Store/UWP apps all run inside `ApplicationFrameHost.exe`, so they're grouped together.
- If another program already owns the shortcut, a message says so at startup and the
  shortcut dialog opens so you can pick another.
