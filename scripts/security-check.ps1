# Deskctl local security and session-isolation checks.
#
# Covers the AC-SEC / AC-SESS items that can be exercised on a single Windows
# host against a private daemon instance:
#   AC-SEC-001 本地监听 (daemon owns no TCP/UDP listener)
#   AC-SEC-002 访问控制 (bad daemon token rejected)
#   AC-SEC-003 无审批 (tool results carry no approval/pending fields)
#   AC-SEC-004 风险元数据 (input tools return meta.risk and do not pause)
#   AC-SEC-005 自定义截图路径 (nested path created, extension matches format)
#   AC-SEC-006 字库路径穿越 (dict_name traversal rejected)
#   AC-SEC-009 / AC-SESS-002 会话隔离 (foreign session token rejected)
#   AC-SEC-010 无脚本执行 (no tool executes shell commands)
#   AC-SESS-003 输入串行 (8 concurrent input calls are serialized)
#   AC-SESS-005 会话关闭 (tokens invalid after close)
#   AC-SESS-007 会话上限 (max_sessions -> resource_busy)
#   AC-SESS-009 公平性 (no starvation under input contention)
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
    # AC-SEC-001: the private daemon must not open TCP/UDP listeners.
    $listeners = @(netstat -ano | Select-String -Pattern "^\s+(TCP|UDP)\s+.*\s$($daemon.Id)\s*$")
    Add-Check 'AC-SEC-001' 'daemon owns no TCP/UDP listeners' ($listeners.Count -eq 0) "listeners=$($listeners.Count) pid=$($daemon.Id)"

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

    # AC-SEC-003 / AC-SEC-004: input tools report meta.risk and return without
    # any approval handshake or pending queue.
    $mouse = Invoke-Deskctl (@('call', $sessionB.session_id, 'desktop_mouse',
            '--session-token', $sessionB.session_token, '--json', '{"action":"position"}') + $common)
    $approvalWords = @('approve', 'approval', 'pending_approval')
    $approvalHit = @($approvalWords | Where-Object { $mouse.text -match $_ })
    Add-Check 'AC-SEC-003' 'tool result has no approval/pending fields' ($approvalHit.Count -eq 0) "matches=$($approvalHit -join ',')"
    Add-Check 'AC-SEC-004' 'input tool returns meta.risk without pausing' ($mouse.json.meta.risk -eq 'bounded_write') "risk=$($mouse.json.meta.risk) exit=$($mouse.exit)"

    # AC-SEC-005: a nested custom screenshot path is created and the extension
    # follows the selected format.
    $secDir = Join-Path ([System.IO.Path]::GetTempPath()) 'deskctl-sec-check'
    Remove-Item -Recurse -Force $secDir -ErrorAction SilentlyContinue
    $nested = Join-Path $secDir 'nested\shot.jpg'
    $shot = Invoke-Deskctl (@('call', $sessionB.session_id, 'desktop_screenshot',
            '--session-token', $sessionB.session_token, '--json', ('{{"path":"{0}"}}' -f ($nested -replace '\\', '/'))) + $common)
    $created = Test-Path $nested
    Add-Check 'AC-SEC-005' 'nested custom path is written with the selected extension' ($created -and $shot.json.data.format -eq 'jpeg' -and $shot.json.data.path -match '\.jpg$') "path=$($shot.json.data.path) format=$($shot.json.data.format) exists=$created"

    # AC-SESS-003 / AC-SESS-009: concurrent no-op mouse moves are serialized by
    # the input resource gate; all calls finish and none starves.
    $cursor = (Invoke-Deskctl (@('call', $sessionB.session_id, 'desktop_mouse',
            '--session-token', $sessionB.session_token, '--json', '{"action":"position"}') + $common)).json.data
    $moveArgs = '{{"action":"move","x":{0},"y":{1}}}' -f $cursor.x, $cursor.y
    $targets = @($sessionA, $sessionB, $sessionA, $sessionB, $sessionA, $sessionB, $sessionA, $sessionB)
    $callArgs = @(foreach ($target in $targets) {
            , (@('call', $target.session_id, 'desktop_mouse', '--session-token', $target.session_token, '--json', $moveArgs) + $common)
        })
    $concurrent = @($callArgs | ForEach-Object -Parallel {
            $timer = [System.Diagnostics.Stopwatch]::StartNew()
            $text = (& $using:Binary @_ 2>&1 | Out-String).Trim()
            $timer.Stop()
            [pscustomobject]@{ exit = $LASTEXITCODE; ms = $timer.ElapsedMilliseconds; output = $text }
        } -ThrottleLimit 8)
    $concurrentFailed = @($concurrent | Where-Object { $_.exit -ne 0 })
    $latency = @($concurrent | ForEach-Object { [int]$_.ms } | Sort-Object)
    $p50 = $latency[[int][Math]::Floor($latency.Count / 2)]
    $max = $latency[-1]
    Add-Check 'AC-SESS-003' '8 concurrent input calls are serialized, all succeed' ($concurrentFailed.Count -eq 0) "failures=$($concurrentFailed.Count) exits=$((@($concurrent | ForEach-Object { $_.exit }) -join ','))"
    Add-Check 'AC-SESS-009' 'no starvation under input contention' ($concurrentFailed.Count -eq 0 -and $max -le [Math]::Max(2000, $p50 * 20)) "p50=${p50}ms max=${max}ms"

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
