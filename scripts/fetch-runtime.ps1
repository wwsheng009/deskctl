[CmdletBinding()]
param(
    [string]$Version = "1.27.0",
    [string]$SourceUrl,
    [string]$PackagePath,
    [string]$FromDirectory,
    [string]$Destination,
    [switch]$Force,
    [switch]$VerifyOnly
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Destination) {
    $Destination = Join-Path $repoRoot "third_party\onnxruntime\win-x64"
}
$Destination = [System.IO.Path]::GetFullPath($Destination)
$manifestPath = Join-Path $Destination "hashes.json"
$requiredFiles = @("onnxruntime.dll", "onnxruntime_providers_shared.dll")

function Get-Sha256([string]$Path) {
    return (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Read-Manifest {
    if (-not (Test-Path $manifestPath)) {
        return @{}
    }
    $raw = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json
    $result = @{}
    foreach ($property in $raw.PSObject.Properties) {
        $result[$property.Name] = [string]$property.Value
    }
    return $result
}

function Verify-Destination {
    $manifest = Read-Manifest
    $failures = @()
    foreach ($name in $requiredFiles) {
        $path = Join-Path $Destination $name
        if (-not (Test-Path $path)) {
            $failures += "missing $name"
            continue
        }
        if ($manifest.ContainsKey($name)) {
            $actual = Get-Sha256 $path
            if ($actual -ne $manifest[$name]) {
                $failures += "hash mismatch for $name"
            }
        }
    }
    if ($failures.Count -gt 0) {
        throw ("runtime verification failed: " + ($failures -join "; "))
    }
    Write-Host "ONNX Runtime verification passed: $Destination"
}

if ($VerifyOnly) {
    Verify-Destination
    exit 0
}

New-Item -ItemType Directory -Force -Path $Destination | Out-Null

if ($FromDirectory) {
    $FromDirectory = [System.IO.Path]::GetFullPath($FromDirectory)
    foreach ($name in $requiredFiles) {
        $source = Join-Path $FromDirectory $name
        if (-not (Test-Path $source)) {
            throw "missing source file: $source"
        }
        Copy-Item -Path $source -Destination (Join-Path $Destination $name) -Force
    }
} else {
    $temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("deskctl-ort-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $temporary | Out-Null
    try {
        if (-not $SourceUrl) {
            $SourceUrl = "https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime/$Version"
        }
        $package = if ($PackagePath) {
            [System.IO.Path]::GetFullPath($PackagePath)
        } else {
            $download = Join-Path $temporary "onnxruntime.nupkg.zip"
            Write-Host "Downloading ONNX Runtime $Version from $SourceUrl"
            Invoke-WebRequest -Uri $SourceUrl -OutFile $download -UseBasicParsing
            $download
        }
        if (-not (Test-Path $package)) {
            throw "runtime package not found: $package"
        }
        $expanded = Join-Path $temporary "expanded"
        Expand-Archive -Path $package -DestinationPath $expanded -Force
        foreach ($name in $requiredFiles) {
            $match = Get-ChildItem -Path $expanded -Recurse -File -Filter $name |
                Select-Object -First 1
            if (-not $match) {
                throw "package does not contain $name"
            }
            Copy-Item -Path $match.FullName -Destination (Join-Path $Destination $name) -Force
        }
    } finally {
        Remove-Item -Path $temporary -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$hashes = [ordered]@{}
foreach ($name in $requiredFiles) {
    $path = Join-Path $Destination $name
    if (Test-Path $path) {
        $hashes[$name] = Get-Sha256 $path
    }
}
$hashes | ConvertTo-Json | Set-Content -Path $manifestPath -Encoding UTF8
Write-Host "ONNX Runtime ready: $Destination"
Verify-Destination
