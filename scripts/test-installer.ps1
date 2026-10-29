$ErrorActionPreference = 'Stop'
$installers = @(Get-ChildItem dist/*.msi)
if ($installers.Count -ne 1) { throw 'Expected exactly one MSI in dist.' }
$msi = $installers[0].FullName
$installedExe = Join-Path $env:LOCALAPPDATA 'SameAppSwitcher/SameAppSwitcher.exe'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'SameAppSwitcher/SameAppSwitcher.lnk'
$log = Join-Path $env:TEMP 'SameAppSwitcher-install.log'
try {
    $process = Start-Process msiexec.exe -ArgumentList @('/i', "`"$msi`"", '/qn', '/norestart', '/l*v', "`"$log`"") -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) { throw "Install failed: $($process.ExitCode)" }
    if (!(Test-Path $installedExe) -or !(Test-Path $shortcut)) { throw 'Installed EXE or Start menu shortcut is missing.' }
    if ((Get-FileHash $installedExe).Hash -ne (Get-FileHash target/x86_64-pc-windows-gnu/release/SameAppSwitcher.exe).Hash) {
        throw 'Installed EXE does not match the build.'
    }
} catch {
    if (Test-Path $log) { Get-Content $log -Tail 100 }
    throw
} finally {
    $process = Start-Process msiexec.exe -ArgumentList @('/x', "`"$msi`"", '/qn', '/norestart') -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010, 1605)) { throw "Uninstall failed: $($process.ExitCode)" }
}
if ((Test-Path $installedExe) -or (Test-Path $shortcut)) { throw 'Uninstall left the EXE or shortcut behind.' }
