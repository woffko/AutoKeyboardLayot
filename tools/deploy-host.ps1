<#
.SYNOPSIS
Replaces the per-user AutoKeyboardLayot.exe with a verified build, keeping a backup and rolling back on failure.

.DESCRIPTION
Steps: verify the candidate's SHA-256, refuse to continue when an unexpected agent process or application
window exists, run the candidate's --verify-profile preflight (it must exit 0), copy the installed
executable to AutoKeyboardLayot.exe.before-<BackupSuffix>, ask the running agent to close through its
advertised graceful-close window (WM_CLOSE, at most 10 seconds), replace the executable, verify the
installed hash, start the new agent, verify exactly one agent process with its observer window, and prune
older AutoKeyboardLayot.exe.before-* backups so that the newest KeepBackups remain. Any failure after the
backup was made restores the backup and restarts the previous agent when one was running.

The installation directory is derived from %LOCALAPPDATA%. With -WhatIf the script performs only read-only
checks, prints the plan and changes nothing (the candidate preflight is not run). -Confirm asks before
the replacement starts.

.EXAMPLE
powershell -NoProfile -File deploy-host.ps1 -Source .\AutoKeyboardLayot.exe -ExpectedSha256 <64 hex digits> -BackupSuffix r1 -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedSha256,
    [Parameter(Mandatory = $true)][ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$')][string]$BackupSuffix,
    [ValidateRange(1, 100)][int]$KeepBackups = 3
)
$ErrorActionPreference = 'Stop'
# Some read-only cmdlets (Get-FileHash, for one) also honor -WhatIf and then return nothing, so the
# flag is kept in $dryRun and the preference is cleared: every check below runs for real, and the
# script itself stops before the first change.
$dryRun = [bool]$WhatIfPreference
$WhatIfPreference = $false

if ([string]::IsNullOrEmpty($env:LOCALAPPDATA)) { throw 'LOCALAPPDATA is not set.' }
$installDirectory = Join-Path $env:LOCALAPPDATA 'Programs\AutoKeyboardLayot'
$destination = Join-Path $installDirectory 'AutoKeyboardLayot.exe'
$backup = $destination + '.before-' + $BackupSuffix
$expected = $ExpectedSha256.ToLowerInvariant()
$processName = 'AutoKeyboardLayot'

if (-not ('AgentWindow' -as [type])) {
    Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class AgentWindow {
    public delegate bool Callback(IntPtr hwnd, IntPtr arg);
    [DllImport("user32.dll")] public static extern bool EnumWindows(Callback callback, IntPtr arg);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, StringBuilder name, int count);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr hwnd, string name);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp);
    // The observer window of the agent with this process id that advertises graceful close, or zero.
    public static IntPtr Find(uint target) {
        IntPtr found = IntPtr.Zero;
        Callback callback = (window, arg) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner == target) {
                var name = new StringBuilder(256);
                GetClassName(window, name, 256);
                if (name.ToString() == "AutoKeyboardLayot.ObserverWindow" && GetProp(window, "AutoKeyboardLayot.GracefulClose.v1") == new IntPtr(1)) {
                    found = window;
                    return false;
                }
            }
            return true;
        };
        EnumWindows(callback, IntPtr.Zero);
        return found;
    }
}
'@
}

function Get-AgentProcesses { Get-Process -Name $processName -ErrorAction SilentlyContinue }

# Existing backups, newest first; only regular files that match the backup name are ever considered.
function Get-Backups {
    if (-not (Test-Path -LiteralPath $installDirectory -PathType Container)) { return @() }
    @(Get-ChildItem -LiteralPath $installDirectory -Filter 'AutoKeyboardLayot.exe.before-*' -File |
        Where-Object { -not ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) } |
        Sort-Object -Property CreationTimeUtc -Descending)
}

# Closes one agent through its graceful-close window; $true when it exited in time.
function Close-Agent($process) {
    $window = [AgentWindow]::Find([uint32]$process.Id)
    if ($window -eq [IntPtr]::Zero) { return $false }
    if (-not [AgentWindow]::PostMessage($window, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)) { return $false }
    return $process.WaitForExit(10000)
}

