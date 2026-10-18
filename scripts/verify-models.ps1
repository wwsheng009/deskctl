[CmdletBinding()]
param(
    [string]$Models = ''
)

$ErrorActionPreference = 'Stop'
if (-not $Models) {
    $Models = Join-Path $PSScriptRoot '..\models'
}
$Models = [System.IO.Path]::GetFullPath($Models)
$hashFile = Join-Path $Models 'hashes.json'
if (-not (Test-Path $hashFile)) {
    throw "missing hashes.json in $Models"
}

$expected = Get-Content $hashFile -Raw | ConvertFrom-Json
$failed = @()
foreach ($property in $expected.PSObject.Properties) {
    $path = Join-Path $Models $property.Name
    if (-not (Test-Path $path)) {
        $failed += "missing: $($property.Name)"
        continue
    }
    $actual = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLowerInvariant()
    if ($actual -ne $property.Value.ToLowerInvariant()) {
        $failed += "hash mismatch: $($property.Name)"
    } else {
        Write-Host "ok $($property.Name)"
    }
}

$runtimeHashes = Join-Path $PSScriptRoot '..\third_party\onnxruntime\win-x64\hashes.json'
if (Test-Path $runtimeHashes) {
    $runtimeDir = Split-Path $runtimeHashes
    $runtimeExpected = Get-Content $runtimeHashes -Raw | ConvertFrom-Json
    foreach ($property in $runtimeExpected.PSObject.Properties) {
        $path = Join-Path $runtimeDir $property.Name
        if (-not (Test-Path $path)) {
            $failed += "missing runtime: $($property.Name)"
            continue
        }
        $actual = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLowerInvariant()
        if ($actual -ne $property.Value.ToLowerInvariant()) {
            $failed += "runtime hash mismatch: $($property.Name)"
        } else {
            Write-Host "ok runtime/$($property.Name)"
        }
    }
}

if ($failed.Count -gt 0) {
    $failed | ForEach-Object { Write-Error $_ }
    exit 1
}

Write-Host 'all model and runtime hashes verified'

