param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$OutDir
)

$ErrorActionPreference = 'Stop'
$utf8 = [System.Text.UTF8Encoding]::new($false)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$textPath = Join-Path $OutDir 'document.txt'
$htmlPath = Join-Path $OutDir 'document.html'
$metaPath = Join-Path $OutDir 'metadata.json'
$filesDir = Join-Path $OutDir 'document.files'

# Remove only our own previous outputs; never delete unrelated files.
Remove-Item $textPath, $htmlPath, $metaPath -Force -ErrorAction SilentlyContinue
if (Test-Path $filesDir) { Remove-Item $filesDir -Recurse -Force -ErrorAction SilentlyContinue }

$word = $null
$doc = $null
$failed = $false
$warning = $null
$images = @()
$tables = 0
$inlineShapes = 0
$shapes = 0
$pages = 0

try {
    $word = New-Object -ComObject Word.Application
    $word.Visible = $false
    $word.DisplayAlerts = 0
    # Disable macros before opening the document.
    $word.AutomationSecurity = 3
    $doc = $word.Documents.Open($Path, $false, $true, $false)

    $text = $doc.Content.Text
    [IO.File]::WriteAllText($textPath, $text, $utf8)
    $tables = [int]$doc.Tables.Count
    $inlineShapes = [int]$doc.InlineShapes.Count
    $shapes = [int]$doc.Shapes.Count

    try {
        $doc.ActiveWindow.View.Type = 3
        $doc.Repaginate()
        $pages = [int]$doc.ComputeStatistics(2)
    } catch {
        $pages = 0
    }

    try {
        # wdFormatHTML = 8. Word writes embedded images to document.files.
        $doc.SaveAs2($htmlPath, 8)
        if (Test-Path $filesDir) {
            $images = @(Get-ChildItem $filesDir -File | ForEach-Object { $_.FullName })
        }
    } catch {
        $warning = "image export failed: $($_.Exception.Message)"
    }

    $meta = [ordered]@{
        ok = $true
        source = $Path
        text_path = $textPath
        html_path = if (Test-Path $htmlPath) { $htmlPath } else { $null }
        images_dir = if (Test-Path $filesDir) { $filesDir } else { $null }
        images = @($images)
        image_count = @($images).Count
        tables = $tables
        inline_shapes = $inlineShapes
        shapes = $shapes
        pages = $pages
        warning = $warning
    }
    [IO.File]::WriteAllText($metaPath, ($meta | ConvertTo-Json -Depth 6), $utf8)
} catch {
    $failed = $true
    [Console]::Error.WriteLine("doc-extract: $($_.Exception.Message)")
} finally {
    if ($null -ne $doc) {
        try { $doc.Close($false) } catch {}
        try { [System.Runtime.InteropServices.Marshal]::ReleaseComObject($doc) | Out-Null } catch {}
        $doc = $null
    }
    if ($null -ne $word) {
        try { $word.Quit() } catch {}
        try { [System.Runtime.InteropServices.Marshal]::ReleaseComObject($word) | Out-Null } catch {}
        $word = $null
    }
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}

if ($failed) { exit 1 }
Write-Output "doc-extract: wrote $metaPath"
