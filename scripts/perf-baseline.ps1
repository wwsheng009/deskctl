# Deskctl local performance baseline (AC-PERF-001..005, 008, 009 smoke).
#
# Starts a private daemon instance on its own pipe/token, drives the hidden
# `deskctl bench` command and records p50/p95/max plus daemon RSS deltas.
# Usage:
#   pwsh scripts/perf-baseline.ps1
#   pwsh scripts/perf-baseline.ps1 -Binary E:\path\to\deskctl.exe -CountScale 2

param(
    [string]$Binary,
    [string]$Socket = '\\.\pipe\deskctl-perf',
    [string]$TokenFile,
    [int]$MaxSessions = 8,
    [double]$CountScale = 1.0
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $TokenFile) {
    $TokenFile = Join-Path $root 'artifacts\perf-daemon.token'
}
if (-not $Binary) {
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $Binary = Join-Path $targetDir 'release\deskctl.exe'
}
if (-not (Test-Path $Binary)) {
    throw "deskctl binary not found at '$Binary'. Run: cargo build --release -p deskctl-cli"
}
$artifacts = Join-Path $root 'artifacts'
New-Item -ItemType Directory -Force -Path $artifacts | Out-Null
$tokenDir = Split-Path -Parent $TokenFile
if ($tokenDir) { New-Item -ItemType Directory -Force -Path $tokenDir | Out-Null }

function Get-Stats {
    param([double[]]$Samples)
    if (-not $Samples -or $Samples.Count -eq 0) {
        return [pscustomobject]@{ count = 0; p50_ms = $null; p95_ms = $null; max_ms = $null }
    }
    $sorted = @($Samples | Sort-Object)
    $pick = {
        param([double]$Quantile)
        $index = [Math]::Min($sorted.Count - 1, [Math]::Round(($sorted.Count - 1) * $Quantile))
        [Math]::Round($sorted[$index], 2)
    }
    [pscustomobject]@{
        count  = $sorted.Count
        p50_ms = & $pick 0.50
        p95_ms = & $pick 0.95
        max_ms = [Math]::Round($sorted[$sorted.Count - 1], 2)
    }
}

function Measure-Run {
    param([scriptblock]$Action, [int]$Iterations)
    $samples = New-Object System.Collections.Generic.List[double]
    for ($i = 0; $i -lt $Iterations; $i++) {
        $watch = [System.Diagnostics.Stopwatch]::StartNew()
        & $Action | Out-Null
        $watch.Stop()
        $samples.Add($watch.Elapsed.TotalMilliseconds)
    }
    return $samples.ToArray()
}

function Invoke-Cli {
    param([string[]]$Arguments)
    $output = & $Binary @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "deskctl $($Arguments -join ' ') failed: $($output -join ' ')"
    }
    return ($output | Out-String).Trim()
}

$cli = { param($Arguments) & $Binary @Arguments | Out-Null }
$common = @('--socket', $Socket, '--token-file', $TokenFile)

Write-Host "binary:  $Binary"
Write-Host "socket:  $Socket"
Write-Host "token:   $TokenFile"

# 0. Make sure no stale daemon holds this pipe.
try { Invoke-Cli (@('shutdown') + $common) | Out-Null } catch { }

$daemon = Start-Process -FilePath $Binary -ArgumentList (@('serve') + $common + @('--max-sessions', "$MaxSessions")) -PassThru -WindowStyle Hidden
$readyWatch = [System.Diagnostics.Stopwatch]::StartNew()
$ready = $false
for ($attempt = 0; $attempt -lt 60; $attempt++) {
    Start-Sleep -Milliseconds 250
    try {
        Invoke-Cli (@('session', 'list') + $common) | Out-Null
        $ready = $true
        break
    } catch {
        if ($daemon.HasExited) { throw "daemon exited early with code $($daemon.ExitCode)" }
    }
}
$readyWatch.Stop()
if (-not $ready) { throw 'daemon did not become ready within 15s' }
$daemonReadyMs = [Math]::Round($readyWatch.Elapsed.TotalMilliseconds, 2)
Write-Host "daemon ready in $daemonReadyMs ms (pid $($daemon.Id))"

$rss = @{}
function Get-DaemonRss { (Get-Process -Id $daemon.Id).WorkingSet64 / 1MB }
$rss['after_ready'] = [Math]::Round((Get-DaemonRss), 1)

