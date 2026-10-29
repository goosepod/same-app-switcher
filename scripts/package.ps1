# Run via ./build.ps1 -Package, or use the phases in the signing workflow.
param(
    [ValidateSet('All', 'Prepare', 'Package', 'Finalize')]
    [string]$Phase = 'All',
    [switch]$RequireSigned
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE) { throw 'Could not read Cargo metadata.' }
$version = ($metadata.packages | Where-Object name -eq 'SameAppSwitcher').version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'MSI packaging requires a major.minor.patch version.' }
if ($env:GITHUB_REF_TYPE -eq 'tag' -and $env:GITHUB_REF_NAME -cne "v$version") {
    throw "Tag $env:GITHUB_REF_NAME must match Cargo.toml version v$version."
}
$parts = $version.Split('.')
if ([int]$parts[0] -gt 255 -or [int]$parts[1] -gt 255 -or [int]$parts[2] -gt 65535) {
    throw 'Version exceeds Windows Installer limits (255.255.65535).'
}
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$packageDir = Join-Path $targetDir 'package'
$exe = Join-Path $packageDir 'SameAppSwitcher.exe'
$startupDll = Join-Path $packageDir 'startup.dll'
$stem = "SameAppSwitcher-$version-windows-x64"
New-Item -ItemType Directory -Force dist | Out-Null

if ($Phase -in @('All', 'Prepare')) {
    New-Item -ItemType Directory -Force $packageDir | Out-Null
    Copy-Item (Join-Path $targetDir 'x86_64-pc-windows-msvc/release/SameAppSwitcher.exe') $exe -Force
    cl.exe /nologo /LD /MT /W4 /WX /DUNICODE /D_UNICODE installer/startup.c "/Fo$packageDir/startup.obj" /link "/OUT:$startupDll" msi.lib advapi32.lib
    if ($LASTEXITCODE) { throw 'Could not build the native installer startup action.' }
}

# Sign the staged EXE and DLL between Prepare and Package.
if ($Phase -in @('All', 'Package')) {
    if (!(Test-Path $exe) -or !(Test-Path $startupDll)) { throw 'Run the Prepare phase before packaging.' }
    Copy-Item $exe "dist/$stem.exe" -Force
    wix build installer/Package.wxs -arch x64 -d "Version=$version" -d "ExePath=$((Resolve-Path $exe).Path)" -d "StartupDll=$((Resolve-Path $startupDll).Path)" -o "dist/$stem.msi"
    if ($LASTEXITCODE) { throw 'WiX installer build failed.' }
}

# Sign the MSI between Package and Finalize. Hashes and the ZIP must be last.
if ($Phase -in @('All', 'Finalize')) {
    $files = @("dist/$stem.exe", "dist/$stem.zip", "dist/$stem.msi")
    if ($RequireSigned) {
        foreach ($file in @("dist/$stem.exe", "dist/$stem.msi", $startupDll)) {
            $signature = Get-AuthenticodeSignature $file
            if ($signature.Status -ne 'Valid' -or $null -eq $signature.TimeStamperCertificate) {
                throw "Missing valid, timestamped Authenticode signature: $file"
            }
        }
    }
    if ((Get-FileHash $exe).Hash -ne (Get-FileHash "dist/$stem.exe").Hash) {
        throw 'Packaged EXE differs from the prepared payload.'
    }
    $stage = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
    try {
        New-Item -ItemType Directory $stage | Out-Null
        Copy-Item "dist/$stem.exe" "$stage/SameAppSwitcher.exe"
        Copy-Item README.md, LICENSE $stage
        Compress-Archive -Path "$stage/*" -DestinationPath "dist/$stem.zip" -Force
    } finally {
        Remove-Item $stage -Recurse -Force
    }
    $files | ForEach-Object {
        $hash = Get-FileHash $_ -Algorithm SHA256
        '{0}  {1}' -f $hash.Hash.ToLowerInvariant(), (Split-Path $_ -Leaf)
    } | Set-Content dist/SHA256SUMS.txt -Encoding ascii
}
if ($env:GITHUB_OUTPUT) { "version=$version" >> $env:GITHUB_OUTPUT }
