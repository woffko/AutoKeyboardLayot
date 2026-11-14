param(
    [Parameter(Mandatory=$true)][string]$Stage,
    [Parameter(Mandatory=$true)][string]$SetupSha256,
    [Parameter(Mandatory=$true)][string]$Receipt,
    [Parameter(Mandatory=$true)][string]$ExpectedComputer,
    [Parameter(Mandatory=$true)][string]$LocalAppData,
    [Parameter(Mandatory=$true)][string]$TaskUser
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
$setup = Join-Path $Stage 'setup.exe'
$installTarget = Join-Path $Stage ('Installed App ' + [char]0xFC)
$log = Join-Path $Stage 'blocked-install.log'
$profile = Join-Path $LocalAppData 'AutoKeyboardLayot'
$lock = Join-Path $LocalAppData 'AutoKeyboardLayot.installation.lock'
$registration = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{D913907B-2031-4C26-A199-BF60B0E51B6D}_is1'
$installTask = 'AklCrossSessionInstall'
$result = [ordered]@{state='failed'; phase='preflight'; session_id=[Diagnostics.Process]::GetCurrentProcess().SessionId; network_used=$false; typing_acceptance=$false}

function Hash([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function FenceProbe {
    try { $f = [IO.File]::Open($lock, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None); $f.Dispose(); return 'free' }
    catch { return ($_.Exception.GetType().Name + ': ' + $_.Exception.Message) }
}
function FenceFree { return (FenceProbe) -eq 'free' }
# Waits until the task has started (Running, or a result other than "has not yet run"), at most a
# minute; $false when it never did.
function Wait-TaskStart([string]$Name) {
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Get-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue).State -eq 'Running') { return $true }
        if ((Get-ScheduledTaskInfo -TaskName $Name -ErrorAction SilentlyContinue).LastTaskResult -ne 267011) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
# Waits until the started task is no longer running; $false when it still ran after $Seconds.
function Wait-Task([string]$Name, [int]$Seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Get-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue).State -ne 'Running') { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
function Drop-Task([string]$Name) { Unregister-ScheduledTask -TaskName $Name -Confirm:$false -ErrorAction SilentlyContinue }

$reader = $null
try {
    $manufacturer = (Get-CimInstance Win32_ComputerSystem).Manufacturer
    if ($env:COMPUTERNAME -ne $ExpectedComputer -or ($manufacturer -notlike '*VMware*' -and $manufacturer -notlike '*QEMU*')) { throw 'Wrong guest' }
    if ((Hash $setup) -ne $SetupSha256) { throw 'Setup hash' }
    Drop-Task $installTask
    if ((& "$env:SystemRoot\System32\query.exe" user 2>&1 | Out-String) -notmatch 'console\s+1\s+Active') { throw 'No active console session' }
    if (Test-Path -LiteralPath $profile) { throw 'Test profile not empty' }
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
    $installPrincipal = New-ScheduledTaskPrincipal -UserId $TaskUser -LogonType Interactive
    Register-ScheduledTask -TaskName $installTask -Action $installAction -Principal $installPrincipal -Force | Out-Null
    Start-ScheduledTask -TaskName $installTask
    if (-not (Wait-TaskStart $installTask)) { throw 'Setup did not start' }
    $result.install_finished = Wait-Task $installTask 180
    if (-not $result.install_finished) { throw 'Setup did not finish within three minutes' }
    $result.install_session = 1
    $result.installer_last_result = (Get-ScheduledTaskInfo -TaskName $installTask).LastTaskResult
    $logText = if (Test-Path -LiteralPath $log) { [IO.File]::ReadAllText($log) } else { '' }
    # The fence has its own message (AkFenceBusy), distinct from a busy program's AkCloseBusy.
    $result.blocked_log_reports_fence = $logText -like '*Another setup or uninstall of AutoKeyboardLayot is running*'
    $result.no_install_target = -not (Test-Path -LiteralPath (Join-Path $installTarget 'AutoKeyboardLayot.exe'))
    $result.reader_still_holds = -not (FenceFree)
    if (-not ($result.blocked_log_reports_fence -and $result.no_install_target -and $result.reader_still_holds)) { throw 'Cross-session exclusion did not refuse the installer' }
    $result.phase = 'release'
    $reader.Dispose(); $reader = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    $released = $false
    while ([DateTime]::UtcNow -lt $deadline) { if (FenceFree) { $released = $true; break }; Start-Sleep -Milliseconds 300 }
    $result.fence_released_after_reader_stop = $released
    # The fence must come free by itself once the reader is gone; freeing it by stopping processes
    # would hide the very hold this test looks for.
    if (-not $released) {
        $result.blocking_processes = @(Get-Process -ErrorAction SilentlyContinue |
            Where-Object { $_.ProcessName -like 'AutoKeyboardLayot*' -or $_.ProcessName -like 'setup*' -or $_.ProcessName -like 'is-*' } |
            ForEach-Object { $_.Id.ToString() + ':' + $_.ProcessName + ':s' + $_.SessionId })
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
    # Clean-up only: a setup left from this run (from the stage or its own is-*.tmp copy) must not
    # hold the fence for the next one.
    Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -like ($Stage.TrimEnd('\') + '\*') -or $_.Path -like '*\is-*.tmp\*') } |
        Stop-Process -Force -ErrorAction SilentlyContinue
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
