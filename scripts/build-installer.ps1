[CmdletBinding()]
param(
    [string]$Output = ''
)

$ErrorActionPreference = 'Stop'
if (-not $Output) {
    $Output = Join-Path $PSScriptRoot '..\dist'
}
$root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Output = [System.IO.Path]::GetFullPath($Output)
$stage = Join-Path $Output 'deskctl'
$targetDir = if ($env:CARGO_TARGET_DIR) {
    [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
} else {
    Join-Path $root 'target'
}

Push-Location $root
try {
    cargo build --release -p deskctl-cli
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    & (Join-Path $PSScriptRoot 'verify-models.ps1')

    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item (Join-Path $targetDir 'release\deskctl.exe') $stage
    Copy-Item -Recurse 'models' (Join-Path $stage 'models')
    Copy-Item -Recurse 'third_party\onnxruntime\win-x64\onnxruntime.dll' (Join-Path $stage 'models')
    Copy-Item -Recurse 'third_party\onnxruntime\win-x64\onnxruntime_providers_shared.dll' (Join-Path $stage 'models')
    Copy-Item 'LICENSE', 'NOTICE', 'README.md', 'THIRD_PARTY_NOTICES.md' $stage
    Copy-Item -Recurse 'skills' (Join-Path $stage 'skills')
    Copy-Item -Recurse 'contracts' (Join-Path $stage 'contracts')

    # AC-PKG-001 / AC-PKG-006: the package must contain the binary, runtime,
    # models, licenses and skills.
    $required = @(
        'deskctl.exe',
        'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'README.md',
        'models\ch_PP-OCRv4_det.onnx', 'models\ch_PP-OCRv4_rec.onnx',
        'models\ch_PP-OCR_keys_v1.txt', 'models\icon_detect.onnx',
        'models\onnxruntime.dll', 'models\onnxruntime_providers_shared.dll',
        'skills', 'contracts'
    )
    $missing = @($required | Where-Object { -not (Test-Path (Join-Path $stage $_)) })
    if ($missing.Count -gt 0) { throw "package is missing: $($missing -join ', ')" }
    Write-Host "package contents verified ($($required.Count) required entries)"

    $iscc = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($iscc) {
        & $iscc.Source (Join-Path $root 'installer\deskctl.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
    } else {
        $zip = Join-Path $Output 'deskctl-windows-x64.zip'
        Remove-Item -Force $zip -ErrorAction SilentlyContinue
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
        Write-Host "portable package: $zip"

        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
        try {
            $entries = @($archive.Entries | ForEach-Object { $_.FullName })
        } finally {
            $archive.Dispose()
        }
        $leaves = @($entries | ForEach-Object { ($_ -replace '\\', '/').Split('/')[-1] })
        $zipMissing = @(@(
                'deskctl.exe',
                'ch_PP-OCRv4_det.onnx', 'ch_PP-OCRv4_rec.onnx', 'ch_PP-OCR_keys_v1.txt',
                'icon_detect.onnx',
                'onnxruntime.dll', 'onnxruntime_providers_shared.dll',
                'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md'
            ) | Where-Object { $leaves -notcontains $_ })
        $skillEntries = @($entries | Where-Object { ($_ -replace '\\', '/') -match '^skills/.+/SKILL\.md$' })
        if ($zipMissing.Count -gt 0) { throw "zip is missing: $($zipMissing -join ', ')" }
        if ($skillEntries.Count -eq 0) { throw 'zip does not contain any skills/*/SKILL.md' }
        Write-Host "zip verified: $($entries.Count) entries, $($skillEntries.Count) skill files"
    }
} finally {
    Pop-Location
}