$result = [ordered]@{
    generated_at    = (Get-Date).ToString('s')
    machine         = [pscustomobject]@{
        os      = [System.Environment]::OSVersion.VersionString
        cpus    = [System.Environment]::ProcessorCount
        binary  = $Binary
    }
    daemon_ready_ms = $daemonReadyMs
}

# 1. AC-PERF-001: CLI --help start-up.
$helpIterations = [Math]::Max(10, [int](20 * $CountScale))
$helpSamples = Measure-Run -Iterations $helpIterations -Action { & $Binary --help | Out-Null }
$result['cli_help'] = Get-Stats $helpSamples

# 2. AC-PERF-003: IPC round trip.
$ipcIterations = [Math]::Max(20, [int](50 * $CountScale))
$ipcOut = Invoke-Cli (@('bench', '--op', 'screen_size', '--count', "$ipcIterations", '--warmup', '5') + $common)
$result['ipc_round_trip'] = ($ipcOut | ConvertFrom-Json).latency

# 2b. AC-PERF-007: 100 consecutive calls must stay within a 10% RSS budget.
$growthIterations = [Math]::Max(50, [int](100 * $CountScale))
$rssBefore = Get-DaemonRss
Invoke-Cli (@('bench', '--op', 'screen_size', '--count', "$growthIterations", '--warmup', '10') + $common) | Out-Null
$rssAfter = Get-DaemonRss
$growthPct = if ($rssBefore -gt 0) { [Math]::Round((($rssAfter - $rssBefore) / $rssBefore) * 100.0, 2) } else { $null }
$result['memory_growth'] = [pscustomobject]@{
    calls         = $growthIterations
    rss_before_mb = [Math]::Round($rssBefore, 1)
    rss_after_mb  = [Math]::Round($rssAfter, 1)
    growth_pct    = $growthPct
    within_budget = ($null -ne $growthPct -and $growthPct -lt 10)
}

# 3. AC-PERF-004: warm full-screen screenshot.
$shotIterations = [Math]::Max(10, [int](20 * $CountScale))
$shotOut = Invoke-Cli (@('bench', '--op', 'screenshot', '--count', "$shotIterations", '--warmup', '3') + $common)
$result['fullscreen_screenshot'] = ($shotOut | ConvertFrom-Json).latency

# 4. AC-PERF-005: warm window screenshot (skipped when no titled window exists).
try {
    $windowOut = Invoke-Cli (@('bench', '--op', 'window_screenshot', '--count', "$shotIterations", '--warmup', '3') + $common)
    $windowReport = $windowOut | ConvertFrom-Json
    $result['window_screenshot'] = $windowReport.latency
    $result['window_screenshot_target'] = $windowReport.window
} catch {
    $result['window_screenshot'] = [pscustomobject]@{ skipped = $_.Exception.Message }
}

$rss['after_load'] = [Math]::Round((Get-DaemonRss), 1)

# 5. AC-PERF-008: 8 sessions in parallel.
$parallelRounds = [Math]::Max(2, [int](5 * $CountScale))
$parallelOut = Invoke-Cli (@('bench', '--op', 'parallel', '--count', "$parallelRounds", '--parallel', '8') + $common)
$parallelReport = $parallelOut | ConvertFrom-Json
$result['parallel_sessions'] = [pscustomobject]@{
    sessions                     = $parallelReport.sessions
    rounds                       = $parallelReport.rounds
    sessions_closed              = $parallelReport.sessions_closed
    latency                      = $parallelReport.latency
    unique_capture_ids_per_round = $parallelReport.unique_capture_ids_per_round
    failure_count                = @($parallelReport.failures).Count
}

