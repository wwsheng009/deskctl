[CmdletBinding()]
param(
    [string]$Destination = '',
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
if (-not $Destination) {
    $Destination = Join-Path $PSScriptRoot '..\models'
}
$Destination = [System.IO.Path]::GetFullPath($Destination)
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

$files = @(
    @{
        Name = 'ch_PP-OCRv4_det.onnx'
        Url  = 'https://hf-mirror.com/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_det_infer.onnx'
    },
    @{
        Name = 'ch_PP-OCRv4_rec.onnx'
        Url  = 'https://hf-mirror.com/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_rec_infer.onnx'
    },
    @{
        Name = 'ch_PP-OCR_keys_v1.txt'
        Url  = 'https://gitee.com/paddlepaddle/PaddleOCR/raw/main/ppocr/utils/ppocr_keys_v1.txt'
    },
    @{
        Name = 'icon_detect.onnx'
        Url  = 'https://hf-mirror.com/onnx-community/OmniParser-icon_detect_640x640/resolve/main/onnx/model.onnx'
    }
)

foreach ($file in $files) {
    $path = Join-Path $Destination $file.Name
    if ((Test-Path $path) -and -not $Force) {
        Write-Host "keep $($file.Name)"
        continue
    }
    Write-Host "download $($file.Name)"
    Invoke-WebRequest -Uri $file.Url -OutFile $path -UseBasicParsing
}

$hashes = [ordered]@{}
Get-ChildItem $Destination -File | Where-Object {
    $_.Name -in @('ch_PP-OCRv4_det.onnx', 'ch_PP-OCRv4_rec.onnx', 'ch_PP-OCR_keys_v1.txt', 'icon_detect.onnx')
} | Sort-Object Name | ForEach-Object {
    $hashes[$_.Name] = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant()
}
if ($hashes.Count -ne 4) {
    throw "expected 4 model files, found $($hashes.Count)"
}
$hashes | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $Destination 'hashes.json')
Write-Host "models ready: $Destination"

