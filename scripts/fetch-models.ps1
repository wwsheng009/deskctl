[CmdletBinding()]
param(
    [string]$Destination,
    [string]$FromDirectory,
    [switch]$Force,
    [switch]$VerifyOnly
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Destination) {
    $Destination = Join-Path $repoRoot "models"
}
$Destination = [System.IO.Path]::GetFullPath($Destination)
$manifestPath = Join-Path $Destination "hashes.json"

$files = @(
    @{
        Name = "ch_PP-OCRv4_det.onnx"
        Url = "https://hf-mirror.com/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_det_infer.onnx"
    },
    @{
        Name = "ch_PP-OCRv4_rec.onnx"
        Url = "https://hf-mirror.com/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_rec_infer.onnx"
    },
    @{
        Name = "ch_PP-OCR_keys_v1.txt"
        Url = "https://gitee.com/paddlepaddle/PaddleOCR/raw/main/ppocr/utils/ppocr_keys_v1.txt"
    },
    @{
        Name = "icon_detect.onnx"
        Url = "https://hf-mirror.com/onnx-community/OmniParser-icon_detect_640x640/resolve/main/onnx/model.onnx"
    }
)

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

function Verify-Models {
    $manifest = Read-Manifest
    $failures = @()
    foreach ($file in $files) {
        $path = Join-Path $Destination $file.Name
        if (-not (Test-Path $path)) {
            $failures += "missing $($file.Name)"
            continue
        }
        if ($manifest.ContainsKey($file.Name)) {
            $actual = Get-Sha256 $path
            if ($actual -ne $manifest[$file.Name]) {
                $failures += "hash mismatch for $($file.Name)"
            }
        }
    }
    if ($failures.Count -gt 0) {
        throw ("model verification failed: " + ($failures -join "; "))
    }
    Write-Host "Model verification passed: $Destination"
}

if ($VerifyOnly) {
    Verify-Models
    exit 0
}

New-Item -ItemType Directory -Force -Path $Destination | Out-Null

foreach ($file in $files) {
    $target = Join-Path $Destination $file.Name
    if ((Test-Path $target) -and -not $Force) {
        Write-Host "Keeping existing $($file.Name)"
        continue
    }
    if ($FromDirectory) {
        $source = Join-Path ([System.IO.Path]::GetFullPath($FromDirectory)) $file.Name
        if (-not (Test-Path $source)) {
            throw "missing source model: $source"
        }
        Copy-Item -Path $source -Destination $target -Force
    } else {
        Write-Host "Downloading $($file.Name)"
        Invoke-WebRequest -Uri $file.Url -OutFile $target -UseBasicParsing
    }
}

$hashes = [ordered]@{}
foreach ($file in $files) {
    $path = Join-Path $Destination $file.Name
    if (Test-Path $path) {
        $hashes[$file.Name] = Get-Sha256 $path
    }
}
$hashes | ConvertTo-Json | Set-Content -Path $manifestPath -Encoding UTF8
Write-Host "Models ready: $Destination"
Verify-Models
