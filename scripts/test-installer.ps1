# Intended for disposable Windows CI runners: exercises MSI lifecycle and HKCU startup settings.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$installers = @(Get-ChildItem dist/*.msi)
if ($installers.Count -ne 1) { throw 'Expected exactly one MSI in dist.' }
$msi = $installers[0].FullName
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$builtExe = (Resolve-Path (Join-Path $targetDir 'x86_64-pc-windows-msvc/release/SameAppSwitcher.exe')).Path
$startupDll = (Resolve-Path (Join-Path $targetDir 'installer/startup.dll')).Path
$installedExe = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'SameAppSwitcher/SameAppSwitcher.exe'))
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'SameAppSwitcher/SameAppSwitcher.lnk'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$sentinel = 'SameAppSwitcher.InstallerTest.' + [guid]::NewGuid().ToString()
$activeMsi = $null

function Get-Startup {
    Get-ItemPropertyValue $runKey -Name SameAppSwitcher -ErrorAction SilentlyContinue
}
function Invoke-Msi([string]$Action, [string]$Path) {
    $log = Join-Path $env:TEMP ('SameAppSwitcher-' + [guid]::NewGuid().ToString() + '.log')
    $process = Start-Process msiexec.exe -ArgumentList @($Action, "`"$Path`"", '/qn', '/norestart', '/l*v', "`"$log`"") -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) {
        Get-Content $log -Tail 100
        throw "MSI $Action failed: $($process.ExitCode)"
    }
}
function Assert-Removed {
    if ((Test-Path $installedExe) -or (Test-Path $shortcut)) { throw 'Uninstall left the EXE or shortcut behind.' }
}

if ((Test-Path $installedExe) -or (Get-Startup)) { throw 'Use a clean runner without an existing SameAppSwitcher installation or startup entry.' }
try {
    Invoke-Msi /i $msi
    $activeMsi = $msi
    if (!(Test-Path $installedExe) -or !(Test-Path $shortcut)) { throw 'Installed EXE or Start menu shortcut is missing.' }
    if ((Get-FileHash $installedExe).Hash -ne (Get-FileHash $builtExe).Hash) { throw 'Installed EXE does not match the build.' }
    if (Get-Startup) { throw 'Install enabled startup without opting in.' }

    # Simulate enabling startup from the tray. Preserve it through a major upgrade.
    New-Item $runKey -Force | Out-Null
    $command = '"' + $installedExe + '"'
    Set-ItemProperty $runKey -Name SameAppSwitcher -Value $command
    Set-ItemProperty $runKey -Name $sentinel -Value 'unrelated-value'
    $metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE) { throw 'Could not read version.' }
    $version = [version]($metadata.packages | Where-Object name -eq 'SameAppSwitcher').version
    # Use a distinct higher MSI version; the app payload is intentionally identical.
    $upgradeVersion = '{0}.{1}.{2}' -f $version.Major, $version.Minor, ($version.Build + 1)
    $upgradeMsi = Join-Path $targetDir 'installer/upgrade-test.msi'
    wix build installer/Package.wxs -arch x64 -d "Version=$upgradeVersion" -d "ExePath=$builtExe" -d "StartupDll=$startupDll" -o $upgradeMsi
    if ($LASTEXITCODE) { throw 'Could not build the upgrade test MSI.' }
    $upgradeMsi = (Resolve-Path $upgradeMsi).Path
    Invoke-Msi /i $upgradeMsi
    $activeMsi = $upgradeMsi
    if ((Get-Startup) -cne $command) { throw 'Upgrade lost the startup setting.' }

    Invoke-Msi /x $activeMsi
    $activeMsi = $null
    Assert-Removed
    if (Get-Startup) { throw 'Uninstall left the installed app startup entry behind.' }
    if ((Get-ItemPropertyValue $runKey -Name $sentinel) -ne 'unrelated-value') { throw 'Uninstall altered another startup entry.' }

    # A portable copy may take ownership of startup. Uninstall must preserve it.
    Invoke-Msi /i $msi
    $activeMsi = $msi
    $portable = '"C:\Portable Apps\SameAppSwitcher.exe"'
    Set-ItemProperty $runKey -Name SameAppSwitcher -Value $portable
    Invoke-Msi /x $activeMsi
    $activeMsi = $null
    Assert-Removed
    if ((Get-Startup) -cne $portable) { throw 'Uninstall removed the portable copy startup entry.' }
} finally {
    if ($activeMsi) { Invoke-Msi /x $activeMsi }
    Remove-ItemProperty $runKey -Name SameAppSwitcher -ErrorAction SilentlyContinue
    Remove-ItemProperty $runKey -Name $sentinel -ErrorAction SilentlyContinue
}
Write-Host 'Verified native install, upgrade, uninstall, startup cleanup, and portable-copy preservation.'
