param(
    [switch]$Package,
    [switch]$PreparePackage,
    [switch]$Run
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($Package -and $PreparePackage) { throw 'Choose Package or PreparePackage, not both.' }
Push-Location $PSScriptRoot
try {
    # Initialize the x64 compiler and Windows SDK even in a normal PowerShell.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    if (!(Test-Path $vswhere)) {
        throw 'Install Visual Studio Build Tools with Desktop development with C++ (including the Windows SDK).'
    }
    $vsPath = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (!$vsPath) { throw 'Visual Studio C++ build tools were not found.' }
    $devCmd = Join-Path $vsPath 'Common7/Tools/VsDevCmd.bat'
    $initialize = '"{0}" -no_logo -arch=x64 -host_arch=x64 >nul && set' -f $devCmd
    $developerEnvironment = & $env:ComSpec /d /s /c $initialize
    if ($LASTEXITCODE) { throw 'Could not initialize the Visual Studio build environment.' }
    foreach ($line in $developerEnvironment) {
        if ($line -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
    if (!(Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Install Rust from https://rustup.rs, then open a new PowerShell.' }
    rustup target add x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Could not install the Windows MSVC Rust target.' }
    cargo check --locked --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Cargo check failed.' }
    cargo test --locked --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Rust regression tests failed.' }
    cargo build --locked --release --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Cargo build failed.' }

    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    $exe = Join-Path $targetDir 'x86_64-pc-windows-msvc/release/SameAppSwitcher.exe'
    New-Item -ItemType Directory -Force dist | Out-Null
    # Stop only the copy whose output is about to be replaced.
    $destination = Join-Path $PSScriptRoot 'dist/SameAppSwitcher.exe'
    Get-Process SameAppSwitcher -ErrorAction SilentlyContinue | Where-Object Path -eq $destination | Stop-Process
    Copy-Item $exe $destination -Force
    if ($Package) { & ./scripts/package.ps1 }
    if ($PreparePackage) { & ./scripts/package.ps1 -Phase Prepare }
    if ($Run) { Start-Process $destination }
    Write-Host 'Built dist/SameAppSwitcher.exe'
} finally {
    Pop-Location
}
