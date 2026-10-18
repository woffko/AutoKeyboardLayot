param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ExpectedSha256,
    [Parameter(Mandatory=$true)][string]$InstalledAppHash,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [ValidateSet('retry', 'helper-failure', 'timer-exception', 'no-store', 'uninstall-confirmation', 'missing-exe',
                 'startup-removal', 'fence-invalid', 'probe')]
    [string]$Scenario = 'retry',
    [string]$Language = 'Deutsch'
)
# Installer acceptance in the tested user's interactive session, against the real setup:
#   retry                   check a language, Next is refused while another process holds the installation
#                           fence, the selection stays, Next after the fence is free installs it; the fence is
#                           free and the program starts while the Finished page is open; the unchecked startup
#                           task leaves no Run value
#   helper-failure          the package helper dies: the page offers English only and setup completes
#   timer-exception         the page cannot trust the helper's first reply (a planted reply-0 of another
#                           session): it stops waiting, and Cancel ends setup
#   no-store                an existing English-only profile without a package store: setup continues with
#                           English only and leaves the profile unchanged
#   uninstall-confirmation  answering No in the uninstaller leaves the running program alone; Yes closes it
#                           and removes it
#   missing-exe             a silent uninstall succeeds when the installed program is missing
#   startup-removal         a reinstall with the startup task unchecked removes the Run value
#   fence-invalid           a lock file that is not empty is reported by name and setup installs nothing
#   probe                   the installation-disabled UI probe build (/DInstallerUiProbe): every Next with a
#                           language is refused and can be repeated, the helper never receives a download or
#                           install command, Cancel ends it, and no profile is created
# The language list comes from the published catalog (network). The script refuses to run when the user
# already has a profile or an installation, and removes what it created.
$ErrorActionPreference = 'Stop'
if ($ExpectedSha256 -notmatch '^[a-f0-9]{64}$' -or $InstalledAppHash -notmatch '^[a-f0-9]{64}$' -or
    (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash -ne $ExpectedSha256) { throw 'Setup hash mismatch.' }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class SetupWindows {
  public delegate bool Callback(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumWindows(Callback c, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr h, Callback c, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder b, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr Message(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static string Text(IntPtr h) { var b = new StringBuilder(8192); SendText(h, 0x000D, (IntPtr)b.Capacity, b); return b.ToString(); }
  public static string Class(IntPtr h) { var b = new StringBuilder(256); GetClassName(h, b, 256); return b.ToString(); }
  public static uint Owner(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static string Row(IntPtr h, int index) { var b = new StringBuilder(1024); SendText(h, 0x189, (IntPtr)index, b); return b.ToString(); }
  public static IntPtr[] Windows() { var a = new List<IntPtr>(); EnumWindows((h, p) => { a.Add(h); return true; }, IntPtr.Zero); return a.ToArray(); }
  public static IntPtr[] Children(IntPtr h) { var a = new List<IntPtr>(); EnumChildWindows(h, (c, p) => { a.Add(c); return true; }, IntPtr.Zero); return a.ToArray(); }
}
'@
$appId = '{D913907B-2031-4C26-A199-BF60B0E51B6D}'
$registration = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\${appId}_is1"
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$profilePath = Join-Path $env:LOCALAPPDATA 'AutoKeyboardLayot'
$fencePath = Join-Path $env:LOCALAPPDATA 'AutoKeyboardLayot.installation.lock'
$installTarget = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-acceptance-' + [guid]::NewGuid().ToString('N'))
$installedExe = Join-Path $installTarget 'AutoKeyboardLayot.exe'
$fenceExisted = Test-Path -LiteralPath $fencePath
$script:owned = @{}
$script:held = $null
$result = [ordered]@{state='failed'; scenario=$Scenario; executable_sha256=$ExpectedSha256; network_used=$false}

function Write-Result {
    $stream = [IO.File]::Open((Join-Path $OutputDirectory 'result.json'), [IO.FileMode]::Create)
    try { $b = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 6)); $stream.Write($b, 0, $b.Length) } finally { $stream.Dispose() }
}
function Check([string]$Name, [bool]$Value) {
    $result[$Name] = $Value
    if (-not $Value) { throw "Check failed: $Name" }
}
function Start-Owned([string]$File, [string]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $File; $info.Arguments = $Arguments; $info.UseShellExecute = $false
    $process = [Diagnostics.Process]::Start($info)
    $script:owned[$process.Id] = $process
    return $process
}
# Setup and the uninstaller restart themselves from a temporary copy; their windows belong to children.
function Refresh-Owned {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
    for ($pass = 0; $pass -lt 3; $pass++) {
        foreach ($item in $all) {
            if ($script:owned.ContainsKey([int]$item.ParentProcessId) -and -not $script:owned.ContainsKey([int]$item.ProcessId)) {
                try { $script:owned[[int]$item.ProcessId] = [Diagnostics.Process]::GetProcessById([int]$item.ProcessId) } catch {}
            }
        }
    }
}
function Owned([IntPtr]$Window) {
    $id = [int][SetupWindows]::Owner($Window)
    return $script:owned.ContainsKey($id) -and -not $script:owned[$id].HasExited
}
function Owned-Windows {
    Refresh-Owned
    return @([SetupWindows]::Windows() | Where-Object { (Owned $_) -and [SetupWindows]::IsWindowVisible($_) })
}
function Find-Control([string]$ClassLike, [string]$TextLike) {
    foreach ($window in Owned-Windows) {
        foreach ($control in [SetupWindows]::Children($window)) {
            if (-not [SetupWindows]::IsWindowVisible($control)) { continue }
            if ($ClassLike -and [SetupWindows]::Class($control) -notlike $ClassLike) { continue }
            if ($TextLike -and [SetupWindows]::Text($control).Replace('&', '') -notlike $TextLike) { continue }
            return $control
        }
    }
    return [IntPtr]::Zero
}
function Wait-Control([string]$ClassLike, [string]$TextLike, [int]$Seconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $control = Find-Control $ClassLike $TextLike
        if ($control -ne [IntPtr]::Zero) { return $control }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Control timeout: $TextLike"
}
function Click([IntPtr]$Control) {
    if (-not (Owned $Control)) { throw 'Control of another process.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(3)
    while (-not [SetupWindows]::IsWindowEnabled($Control) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 50 }
    if (-not [SetupWindows]::IsWindowEnabled($Control)) { throw ('Disabled control: ' + [SetupWindows]::Text($Control)) }
    if (-not [SetupWindows]::PostMessage($Control, 0xF5, [IntPtr]::Zero, [IntPtr]::Zero)) { throw 'Click failed.' }
    Start-Sleep -Milliseconds 300
}
function Next-Button { return Find-Control '*Button*' 'Next*' }
function Wizard-Text {
    $texts = foreach ($window in Owned-Windows) { [SetupWindows]::Children($window) | Where-Object { [SetupWindows]::IsWindowVisible($_) } | ForEach-Object { [SetupWindows]::Text($_) } }
    return ($texts -join "`n")
}
function Wait-Text([string]$Like, [int]$Seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do { if ((Wizard-Text) -like $Like) { return }; Start-Sleep -Milliseconds 300 } while ([DateTime]::UtcNow -lt $deadline)
    throw "Text timeout: $Like"
}
function Fence-Free {
    try { $f = [IO.File]::Open($fencePath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None); $f.Dispose(); return $true }
    catch { return $false }
}
function Package-List { return Find-Control 'TNewCheckListBox' $null }
# Advances with Next until the language list shows the published catalog's rows. The tasks page
# has a check list too; the language page is told apart by its title.
function Reach-LanguageList {
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    while ([DateTime]::UtcNow -lt $deadline) {
        $list = Package-List
        if ($list -ne [IntPtr]::Zero -and (Wizard-Text) -like '*Check package catalog*') {
            $rows = [SetupWindows]::Message($list, 0x18B, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
            if ($rows -gt 0) { $result.catalog_rows = $rows; $result.network_used = $true; return $list }
        } elseif ((Wizard-Text) -notlike '*Check package catalog*') {
            $next = Next-Button
            if ($next -ne [IntPtr]::Zero -and [SetupWindows]::IsWindowEnabled($next)) { Click $next }
        }
        Start-Sleep -Milliseconds 300
    }
    throw 'The language list was not reached.'
}
function Check-Language([IntPtr]$List) {
    $rows = [SetupWindows]::Message($List, 0x18B, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
    for ($index = 0; $index -lt $rows; $index++) {
        if ([SetupWindows]::Row($List, $index) -like "$Language*") {
            [void][SetupWindows]::Message($List, 0x186, [IntPtr]$index, [IntPtr]::Zero)
            [void][SetupWindows]::PostMessage($List, 0x100, [IntPtr]32, [IntPtr]::Zero)
            [void][SetupWindows]::PostMessage($List, 0x101, [IntPtr]32, [IntPtr]::Zero)
            Start-Sleep -Milliseconds 400
            return
        }
    }
    throw "Language row missing: $Language"
}
# Ready page, installation and the Finished page; $AtFinished runs while it is shown.
function Finish-Wizard([scriptblock]$AtFinished) {
    $deadline = [DateTime]::UtcNow.AddSeconds(240)
    while ([DateTime]::UtcNow -lt $deadline) {
        $finish = Find-Control '*Button*' 'Finish*'
        if ($finish -ne [IntPtr]::Zero -and [SetupWindows]::IsWindowEnabled($finish)) {
            if ($AtFinished) { & $AtFinished }
            Click $finish
            return
        }
        $install = Find-Control '*Button*' 'Install'
        if ($install -ne [IntPtr]::Zero -and [SetupWindows]::IsWindowEnabled($install)) { Click $install }
        else {
            $next = Next-Button
            if ($next -ne [IntPtr]::Zero -and [SetupWindows]::IsWindowEnabled($next)) { Click $next }
        }
        Start-Sleep -Milliseconds 400
    }
    throw 'The Finished page was not reached.'
}
function Wait-Exit([Diagnostics.Process]$Process, [int]$Seconds) {
    if (-not $Process.WaitForExit($Seconds * 1000)) { throw 'Process did not exit.' }
    $code = $Process.ExitCode
    # The uninstaller's first phase may end before the copy it started; wait for every helper.
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        Refresh-Owned
        $running = @($script:owned.Values | Where-Object { -not $_.HasExited -and $_.ProcessName -ne 'AutoKeyboardLayot' })
        if ($running.Count -eq 0) { return $code }
        Start-Sleep -Milliseconds 300
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'Setup processes did not exit.'
}
function Silent-Setup([string]$Extra) {
    $script:silentLog = Join-Path $OutputDirectory ('silent-' + [guid]::NewGuid().ToString('N') + '.log')
    $p = Start-Owned $Executable ('/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LANG=english /DIR="' + $installTarget + '" /LOG="' + $script:silentLog + '" ' + $Extra)
    return Wait-Exit $p 300
}
function Uninstaller {
    $value = (Get-ItemProperty -LiteralPath $registration -Name UninstallString).UninstallString
    return [regex]::Match($value, '(?i)"?([^"]+\.exe)"?').Groups[1].Value
}
function Silent-Uninstall {
    $p = Start-Owned (Uninstaller) '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART'
    return Wait-Exit $p 300
}
function Agent-Processes {
    return @(Get-CimInstance Win32_Process -Filter "Name='AutoKeyboardLayot.exe'" | Where-Object { $_.ExecutablePath -eq $installedExe })
}
function Start-Agent {
    $p = Start-Owned $installedExe ''
    Start-Sleep -Seconds 4
    return -not $p.HasExited
}
function Close-Agent {
    if (@(Agent-Processes).Count -eq 0) { return }
    $p = Start-Owned $installedExe ('--prepare-upgrade "' + $installTarget + '"')
    $result.agent_close_exit = Wait-Exit $p 60
    $result.agents_closed = @(Agent-Processes).Count -eq 0
}
function Setup-Session-Directory([string]$Log) {
    $line = @(Get-Content -LiteralPath $Log -ErrorAction SilentlyContinue | Where-Object { $_ -match 'Created temporary directory: ' })
    if ($line.Count -eq 0) { return $null }
    return Join-Path ($line[0] -split 'Created temporary directory: ', 2)[1] 'package-session-1'
}
function Cancel-Setup([Diagnostics.Process]$Setup) {
    Click (Find-Control '*Button*' 'Cancel')
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do { $yes = Find-Control '*Button*' 'Yes'; Start-Sleep -Milliseconds 300 } while ($yes -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline)
    Check 'cancel_asks_to_exit' ($yes -ne [IntPtr]::Zero)
    Click $yes
    Check 'setup_cancelled' ((Wait-Exit $Setup 60) -ne 0)
}
function Store-Packages {
    $current = Join-Path $profilePath 'packages\CURRENT'
    if (-not (Test-Path -LiteralPath $current)) { return $null }
    $hash = (Get-Content -LiteralPath $current -Raw).Trim()
    $state = Get-Content -LiteralPath (Join-Path $profilePath "packages\state-$hash.json") -Raw | ConvertFrom-Json
    return @($state.installed | ForEach-Object { $_.id })
}
function Installed-Hash {
    if (-not (Test-Path -LiteralPath $installedExe)) { return $false }
    return (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash -eq $InstalledAppHash
}

try {
    if (Test-Path -LiteralPath $profilePath) { throw 'The user already has a profile.' }
    if (Test-Path -LiteralPath $registration) { throw 'AutoKeyboardLayot is already installed for this user.' }
    if ($null -ne (Get-ItemProperty -LiteralPath $runKey -Name AutoKeyboardLayot -ErrorAction SilentlyContinue)) { throw 'A Run value already exists.' }
    switch ($Scenario) {
        'retry' {
            if (-not $fenceExisted) { New-Item -ItemType File -Path $fencePath | Out-Null }
            # The same sharing a running agent of this user in another session keeps.
            $script:held = [IO.File]::Open($fencePath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
            $setup = Start-Owned $Executable ('/SP- /LANG=english /DIR="' + $installTarget + '" /LOG="' + (Join-Path $OutputDirectory 'setup.log') + '"')
            $list = Reach-LanguageList
            Check 'next_disabled_without_a_language' (-not [SetupWindows]::IsWindowEnabled((Next-Button)))
            Check-Language $list
            Check 'next_enabled_with_a_language' ([SetupWindows]::IsWindowEnabled((Next-Button)))
            Click (Next-Button)
            Wait-Text '*Another setup or uninstall of AutoKeyboardLayot is running*' 90
            $result.refusal_text = $true
            Start-Sleep -Seconds 1
            Check 'next_enabled_after_the_refusal' ([SetupWindows]::IsWindowEnabled((Next-Button)))
            Check 'list_enabled_after_the_refusal' ([SetupWindows]::IsWindowEnabled((Package-List)))
            $script:held.Dispose(); $script:held = $null
            Click (Next-Button)
            Wait-Text '*Packages installed*' 240
            $result.packages_installed_text = $true
            Finish-Wizard {
                Check 'fence_free_on_the_finished_page' (Fence-Free)
                Check 'agent_starts_on_the_finished_page' (Start-Agent)
            }
            [void](Wait-Exit $setup 60)
            Close-Agent
            Check 'installed_app_hash' (Installed-Hash)
            Check 'language_in_the_store' ((Store-Packages) -contains 'de-de')
            Check 'no_run_value_without_the_startup_task' ($null -eq (Get-ItemProperty -LiteralPath $runKey -Name AutoKeyboardLayot -ErrorAction SilentlyContinue))
        }
        'helper-failure' {
            $setup = Start-Owned $Executable ('/SP- /LANG=english /DIR="' + $installTarget + '" /LOG="' + (Join-Path $OutputDirectory 'setup.log') + '"')
            [void](Reach-LanguageList)
            Refresh-Owned
            $helpers = @($script:owned.Values | Where-Object { -not $_.HasExited -and $_.ProcessName -eq 'installer-package-helper' })
            Check 'helper_found' ($helpers.Count -eq 1)
            $helpers[0].Kill()
            Wait-Text '*Click Next to continue with English only*' 30
            $result.english_only_text = $true
            Check 'next_enabled_after_the_helper_failure' ([SetupWindows]::IsWindowEnabled((Next-Button)))
            Click (Next-Button)
            Finish-Wizard $null
            [void](Wait-Exit $setup 60)
            Check 'installed_app_hash' (Installed-Hash)
            Check 'no_language_installed' (@(Store-Packages).Count -eq 0)
        }
        'timer-exception' {
            $log = Join-Path $OutputDirectory 'setup.log'
            $setup = Start-Owned $Executable ('/SP- /LANG=english /DIR="' + $installTarget + '" /LOG="' + $log + '"')
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            do {
                Start-Sleep -Milliseconds 200
                $line = @(Get-Content -LiteralPath $log -ErrorAction SilentlyContinue | Where-Object { $_ -match 'Created temporary directory: ' })
            } while ($line.Count -eq 0 -and [DateTime]::UtcNow -lt $deadline)
            if ($line.Count -eq 0) { throw 'Setup temporary directory not logged.' }
            # The first helper session directory; the helper refuses a directory that already exists.
            $session = Join-Path ($line[0] -split 'Created temporary directory: ', 2)[1] 'package-session-1'
            New-Item -ItemType Directory -Path $session | Out-Null
            [IO.File]::WriteAllText((Join-Path $session 'reply-0.ini'), "[helper]`r`nformat=1`r`nsession=0123456789abcdef0123456789abcdef`r`nstate=ready`r`npid=4`r`n")
            $deadline = [DateTime]::UtcNow.AddSeconds(60)
            while ((Wizard-Text) -notlike '*Check package catalog*' -and [DateTime]::UtcNow -lt $deadline) {
                $next = Next-Button
                if ($next -ne [IntPtr]::Zero -and [SetupWindows]::IsWindowEnabled($next)) { Click $next }
                Start-Sleep -Milliseconds 300
            }
            Wait-Text '*Package operation failed*' 30
            $result.failure_text = $true
            Cancel-Setup $setup
            Check 'nothing_installed' (-not (Test-Path -LiteralPath $installedExe) -and -not (Test-Path -LiteralPath $registration))
        }
        'fence-invalid' {
            if ($fenceExisted) { throw 'The installation lock file already exists.' }
            [IO.File]::WriteAllText($fencePath, 'x')
            Check 'setup_refused' ((Silent-Setup '') -ne 0)
            $logText = [IO.File]::ReadAllText($script:silentLog)
            Check 'invalid_fence_reported' ($logText -like "*The installation lock file $fencePath cannot be used (code 0)*")
            Check 'nothing_installed' (-not (Test-Path -LiteralPath $installedExe) -and -not (Test-Path -LiteralPath $registration))
        }
        'probe' {
            $log = Join-Path $OutputDirectory 'setup.log'
            $setup = Start-Owned $Executable ('/SP- /LANG=english /DIR="' + $installTarget + '" /LOG="' + $log + '"')
            Check-Language (Reach-LanguageList)
            foreach ($attempt in 1, 2) {
                Click (Next-Button)
                Wait-Text '*Finish or cancel package selection before continuing*' 60
                Start-Sleep -Seconds 1
                Check "next_enabled_after_refusal_$attempt" ([SetupWindows]::IsWindowEnabled((Next-Button)))
                Check "list_enabled_after_refusal_$attempt" ([SetupWindows]::IsWindowEnabled((Package-List)))
            }
            $session = Setup-Session-Directory $log
            $commands = @(Get-ChildItem -LiteralPath $session -Filter 'request-*.json' | ForEach-Object { (Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json).command.action })
            $result.helper_commands = $commands
            Check 'helper_selected_twice' (@($commands | Where-Object { $_ -eq 'select' }).Count -eq 2)
            Check 'no_download_or_install_command' (-not ($commands -contains 'confirm_download') -and -not ($commands -contains 'confirm_install'))
            Cancel-Setup $setup
            Check 'no_profile_created' (-not (Test-Path -LiteralPath $profilePath))
            Check 'nothing_installed' (-not (Test-Path -LiteralPath $installedExe))
        }
        'no-store' {
            # An English-only profile from an earlier version: legacy package mode, no package store.
            Check 'silent_install_exit' ((Silent-Setup '') -eq 0)
            Check 'silent_uninstall_exit' ((Silent-Uninstall) -eq 0)
            $config = Join-Path $profilePath 'config.ini'
            [IO.File]::WriteAllText($config, ([IO.File]::ReadAllText($config) -replace '(?m)^mode=managed\r?$', 'mode=legacy'))
            Remove-Item -LiteralPath (Join-Path $profilePath 'packages') -Recurse -Force
            $before = (Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash
            $setup = Start-Owned $Executable ('/SP- /LANG=english /DIR="' + $installTarget + '" /LOG="' + (Join-Path $OutputDirectory 'setup.log') + '"')
            Check-Language (Reach-LanguageList)
            Click (Next-Button)
            Wait-Text '*This profile keeps no language packages*' 90
            $result.no_store_text = $true
            Check 'next_enabled_without_a_store' ([SetupWindows]::IsWindowEnabled((Next-Button)))
            Click (Next-Button)
            Finish-Wizard $null
            [void](Wait-Exit $setup 60)
            Check 'installed_app_hash' (Installed-Hash)
            Check 'profile_unchanged' ((Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash -eq $before)
            Check 'no_store_created' (-not (Test-Path -LiteralPath (Join-Path $profilePath 'packages')))
        }
        'uninstall-confirmation' {
            Check 'silent_install_exit' ((Silent-Setup '') -eq 0)
            Check 'agent_running' (Start-Agent)
            foreach ($answer in @('No', 'Yes')) {
                $uninstaller = Start-Owned (Uninstaller) ''
                $deadline = [DateTime]::UtcNow.AddSeconds(30)
                do { $button = Find-Control '*Button*' $answer; Start-Sleep -Milliseconds 300 } while ($button -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline)
                if ($button -eq [IntPtr]::Zero) { throw "No $answer button in the uninstaller." }
                $result["confirmation_text_$answer"] = (Wizard-Text) -like '*Are you sure*'
                Click $button
                if ($answer -eq 'No') {
                    [void](Wait-Exit $uninstaller 60)
                    Start-Sleep -Seconds 1
                    $result.agents_after_no = @(Agent-Processes | ForEach-Object { "$($_.ProcessId) $($_.CommandLine)" })
                    Check 'agent_still_running_after_no' (@(Agent-Processes).Count -ge 1)
                    Check 'still_installed_after_no' (Test-Path -LiteralPath $registration)
                } else {
                    $deadline = [DateTime]::UtcNow.AddSeconds(120)
                    while (-not $uninstaller.HasExited -and [DateTime]::UtcNow -lt $deadline) {
                        $ok = Find-Control '*Button*' 'OK'
                        if ($ok -ne [IntPtr]::Zero) { Click $ok }
                        Start-Sleep -Milliseconds 400
                        Refresh-Owned
                    }
                    if (-not $uninstaller.HasExited) { throw 'The uninstaller did not finish.' }
                    Start-Sleep -Seconds 2
                    Check 'agent_closed_after_yes' (@(Agent-Processes).Count -eq 0)
                    Check 'removed_after_yes' (-not (Test-Path -LiteralPath $registration) -and -not (Test-Path -LiteralPath $installedExe))
                }
            }
        }
        'missing-exe' {
            Check 'silent_install_exit' ((Silent-Setup '') -eq 0)
            Remove-Item -LiteralPath $installedExe -Force
            Check 'silent_uninstall_exit' ((Silent-Uninstall) -eq 0)
            Check 'registration_removed' (-not (Test-Path -LiteralPath $registration))
        }
        'startup-removal' {
            Check 'install_with_startup_exit' ((Silent-Setup '/TASKS="startup"') -eq 0)
            $value = (Get-ItemProperty -LiteralPath $runKey -Name AutoKeyboardLayot -ErrorAction SilentlyContinue).AutoKeyboardLayot
            Check 'run_value_with_the_task' ($value -eq ('"' + $installedExe + '"'))
            Check 'reinstall_without_startup_exit' ((Silent-Setup '/MERGETASKS="!startup"') -eq 0)
            Check 'run_value_removed' ($null -eq (Get-ItemProperty -LiteralPath $runKey -Name AutoKeyboardLayot -ErrorAction SilentlyContinue))
        }
    }
    $result.state = 'passed'
} catch {
    $result.error = $_.Exception.Message
    $result.line = $_.InvocationInfo.ScriptLineNumber
    try { $result.visible_text = Wizard-Text } catch {}
} finally {
    if ($null -ne $script:held) { $script:held.Dispose() }
    try { Close-Agent } catch {}
    foreach ($process in $script:owned.Values) { if (-not $process.HasExited) { try { $process.Kill() } catch {} } }
    Start-Sleep -Seconds 1
    if (Test-Path -LiteralPath $registration) { try { $result.cleanup_uninstall_exit = Silent-Uninstall } catch {} }
    Remove-Item -LiteralPath $profilePath -Recurse -Force -ErrorAction SilentlyContinue
    if (-not $fenceExisted) { Remove-Item -LiteralPath $fencePath -Force -ErrorAction SilentlyContinue }
    Remove-ItemProperty -LiteralPath $runKey -Name AutoKeyboardLayot -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $installTarget -Recurse -Force -ErrorAction SilentlyContinue
    $result.clean_afterwards = -not (Test-Path -LiteralPath $profilePath) -and -not (Test-Path -LiteralPath $registration)
    Write-Result
}
if ($result.state -ne 'passed') { exit 1 }
Write-Output 'INSTALLER_ACCEPTANCE_PASSED'