# 5b. AC-PERF-006: warm OCR perception over a fixed 640x400 region.
$ocrSamples = New-Object System.Collections.Generic.List[double]
$ocrSkip = $null
$ocrSession = $null
try {
    $ocrSession = Invoke-Cli (@('session', 'create') + $common) | ConvertFrom-Json
    $ocrImage = Join-Path $artifacts 'perf-ocr-region.png'
    $escapedImage = $ocrImage.Replace('\', '\\')
    $shotArgs = '{"region":{"x":0,"y":0,"width":640,"height":400},"path":"' + $escapedImage + '"}'
    $shot = Invoke-Cli (@('call', $ocrSession.session_id, 'desktop_screenshot',
            '--session-token', $ocrSession.session_token, '--json', $shotArgs) + $common) | ConvertFrom-Json
    if ($shot.ok -ne $true) { throw "screenshot failed: $($shot.error | ConvertTo-Json -Compress)" }
    $perceiveArgs = '{"image_path":"' + $escapedImage + '"}'
    for ($i = 0; $i -lt 4; $i++) {
        $watch = [System.Diagnostics.Stopwatch]::StartNew()
        # OCR text is arbitrary screen content and does not always round-trip
        # through ConvertFrom-Json, so success is judged by the CLI exit code
        # (non-zero whenever the envelope has ok=false).
        Invoke-Cli (@('call', $ocrSession.session_id, 'desktop_perceive',
                '--session-token', $ocrSession.session_token, '--json', $perceiveArgs) + $common) | Out-Null
        $watch.Stop()
        if ($i -gt 0) { $ocrSamples.Add($watch.Elapsed.TotalMilliseconds) }
    }
} catch {
    $ocrSkip = $_.Exception.Message
    if ($ocrSkip.Length -gt 400) { $ocrSkip = $ocrSkip.Substring(0, 400) + '...' }
    Set-Content -Path (Join-Path $artifacts 'ocr-last-error.txt') -Value $_.Exception.Message -Encoding utf8
} finally {
    if ($ocrSession) {
        Invoke-Cli (@('session', 'close', $ocrSession.session_id,
                '--session-token', $ocrSession.session_token) + $common) -ErrorAction SilentlyContinue | Out-Null
    }
}
if ($ocrSkip) {
    $result['ocr_perceive'] = [pscustomobject]@{ skipped = $ocrSkip }
} else {
    $result['ocr_perceive'] = Get-Stats $ocrSamples.ToArray()
}

# 6. AC-PERF-007/009 smoke: RSS after load and after sessions are closed.
Start-Sleep -Seconds 5
$result['rss_mb'] = [pscustomobject]@{
    after_ready = $rss['after_ready']
    after_load  = $rss['after_load']
    after_idle  = [Math]::Round((Get-DaemonRss), 1)
}
$openSessions = (Invoke-Cli (@('session', 'list') + $common) | ConvertFrom-Json)
$result['open_sessions_after_bench'] = @($openSessions).Count

try { Invoke-Cli (@('shutdown') + $common) | Out-Null } catch { }
$daemon.WaitForExit(10000) | Out-Null

$result['screen'] = (Invoke-Cli @('tool', 'desktop_screen_size') | ConvertFrom-Json).data

$path = Join-Path $artifacts 'perf-baseline.json'
$result | ConvertTo-Json -Depth 8 | Set-Content -Path $path -Encoding utf8
Write-Host ''
Write-Host "daemon ready:            $daemonReadyMs ms"
Write-Host "CLI --help:              p50 $($result['cli_help'].p50_ms) / p95 $($result['cli_help'].p95_ms) ms"
Write-Host "IPC round trip:          p50 $($result['ipc_round_trip'].p50_ms) / p95 $($result['ipc_round_trip'].p95_ms) ms"
Write-Host "Memory growth:           $($result['memory_growth'].calls) calls -> $($result['memory_growth'].growth_pct)% RSS (budget < 10%)"
Write-Host "Fullscreen screenshot:   p50 $($result['fullscreen_screenshot'].p50_ms) / p95 $($result['fullscreen_screenshot'].p95_ms) ms"
if ($result['window_screenshot'].p95_ms) {
    $target = $result['window_screenshot_target']
    Write-Host "Window screenshot:       p50 $($result['window_screenshot'].p50_ms) / p95 $($result['window_screenshot'].p95_ms) ms ($($target.width)x$($target.height))"
} else {
    Write-Host "Window screenshot:       skipped ($($result['window_screenshot'].skipped))"
}
Write-Host "Parallel sessions:       $($result['parallel_sessions'].failure_count) failures over $($result['parallel_sessions'].rounds) rounds"
if ($result['ocr_perceive'].p95_ms) {
    Write-Host "OCR perceive (warm):     p50 $($result['ocr_perceive'].p50_ms) / p95 $($result['ocr_perceive'].p95_ms) ms"
} else {
    Write-Host "OCR perceive (warm):     skipped ($($result['ocr_perceive'].skipped))"
}
Write-Host "Daemon RSS (MB):         ready $($result['rss_mb'].after_ready) / load $($result['rss_mb'].after_load) / idle $($result['rss_mb'].after_idle)"
Write-Host "Report:                  $path"
