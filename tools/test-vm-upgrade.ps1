param(
    [Parameter(Mandatory=$true)][string]$Stage,
    [Parameter(Mandatory=$true)][string]$OldSetupSha256,
    [Parameter(Mandatory=$true)][string]$NewSetupSha256,
    [Parameter(Mandatory=$true)][string]$OldAppSha256,
    [Parameter(Mandatory=$true)][string]$NewAppSha256,
    [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
$oldSetup = Join-Path $Stage 'old-setup.exe'
$newSetup = Join-Path $Stage 'new-setup.exe'
$installTarget = Join-Path $Stage ('Installed App ' + [char]0xFC)
$profile = 'C:\Users\w0w\AppData\Local\AutoKeyboardLayot'
$registration = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{D913907B-2031-4C26-A199-BF60B0E51B6D}_is1'
$installTask = 'AklUpgradeInstall'; $uninstallTask = 'AklUpgradeUninstall'
$result = [ordered]@{state='failed'; phase='preflight'; session_id=[Diagnostics.Process]::GetCurrentProcess().SessionId; network_used=$false; typing_acceptance=$false; version_string='0.1.0'}

function Hash([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Wait-Task([string]$Name, [int]$Seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Get-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue).State -ne 'Running') { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
function Drop-Task([string]$Name) { Unregister-ScheduledTask -TaskName $Name -Confirm:$false -ErrorAction SilentlyContinue }
function Invoke-Setup([string]$Setup, [string]$Log, [int]$WaitSeconds) {
    Drop-Task $installTask
    $arguments = '/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LANG=english /DIR="' + $installTarget + '" /LOG="' + $Log + '"'
    $action = New-ScheduledTaskAction -Execute $Setup -Argument $arguments -WorkingDirectory $Stage
    $principal = New-ScheduledTaskPrincipal -UserId 'DESKTOP-ELS4LDK\w0w' -LogonType Interactive
    Register-ScheduledTask -TaskName $installTask -Action $action -Principal $principal -Force | Out-Null
    Start-ScheduledTask -TaskName $installTask
    Start-Sleep -Milliseconds 400
    $finished = Wait-Task $installTask $WaitSeconds
    return $finished
}
function Resolve-Uninstaller {
    $items = @(Get-ChildItem -LiteralPath $installTarget -Filter 'unins*.exe' -ErrorAction SilentlyContinue |
        Where-Object { Test-Path -LiteralPath ([IO.Path]::ChangeExtension($_.FullName,'.dat')) } | Sort-Object Name -Descending)
    if ($items.Count -eq 0) { return $null }
    return $items[0].FullName
}

try {
    if ($env:COMPUTERNAME -ne 'DESKTOP-ELS4LDK' -or (Get-CimInstance Win32_ComputerSystem).Manufacturer -notlike '*VMware*') { throw 'Wrong guest' }
    if ((Hash $oldSetup) -ne $OldSetupSha256) { throw 'Old setup hash' }
    if ((Hash $newSetup) -ne $NewSetupSha256) { throw 'New setup hash' }
    Drop-Task $installTask; Drop-Task $uninstallTask
    if ((& "$env:SystemRoot\System32\query.exe" user 2>&1 | Out-String) -notmatch 'console\s+1\s+Active') { throw 'No active console session' }
    if (@(Get-Process -Name AutoKeyboardLayot -ErrorAction SilentlyContinue).Count) { throw 'App already running' }
    if (Test-Path -LiteralPath $profile) { throw 'w0w profile not empty' }
    if (Test-Path -LiteralPath $installTarget) { Remove-Item -LiteralPath $installTarget -Recurse -Force }
    $result.phase = 'install_old'
    $null = Invoke-Setup $oldSetup (Join-Path $Stage 'old-install.log') 180
    $result.old_installer_result = (Get-ScheduledTaskInfo -TaskName $installTask).LastTaskResult
    $result.old_app_hash_ok = ((Hash (Join-Path $installTarget 'AutoKeyboardLayot.exe')) -eq $OldAppSha256)
    $result.old_profile_created = Test-Path -LiteralPath (Join-Path $profile 'config.ini')
    if (-not ($result.old_app_hash_ok -and $result.old_profile_created)) { throw 'Old install did not complete' }
    # User data that must survive the upgrade.
    [IO.File]::WriteAllText((Join-Path $installTarget 'user-owned-sentinel.txt'), 'upgrade-sentinel', (New-Object Text.UTF8Encoding($false)))
    [IO.File]::WriteAllText((Join-Path $profile 'user_dictionary.txt'), "upgrade-marker`r`n", (New-Object Text.UTF8Encoding($false)))
    $configHash = Hash (Join-Path $profile 'config.ini')
    $pointerHash = Hash (Join-Path $profile 'packages\CURRENT')
    $sentinelHash = Hash (Join-Path $installTarget 'user-owned-sentinel.txt')
    $lexiconHash = Hash (Join-Path $profile 'user_dictionary.txt')
    $result.phase = 'upgrade_new'
    $null = Invoke-Setup $newSetup (Join-Path $Stage 'upgrade.log') 180
    $result.upgrade_installer_result = (Get-ScheduledTaskInfo -TaskName $installTask).LastTaskResult
    $result.new_app_hash_ok = ((Hash (Join-Path $installTarget 'AutoKeyboardLayot.exe')) -eq $NewAppSha256)
    $result.binary_changed = ($OldAppSha256 -ne $NewAppSha256) -and $result.new_app_hash_ok
    $result.config_preserved = ((Hash (Join-Path $profile 'config.ini')) -eq $configHash)
    $result.store_preserved = ((Hash (Join-Path $profile 'packages\CURRENT')) -eq $pointerHash)
    $result.sentinel_preserved = ((Hash (Join-Path $installTarget 'user-owned-sentinel.txt')) -eq $sentinelHash)
    $result.lexicon_preserved = ((Hash (Join-Path $profile 'user_dictionary.txt')) -eq $lexiconHash)
    if (-not ($result.new_app_hash_ok -and $result.config_preserved -and $result.store_preserved -and $result.sentinel_preserved -and $result.lexicon_preserved)) { throw 'Upgrade did not replace the binary while preserving data' }
    $result.old_version_upgrade_test = $true
    # Interrupt an install in flight, then confirm the recoverable path.
    $result.phase = 'interrupt'
    $interrupted = -not (Invoke-Setup $newSetup (Join-Path $Stage 'interrupted.log') 1)
    if ($interrupted) { Stop-ScheduledTask -TaskName $installTask -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 800 }
    $result.install_was_interrupted = $interrupted
    $result.interrupted_app_hash = Hash (Join-Path $installTarget 'AutoKeyboardLayot.exe')
    $result.interrupted_config_preserved = ((Hash (Join-Path $profile 'config.ini')) -eq $configHash)
    $result.phase = 'recover'
    $null = Invoke-Setup $newSetup (Join-Path $Stage 'recover.log') 180
    $result.recovery_installer_result = (Get-ScheduledTaskInfo -TaskName $installTask).LastTaskResult
    $result.recovered_app_hash_ok = ((Hash (Join-Path $installTarget 'AutoKeyboardLayot.exe')) -eq $NewAppSha256)
    $result.recovered_config_preserved = ((Hash (Join-Path $profile 'config.ini')) -eq $configHash)
    $result.recovered_store_preserved = ((Hash (Join-Path $profile 'packages\CURRENT')) -eq $pointerHash)
    $result.recovered_sentinel_preserved = ((Hash (Join-Path $installTarget 'user-owned-sentinel.txt')) -eq $sentinelHash)
    if (-not ($result.recovered_app_hash_ok -and $result.recovered_config_preserved -and $result.recovered_store_preserved -and $result.recovered_sentinel_preserved)) { throw 'Recovery install did not restore a complete install' }
    # Baseline restore: uninstall as the target user, then remove the test profile.
    $result.phase = 'uninstall'
    $uninstaller = Resolve-Uninstaller
    if ($uninstaller) {
        Drop-Task $uninstallTask
        $uAction = New-ScheduledTaskAction -Execute $uninstaller -Argument '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -WorkingDirectory $installTarget
        $uPrincipal = New-ScheduledTaskPrincipal -UserId 'DESKTOP-ELS4LDK\w0w' -LogonType Interactive
        Register-ScheduledTask -TaskName $uninstallTask -Action $uAction -Principal $uPrincipal -Force | Out-Null
        Start-ScheduledTask -TaskName $uninstallTask
        $null = Wait-Task $uninstallTask 120
        $result.uninstall_result = (Get-ScheduledTaskInfo -TaskName $uninstallTask).LastTaskResult
    }
    Start-Sleep -Milliseconds 800
    $result.app_removed = -not (Test-Path -LiteralPath (Join-Path $installTarget 'AutoKeyboardLayot.exe'))
    $result.profile_store_preserved = (Test-Path -LiteralPath (Join-Path $profile 'packages\CURRENT'))
    $result.state = 'passed'; $result.phase = 'complete'
} catch {
    $result.error = $_.Exception.Message
    $result.failure_line = $_.InvocationInfo.ScriptLineNumber
} finally {
    Drop-Task $installTask; Drop-Task $uninstallTask
    Get-Process -Name AutoKeyboardLayot -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $profile -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath 'C:\Users\w0w\AppData\Local\AutoKeyboardLayot.installation.lock' -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $installTarget -Recurse -Force -ErrorAction SilentlyContinue
    $result.profile_removed_after_test = -not (Test-Path -LiteralPath $profile)
    $stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew)
    try { $b = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 5)); $stream.Write($b,0,$b.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}
if ($result.state -ne 'passed') { exit 1 }
Write-Output 'VM_UPGRADE_INTERRUPTION_PASSED'
