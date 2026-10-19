# Audit on/off A/B for the D1 gate: "tool-call p95 regression < 5%" (design/audit-history.md §9).
#
# Starts two private daemon instances from the same binary (only DESKCTL_AUDIT
# differs: `off` vs `basic`), drives the hidden `bench` command over the same
# op, and prints p50/p95 for each mode plus the p95 delta. Results feed
# performance-baseline.md.
#
# Usage:
#   pwsh scripts/audit-perf-ab.ps1
#   pwsh scripts/audit-perf-ab.ps1 -Binary target\release\deskctl.exe -Count 300

param(
    [string]$Binary,
    [int]$Count = 200,
    [int]$Warmup = 20,
    [ValidateSet('screen_size', 'window_screenshot')][string]$Op = 'screen_size',
    # Run `basic` before `off` to control for ordering effects.
    [switch]$Reverse
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Binary) {
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $Binary = Join-Path $targetDir 'debug\deskctl.exe'
}
if (-not (Test-Path $Binary)) {
    throw "deskctl binary not found at '$Binary'. Run: cargo build -p deskctl-cli"
}

function Invoke-BenchRun {
    param([string]$Mode, [string]$TokenFile, [string]$Socket, [string]$AuditDir)
    $common = @('--socket', $Socket, '--token-file', $TokenFile)

    # Stale daemon on the same pipe? Ask it to leave.
    try { & $Binary shutdown @common 2>$null | Out-Null } catch { }

    $env:DESKCTL_AUDIT = $Mode
    $env:DESKCTL_AUDIT_DIR = $AuditDir
    $env:DESKCTL_AUDIT_TEXT = 'digest'
    $daemon = Start-Process -FilePath $Binary `
        -ArgumentList (@('serve', '--max-sessions', '4') + $common) `
        -PassThru -WindowStyle Hidden

    $ready = $false
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        Start-Sleep -Milliseconds 250
        try {
            & $Binary session list @common 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) { $ready = $true; break }
        } catch {
            if ($daemon.HasExited) { throw "daemon ($Mode) exited early with code $($daemon.ExitCode)" }
        }
    }
    if (-not $ready) { $daemon.Kill(); throw "daemon ($Mode) did not become ready within 15s" }

    $benchArgs = @('bench', '--op', $Op, '--count', "$Count", '--warmup', "$Warmup") + $common
    $output = & $Binary @benchArgs 2>&1
    if ($LASTEXITCODE -ne 0) { throw "bench ($Mode) failed: $($output -join ' ')" }

    $report = ($output | Out-String) | ConvertFrom-Json
    & $Binary shutdown @common 2>$null | Out-Null
    if (-not $daemon.WaitForExit(10000)) { $daemon.Kill() }

    Remove-Item Env:DESKCTL_AUDIT, Env:DESKCTL_AUDIT_DIR, Env:DESKCTL_AUDIT_TEXT -ErrorAction SilentlyContinue
    return $report
}

$runs = [ordered]@{}
$modes = if ($Reverse) { @('basic', 'off') } else { @('off', 'basic') }
foreach ($mode in $modes) {
    $workDir = Join-Path $env:TEMP "deskctl-audit-ab-$mode"
    New-Item -ItemType Directory -Force -Path $workDir | Out-Null
    $socket = '\\.\pipe\deskctl-audit-ab-' + $mode
    $tokenFile = Join-Path $workDir 'daemon.token'
    $auditDir = Join-Path $workDir 'audit'
    $runs[$mode] = Invoke-BenchRun -Mode $mode -TokenFile $tokenFile -Socket $socket -AuditDir $auditDir
}

$baseline = $runs['off'].latency
$audited = $runs['basic'].latency
$delta = if ($baseline.p95_ms -gt 0) {
    [Math]::Round((($audited.p95_ms - $baseline.p95_ms) / $baseline.p95_ms) * 100, 2)
} else {
    $null
}

Write-Host ("op: {0}   count: {1}   warmup: {2}   binary: {3}" -f $Op, $Count, $Warmup, $Binary)
Write-Host ''
Write-Host ('{0,-8} {1,10} {2,10} {3,10}' -f 'mode', 'p50_ms', 'p95_ms', 'max_ms')
foreach ($mode in @('off', 'basic')) {
    $row = $runs[$mode].latency
    Write-Host ('{0,-8} {1,10} {2,10} {3,10}' -f $mode, $row.p50_ms, $row.p95_ms, $row.max_ms)
}
Write-Host ''
Write-Host ("p95 regression (audit basic vs off): {0}%" -f $delta)
Write-Host ("verdict: {0}" -f $(if ($null -ne $delta -and $delta -lt 5) { 'PASS (<5%)' } else { 'REVIEW' }))
