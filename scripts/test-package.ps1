# Verify final artifacts, including the gate that prevents unsigned signed releases.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE) { throw 'Could not read version.' }
$version = ($metadata.packages | Where-Object name -eq 'SameAppSwitcher').version
$stem = "SameAppSwitcher-$version-windows-x64"
$lines = @(Get-Content dist/SHA256SUMS.txt)
if ($lines.Count -ne 3) { throw 'Expected checksums for EXE, ZIP, and MSI.' }
foreach ($line in $lines) {
    $expected, $file = $line -split '  ', 2
    if ((Get-FileHash (Join-Path dist $file) -Algorithm SHA256).Hash.ToLowerInvariant() -cne $expected) {
        throw "Incorrect checksum: $file"
    }
}
$stage = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
try {
    Expand-Archive "dist/$stem.zip" $stage
    $names = @(Get-ChildItem $stage | ForEach-Object Name | Sort-Object)
    if (($names -join ',') -cne 'LICENSE,README.md,SameAppSwitcher.exe') { throw 'Unexpected ZIP contents.' }
    if ((Get-FileHash "$stage/SameAppSwitcher.exe").Hash -ne (Get-FileHash "dist/$stem.exe").Hash) {
        throw 'ZIP and standalone EXE differ.'
    }
} finally {
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
}
$rejected = $false
try {
    ./scripts/package.ps1 -Phase Finalize -RequireSigned
} catch {
    if ($_.Exception.Message -notlike 'Missing valid, timestamped Authenticode signature:*') { throw }
    $rejected = $true
}
if (!$rejected) { throw 'The signed-release gate accepted an unsigned build.' }
Write-Host 'Verified checksums, ZIP contents, payload equality, and rejection of unsigned signed releases.'
