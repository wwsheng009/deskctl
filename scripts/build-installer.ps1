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

Push-Location $root
try {
    cargo build --release -p deskctl-cli
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    & (Join-Path $PSScriptRoot 'verify-models.ps1')

    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item 'target\release\deskctl.exe' $stage
    Copy-Item -Recurse 'models' (Join-Path $stage 'models')
    Copy-Item -Recurse 'third_party\onnxruntime\win-x64\onnxruntime.dll' (Join-Path $stage 'models')
    Copy-Item -Recurse 'third_party\onnxruntime\win-x64\onnxruntime_providers_shared.dll' (Join-Path $stage 'models')
    Copy-Item 'LICENSE', 'NOTICE', 'README.md', 'THIRD_PARTY_NOTICES.md' $stage
    Copy-Item -Recurse 'skills' (Join-Path $stage 'skills')
    Copy-Item -Recurse 'contracts' (Join-Path $stage 'contracts')

    $iscc = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($iscc) {
        & $iscc.Source (Join-Path $root 'installer\deskctl.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
    } else {
        $zip = Join-Path $Output 'deskctl-windows-x64.zip'
        Remove-Item -Force $zip -ErrorAction SilentlyContinue
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
        Write-Host "portable package: $zip"
    }
} finally {
    Pop-Location
}

