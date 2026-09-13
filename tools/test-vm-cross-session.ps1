param(
    [Parameter(Mandatory=$true)][string]$Stage,
    [Parameter(Mandatory=$true)][string]$SetupSha256,
    [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
$setup = Join-Path $Stage 'setup.exe'
$installTarget = Join-Path $Stage ('Installed App ' + [char]0xFC)
$log = Join-Path $Stage 'blocked-install.log'
$profile = 'C:\Users\w0w\AppData\Local\AutoKeyboardLayot'
$lock = 'C:\Users\w0w\AppData\Local\AutoKeyboardLayot.installation.lock'
$registration = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{D913907B-2031-4C26-A199-BF60B0E51B6D}_is1'
$installTask = 'AklCrossSessionInstall'
$result = [ordered]@{state='failed'; phase='preflight'; session_id=[Diagnostics.Process]::GetCurrentProcess().SessionId; network_used=$false; typing_acceptance=$false}

function Hash([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function FenceProbe {
    try { $f = [IO.File]::Open($lock, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None); $f.Dispose(); return 'free' }
    catch { return ($_.Exception.GetType().Name + ': ' + $_.Exception.Message) }
}
function FenceFree { return (FenceProbe) -eq 'free' }
function Wait-Task([string]$Name, [int]$Seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Get-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue).State -ne 'Running') { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}
function Drop-Task([string]$Name) { Unregister-ScheduledTask -TaskName $Name -Confirm:$false -ErrorAction SilentlyContinue }

$reader = $null
try {
    if ($env:COMPUTERNAME -ne 'DESKTOP-ELS4LDK' -or (Get-CimInstance Win32_ComputerSystem).Manufacturer -notlike '*VMware*') { throw 'Wrong guest' }
    if ((Hash $setup) -ne $SetupSha256) { throw 'Setup hash' }
    Drop-Task $installTask
    if ((& "$env:SystemRoot\System32\query.exe" user 2>&1 | Out-String) -notmatch 'console\s+1\s+Active') { throw 'No active console session' }
    if (Test-Path -LiteralPath $profile) { throw 'w0w profile not empty' }
    if (Test-Path -LiteralPath $installTarget) { throw 'Install target not clear' }
    Remove-Item -LiteralPath $log -Force -ErrorAction SilentlyContinue
    $lockExistedBefore = Test-Path -LiteralPath $lock
    if (-not $lockExistedBefore) { New-Item -ItemType File -Path $lock -Force | Out-Null }
    # Session 0 holds the shared installation fence for the target user (the same
    # sharing the running agent uses); session 1 then runs the installer as that
    # user and must be refused by the exclusive fence open.
    $result.phase = 'fence_held_session0'
    $reader = [IO.File]::Open($lock, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $result.session0_reader_holds_fence = -not (FenceFree)
    if (-not $result.session0_reader_holds_fence) { throw 'Session 0 reader did not hold the fence' }
    $result.phase = 'blocked_install_session1'
    $arguments = '/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LANG=english /DIR="' + $installTarget + '" /LOG="' + $log + '"'
    $installAction = New-ScheduledTaskAction -Execute $setup -Argument $arguments -WorkingDirectory $Stage
    $installPrincipal = New-ScheduledTaskPrincipal -UserId 'DESKTOP-ELS4LDK\w0w' -LogonType Interactive
    Register-ScheduledTask -TaskName $installTask -Action $installAction -Principal $installPrincipal -Force | Out-Null
    Start-ScheduledTask -TaskName $installTask
    Start-Sleep -Seconds 1
    $null = Wait-Task $installTask 180
    $result.install_session = 1
    $result.installer_last_result = (Get-ScheduledTaskInfo -TaskName $installTask).LastTaskResult
    $logText = if (Test-Path -LiteralPath $log) { [IO.File]::ReadAllText($log) } else { '' }
    $result.blocked_log_has_code12 = $logText -like '*Helper code: 12*'
    $result.no_install_target = -not (Test-Path -LiteralPath (Join-Path $installTarget 'AutoKeyboardLayot.exe'))
    $result.reader_still_holds = -not (FenceFree)
    if (-not ($result.blocked_log_has_code12 -and $result.no_install_target -and $result.reader_still_holds)) { throw 'Cross-session exclusion did not refuse the installer' }
    $result.phase = 'release'
    $reader.Dispose(); $reader = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    $released = $false
    while ([DateTime]::UtcNow -lt $deadline) { if (FenceFree) { $released = $true; break }; Start-Sleep -Milliseconds 300 }
    if (-not $released) {
        $procs = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -like 'AutoKeyboardLayot*' -or $_.ProcessName -like 'setup*' -or $_.ProcessName -like 'is-*' })
        $result.blocking_processes = @($procs | ForEach-Object { $_.Id.ToString() + ':' + $_.ProcessName + ':s' + $_.SessionId })
        $procs | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 1000
        $result.fence_free_after_kill = FenceFree
    }
    $result.fence_released_after_reader_stop = $released
    if (-not ($released -or $result.fence_free_after_kill -eq $true)) {
        $result.fence_probe = FenceProbe
        $result.lock_exists = Test-Path -LiteralPath $lock
        if ($result.lock_exists) { $result.lock_attributes = (Get-Item -LiteralPath $lock -Force).Attributes.ToString() }
        throw 'Fence was not released after the reader stopped'
    }
    $result.state = 'passed'; $result.phase = 'complete'
} catch {
    $result.error = $_.Exception.Message
    $result.failure_line = $_.InvocationInfo.ScriptLineNumber
} finally {
    if ($null -ne $reader) { $reader.Dispose() }
    Drop-Task $installTask
    if (-not (Test-Path -LiteralPath $registration)) {
        Remove-Item -LiteralPath $profile -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $lock -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $installTarget -Recurse -Force -ErrorAction SilentlyContinue
    $result.profile_removed_after_test = -not (Test-Path -LiteralPath $profile)
    $stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew)
    try { $b = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 5)); $stream.Write($b,0,$b.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}
if ($result.state -ne 'passed') { exit 1 }
Write-Output 'VM_CROSS_SESSION_EXCLUSION_PASSED'
