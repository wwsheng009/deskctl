# Deskctl local security and session-isolation checks.
#
# Covers the AC-SEC / AC-SESS items that can be exercised on a single Windows
# host against a private daemon instance:
#   AC-SEC-002 访问控制 (bad daemon token rejected)
#   AC-SEC-006 字库路径穿越 (dict_name traversal rejected)
#   AC-SEC-009 / AC-SESS-002 会话隔离 (foreign session token rejected)
#   AC-SEC-010 无脚本执行 (no tool executes shell commands)
#   AC-SESS-005 会话关闭 (tokens invalid after close)
#   AC-SESS-007 会话上限 (max_sessions -> resource_busy)
# Usage: pwsh scripts/security-check.ps1 [-Binary <path>]

param(
    [string]$Binary,
    [string]$Socket = '\\.\pipe\deskctl-security',
    [string]$TokenFile,
    [int]$MaxSessions = 2
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $TokenFile) {
    $TokenFile = Join-Path $root 'artifacts\security-daemon.token'
}
if (-not $Binary) {
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $Binary = Join-Path $targetDir 'release\deskctl.exe'
}
if (-not (Test-Path $Binary)) {
    throw "deskctl binary not found at '$Binary'. Run: cargo build --release -p deskctl-cli"
}
$tokenDir = Split-Path -Parent $TokenFile
if ($tokenDir) { New-Item -ItemType Directory -Force -Path $tokenDir | Out-Null }

$script:checks = New-Object System.Collections.Generic.List[object]

function Add-Check {
    param([string]$Id, [string]$Title, [bool]$Passed, [string]$Evidence)
    $script:checks.Add([pscustomobject]@{ id = $Id; title = $Title; passed = $Passed; evidence = $Evidence })
    $flag = if ($Passed) { 'PASS' } else { 'FAIL' }
    Write-Host "[$flag] $Id $Title -- $Evidence"
}

function Invoke-Deskctl {
    param([string[]]$Arguments, [switch]$AllowFailure)
    $output = & $Binary @Arguments 2>&1
    $exit = $LASTEXITCODE
    $text = ($output | Out-String).Trim()
    if (-not $AllowFailure -and $exit -ne 0) {
        throw "deskctl $($Arguments -join ' ') failed ($exit): $text"
    }
    $json = $null
    try {
        $json = $text | ConvertFrom-Json
    } catch {
        $json = $null
    }
    return [pscustomobject]@{ exit = $exit; text = $text; json = $json }
}

$common = @('--socket', $Socket, '--token-file', $TokenFile)
try { Invoke-Deskctl (@('shutdown') + $common) -AllowFailure | Out-Null } catch { }

$daemon = Start-Process -FilePath $Binary -ArgumentList (@('serve') + $common + @('--max-sessions', "$MaxSessions")) -PassThru -WindowStyle Hidden

$ready = $false
$lastError = ''
for ($attempt = 0; $attempt -lt 60; $attempt++) {
    Start-Sleep -Milliseconds 250
    try {
        Invoke-Deskctl (@('session', 'list') + $common) | Out-Null
        $ready = $true
        break
    } catch {
        $lastError = $_.Exception.Message
        if ($daemon.HasExited) { throw "daemon exited early with code $($daemon.ExitCode)" }
    }
}
if (-not $ready) { throw "daemon did not become ready within 15s: $lastError" }

