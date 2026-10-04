#Requires -Version 7.0
<# Builds the GUI and both fixed, matching-bitness hook brokers. No test is run,
   software installed, hook activated or original binary used by this script.
   Invoke in a Windows developer PowerShell with the Rust MSVC targets installed. #>
[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Run this script on Windows with the MSVC toolchain and Windows SDK.' }
$root = Split-Path -Parent $PSScriptRoot
if ($env:CARGO_TARGET_DIR) { throw 'Unset CARGO_TARGET_DIR: the fixed payload builder uses hook-engine/target.' }
function Run-Cargo([string[]]$CargoArgs) {
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo $($CargoArgs -join ' ') failed: $LASTEXITCODE" }
}
Push-Location $root
try {
    $out = Join-Path $root 'dist/CoralSpyNext'
    [void](New-Item -ItemType Directory -Force -Path $out)
    Push-Location hook-engine
    try {
        foreach ($item in @(
            @{ target = 'x86_64-pc-windows-msvc'; suffix = 'x64' },
            @{ target = 'i686-pc-windows-msvc'; suffix = 'x86' }
        )) {
            Run-Cargo -CargoArgs @('build', '--locked', '--release', '--target', $item.target, '-p', 'coralspy-hook-payload')
            Run-Cargo -CargoArgs @('build', '--locked', '--release', '--target', $item.target, '-p', 'coralspy-hook-broker')
            Copy-Item -LiteralPath "target/$($item.target)/release/coralspy-hook-broker.exe" -Destination (Join-Path $out "coralspy-hook-broker-$($item.suffix).exe")
        }
    } finally { Pop-Location }
    Run-Cargo -CargoArgs @('build', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc')
    Copy-Item -LiteralPath 'target/x86_64-pc-windows-msvc/release/coralspynext.exe' -Destination $out
    Copy-Item README.md, LICENSE, THIRD_PARTY.md, TESTING.md -Destination $out
    Copy-Item -Recurse -Force docs -Destination $out
    Get-ChildItem -LiteralPath $out -Filter '*.exe' | Get-FileHash -Algorithm SHA256 |
        Format-Table -AutoSize | Out-String | Set-Content -LiteralPath (Join-Path $out 'SHA256.txt') -Encoding utf8
    Write-Host "Built GUI + x64/x86 fixed brokers in $out. Native runtime tests were not run by this script."
} finally { Pop-Location }