# --- Read-only checks (run for -WhatIf as well) ---
if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) { throw 'Candidate executable not found.' }
if (-not (Test-Path -LiteralPath $destination -PathType Leaf)) {
    throw 'No installed executable at the expected location; use the installer for a first installation.'
}
if (Test-Path -LiteralPath $backup) { throw 'Backup already exists; choose another -BackupSuffix.' }
if ((Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'Candidate hash mismatch.'
}
$running = @(Get-AgentProcesses)
if ($running.Count -gt 1) {
    throw 'Multiple application processes (an open Settings window counts); close them and retry.'
}
foreach ($process in $running) {
    if ($process.Path -ne $destination -or $process.MainWindowTitle) {
        throw 'Unexpected process path or open application window; deployment stopped.'
    }
    if ([AgentWindow]::Find([uint32]$process.Id) -eq [IntPtr]::Zero) {
        throw 'The running agent does not advertise graceful close; deployment stopped.'
    }
}

# --- Plan ---
$existing = @(Get-Backups)
$slots = $KeepBackups - 1   # the backup made by this run takes one slot
$keptBackups = @()
$prunedBackups = @()
for ($index = 0; $index -lt $existing.Count; $index++) {
    if ($index -lt $slots) { $keptBackups += $existing[$index] } else { $prunedBackups += $existing[$index] }
}
Write-Output ('PLAN: replace the installed executable with the candidate (sha256 ' + $expected + ')')
Write-Output ('PLAN: back up to ' + $backup)
Write-Output ('PLAN: running agents to close gracefully: ' + $running.Count)
Write-Output ('PLAN: keep the newest ' + $KeepBackups + ' backups; existing backups kept ' + $keptBackups.Count + ', removed ' + $prunedBackups.Count)
foreach ($file in $prunedBackups) { Write-Output ('PLAN: remove old backup ' + $file.Name) }

if ($dryRun) {
    Write-Output 'WHATIF: checks passed; nothing was changed.'
    return
}
if (-not $PSCmdlet.ShouldProcess($destination, 'Replace with the verified candidate (backup, graceful close, restart, rollback on failure)')) {
    return
}

# --- Preflight of the candidate (it reads the profile and must not change it) ---
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = $Source
$start.Arguments = '--verify-profile'
$start.UseShellExecute = $false
$check = [System.Diagnostics.Process]::Start($start)
if (-not $check.WaitForExit(15000)) { throw 'Candidate preflight pending; do not launch another check.' }
if ($check.ExitCode -ne 0) { throw 'Candidate profile preflight failed; installed application unchanged.' }

# --- Replacement with rollback ---
Copy-Item -LiteralPath $destination -Destination $backup
$agent = $null
try {
    foreach ($process in $running) {
        if (-not (Close-Agent $process)) { throw 'Agent did not close gracefully; deployment stopped.' }
    }
    Copy-Item -LiteralPath $Source -Destination $destination -Force
    if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
        throw 'Installed hash mismatch.'
    }
    $agent = Start-Process -FilePath $destination -PassThru
    Start-Sleep -Seconds 5
    $agent.Refresh()
    if ($agent.HasExited) { throw 'New agent exited.' }
    $agents = @(Get-AgentProcesses)
    if ($agents.Count -ne 1 -or $agents[0].Id -ne $agent.Id) { throw 'Expected exactly one agent process after the start.' }
    if ([AgentWindow]::Find([uint32]$agent.Id) -eq [IntPtr]::Zero) { throw 'New agent window unavailable.' }
} catch {
    $failure = $_.Exception.Message
    foreach ($process in @(Get-AgentProcesses)) {
        if (-not (Close-Agent $process)) { Stop-Process -Id $process.Id -Force }
    }
    Copy-Item -LiteralPath $backup -Destination $destination -Force
    if ($running.Count -gt 0) { Start-Process -FilePath $destination }
    throw ('Deployment failed and was rolled back: ' + $failure)
}

# --- Prune old backups, keeping the newest $KeepBackups (this run's backup is the newest) ---
$backups = @(Get-Backups)
$pruned = 0
for ($index = $KeepBackups; $index -lt $backups.Count; $index++) {
    Remove-Item -LiteralPath $backups[$index].FullName
    $pruned++
}

Write-Output ('DEPLOYED_PID=' + $agent.Id + ' PROFILE_PREFLIGHT_EXIT=0')
Write-Output ('EXE_SHA256=' + (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant())
Write-Output ('BACKUP=' + $backup)
Write-Output ('PRUNED=' + $pruned + ' KEPT_BACKUPS=' + @(Get-Backups).Count)