try {
    # AC-SEC-002: a wrong daemon token must be rejected.
    $badToken = Join-Path ([System.IO.Path]::GetTempPath()) 'deskctl-bad.token'
    Set-Content -Path $badToken -Value 'not-the-real-token' -Encoding ascii
    $bad = Invoke-Deskctl (@('session', 'list', '--socket', $Socket, '--token-file', $badToken)) -AllowFailure
    Add-Check 'AC-SEC-002' 'wrong daemon token is rejected' ($bad.exit -ne 0 -and $bad.text -match 'permission_denied') "exit=$($bad.exit) output=$($bad.text)"
    Remove-Item -Path $badToken -Force -ErrorAction SilentlyContinue

    # AC-SEC-010: the CLI surface exposes no shell/script execution command.
    $help = Invoke-Deskctl @('--help')
    $forbidden = @('shell', 'exec', 'script', 'approve', 'reject', 'approval')
    $seen = @($forbidden | Where-Object { $help.text -match "\b$_\b" })
    Add-Check 'AC-SEC-010' 'no shell/script/approval commands in CLI help' ($seen.Count -eq 0) "matches=$($seen -join ',')"

    # Create two sessions for isolation checks.
    $sessionA = (Invoke-Deskctl (@('session', 'create') + $common)).json
    $sessionB = (Invoke-Deskctl (@('session', 'create') + $common)).json

    # AC-SEC-009 / AC-SESS-002: session A's token must not authorize session B.
    $crossed = Invoke-Deskctl (@('call', $sessionB.session_id, 'desktop_screen_size',
            '--session-token', $sessionA.session_token, '--json', '{}') + $common) -AllowFailure
    Add-Check 'AC-SEC-009' 'foreign session token is rejected' ($crossed.exit -ne 0 -and $crossed.text -notmatch '"ok":\s*true') "exit=$($crossed.exit) output=$($crossed.text -replace '\s+', ' ')"

    # AC-SEC-006: dict_name traversal must be rejected.
    $traversal = Invoke-Deskctl (@('call', $sessionA.session_id, 'desktop_find_text',
            '--session-token', $sessionA.session_token, '--json', '{"dict_name":"..\\secret"}') + $common) -AllowFailure
    Add-Check 'AC-SEC-006' 'dict_name traversal is rejected' ($traversal.text -match 'policy_denied|invalid_params') "output=$($traversal.text -replace '\s+', ' ')"

    # AC-SESS-007: exceeding max_sessions returns resource_busy.
    $overflow = Invoke-Deskctl (@('session', 'create') + $common) -AllowFailure
    Add-Check 'AC-SESS-007' "session limit ($MaxSessions) returns resource_busy" ($overflow.text -match 'resource_busy') "output=$($overflow.text -replace '\s+', ' ')"

    # AC-SESS-005: tokens stop working after the session is closed.
    Invoke-Deskctl (@('session', 'close', $sessionA.session_id, '--session-token', $sessionA.session_token) + $common) | Out-Null
    $afterClose = Invoke-Deskctl (@('call', $sessionA.session_id, 'desktop_screen_size',
            '--session-token', $sessionA.session_token, '--json', '{}') + $common) -AllowFailure
    Add-Check 'AC-SESS-005' 'tokens are invalid after session close' ($afterClose.exit -ne 0) "exit=$($afterClose.exit) output=$($afterClose.text -replace '\s+', ' ')"

    Invoke-Deskctl (@('session', 'close', $sessionB.session_id, '--session-token', $sessionB.session_token) + $common) -AllowFailure | Out-Null
} finally {
    try { Invoke-Deskctl (@('shutdown') + $common) -AllowFailure | Out-Null } catch { }
    $daemon.WaitForExit(10000) | Out-Null
}

$failed = @($script:checks | Where-Object { -not $_.passed })
$report = [pscustomobject]@{
    generated_at = (Get-Date).ToString('s')
    binary       = $Binary
    checks       = $script:checks
    failed       = $failed.Count
}
$artifacts = Join-Path $root 'artifacts'
New-Item -ItemType Directory -Force -Path $artifacts | Out-Null
$path = Join-Path $artifacts 'security-check.json'
$report | ConvertTo-Json -Depth 6 | Set-Content -Path $path -Encoding utf8
Write-Host ''
Write-Host "$($script:checks.Count - $failed.Count)/$($script:checks.Count) checks passed; report: $path"
if ($failed.Count -gt 0) { exit 1 }
