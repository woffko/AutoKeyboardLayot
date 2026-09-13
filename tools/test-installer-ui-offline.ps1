param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ExpectedSha256,
    [Parameter(Mandatory=$true)][string]$Catalog,
    [Parameter(Mandatory=$true)][string]$InstalledAppHash,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
if ($ExpectedSha256 -notmatch '^[a-f0-9]{64}$' -or
    (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash -ne $ExpectedSha256) { throw 'Setup hash mismatch.' }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Drawing;
public static class ProbeWindows {
  public delegate bool Callback(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumWindows(Callback c, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr h, Callback c, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder b, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder b, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SetText(IntPtr h, uint m, IntPtr w, string text);
  [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr Message(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] static extern IntPtr ListMessage(IntPtr h, uint m, IntPtr w, StringBuilder b);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] static extern IntPtr SendMessageText(IntPtr h, uint m, IntPtr w, StringBuilder l);
  public static string GetText(IntPtr h, int capacity) { var b = new StringBuilder(capacity); SendMessageText(h, 0x000D, (IntPtr)capacity, b); return b.ToString(); }
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out Rect r);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] static extern bool RedrawWindow(IntPtr h, IntPtr rect, IntPtr region, uint flags);
  [StructLayout(LayoutKind.Sequential)] struct Rect { public int L,T,R,B; }
  public static string Text(IntPtr h) { var b = new StringBuilder(8192); GetWindowText(h,b,b.Capacity); return b.ToString(); }
  public static uint Owner(IntPtr h) { uint p; GetWindowThreadProcessId(h,out p); return p; }
  public static string Class(IntPtr h) { var b=new StringBuilder(256); GetClassName(h,b,256); return b.ToString(); }
  public static string Row(IntPtr h, int index) { var b=new StringBuilder(8192); ListMessage(h,0x189,(IntPtr)index,b); return b.ToString(); }
  public static IntPtr[] Windows() { var a=new List<IntPtr>(); EnumWindows((h,p)=>{a.Add(h);return true;},IntPtr.Zero); return a.ToArray(); }
  public static IntPtr[] Children(IntPtr h) { var a=new List<IntPtr>(); EnumChildWindows(h,(c,p)=>{a.Add(c);return true;},IntPtr.Zero); return a.ToArray(); }
  public static void Capture(IntPtr h, string path) {
    RedrawWindow(h,IntPtr.Zero,IntPtr.Zero,0x185);
    Rect r; if(!GetWindowRect(h,out r)) throw new Exception("window rectangle");
    using(var bitmap=new Bitmap(r.R-r.L,r.B-r.T)) {
      using(var graphics=Graphics.FromImage(bitmap)) {
        var dc=graphics.GetHdc();
        try { if(!PrintWindow(h,dc,2)) throw new Exception("window capture"); }
        finally { graphics.ReleaseHdc(dc); }
      }
      bitmap.Save(path,System.Drawing.Imaging.ImageFormat.Png);
    }
  }
}
'@
$installTarget = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-ui-offline-' + [guid]::NewGuid().ToString('N'))
$profilePath = Join-Path $env:LOCALAPPDATA 'AutoKeyboardLayot'
$fencePath = Join-Path $env:LOCALAPPDATA 'AutoKeyboardLayot.installation.lock'
$profileExisted = Test-Path -LiteralPath $profilePath
$fenceExisted = Test-Path -LiteralPath $fencePath
$info = New-Object Diagnostics.ProcessStartInfo
$info.FileName = $Executable
$info.Arguments = '/SP- /LANG=english /DIR="' + $installTarget + '" /LOG="' + (Join-Path $OutputDirectory 'inno.log') + '"'
$info.UseShellExecute = $false
$process = [Diagnostics.Process]::Start($info)
$script:taskProcesses = @{$process.Id=$process}
function Refresh-OwnedProcesses {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
    for ($pass=0; $pass -lt 3; $pass++) {
        foreach ($item in $all) {
            if ($script:taskProcesses.ContainsKey([int]$item.ParentProcessId) -and -not $script:taskProcesses.ContainsKey([int]$item.ProcessId)) {
                try { $script:taskProcesses[[int]$item.ProcessId] = [Diagnostics.Process]::GetProcessById([int]$item.ProcessId) } catch {}
            }
        }
    }
}
function Own-Window([IntPtr]$Handle) {
    $ownerId = [int][ProbeWindows]::Owner($Handle)
    return $script:taskProcesses.ContainsKey($ownerId) -and -not $script:taskProcesses[$ownerId].HasExited
}
function Click-Owned([IntPtr]$Handle) {
    if (-not (Own-Window $Handle)) { throw 'Invalid control ownership.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(1)
    while (-not [ProbeWindows]::IsWindowEnabled($Handle) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 20 }
    if (-not [ProbeWindows]::IsWindowEnabled($Handle)) { throw ('Disabled owned control: ' + [ProbeWindows]::Text($Handle)) }
    if (-not [ProbeWindows]::PostMessage($Handle, 0xF5, [IntPtr]::Zero, [IntPtr]::Zero)) { throw 'Button message failed.' }
}
function Find-Dialog {
    foreach ($window in [ProbeWindows]::Windows()) {
        if ((Own-Window $window) -and [ProbeWindows]::IsWindowVisible($window) -and [ProbeWindows]::Class($window) -eq '#32770') { return $window }
    }
    return [IntPtr]::Zero
}
function Wait-Dialog {
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    do { $dialog = Find-Dialog; if ($dialog -ne [IntPtr]::Zero) { return $dialog }; Start-Sleep -Milliseconds 50 } while ([DateTime]::UtcNow -lt $deadline)
    throw 'Owned dialog timeout.'
}
function Owned-Control([string]$Class, [string]$Caption) {
    foreach ($window in [ProbeWindows]::Windows()) {
        if (-not (Own-Window $window) -or -not [ProbeWindows]::IsWindowVisible($window)) { continue }
        foreach ($control in [ProbeWindows]::Children($window)) {
            if (-not [ProbeWindows]::IsWindowVisible($control)) { continue }
            if ($Class -and [ProbeWindows]::Class($control) -ne $Class) { continue }
            if ($Caption -and [ProbeWindows]::Text($control).Replace('&','') -notlike $Caption) { continue }
            return $control
        }
    }
    return [IntPtr]::Zero
}
function Wait-Control([string]$Class, [string]$Caption, [int]$Seconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do { $control = Owned-Control $Class $Caption; if ($control -ne [IntPtr]::Zero) { return $control }; Start-Sleep -Milliseconds 100 } while ([DateTime]::UtcNow -lt $deadline)
    throw ('Control timeout: ' + $Caption)
}
function Owned-Button([string]$Caption) {
    foreach ($window in [ProbeWindows]::Windows()) {
        if (-not (Own-Window $window) -or -not [ProbeWindows]::IsWindowVisible($window)) { continue }
        foreach ($control in [ProbeWindows]::Children($window)) {
            if (-not [ProbeWindows]::IsWindowVisible($control)) { continue }
            if ([ProbeWindows]::Class($control) -notlike '*Button*') { continue }
            if ($Caption -and [ProbeWindows]::Text($control).Replace('&','') -notlike $Caption) { continue }
            return $control
        }
    }
    return [IntPtr]::Zero
}
function Wait-Button([string]$Caption, [int]$Seconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do { $control = Owned-Button $Caption; if ($control -ne [IntPtr]::Zero) { return $control }; Start-Sleep -Milliseconds 100 } while ([DateTime]::UtcNow -lt $deadline)
    throw ('Button timeout: ' + $Caption)
}
function Answer-Yes {    $dialog = Wait-Dialog
    $yes = @([ProbeWindows]::Children($dialog) | Where-Object { [ProbeWindows]::IsWindowVisible($_) -and [ProbeWindows]::Text($_).Replace('&','') -eq 'Yes' })
    if ($yes.Count -ne 1) { throw 'Confirmation Yes button not unique.' }
    Click-Owned $yes[0]
}
function Snapshot-Windows {
    $out = @()
    foreach ($window in [ProbeWindows]::Windows()) {
        if (-not (Own-Window $window) -or -not [ProbeWindows]::IsWindowVisible($window)) { continue }
        $texts = @([ProbeWindows]::Children($window) | Where-Object { [ProbeWindows]::IsWindowVisible($_) } | ForEach-Object { [ProbeWindows]::Text($_) } | Where-Object { $_ })
        $out += @{window=[ProbeWindows]::Text($window); class=[ProbeWindows]::Class($window); controls=$texts}
    }
    return $out
}
$receipt = @{state='failed'; executable_sha256=$ExpectedSha256; network_used=$false; installation_target=$installTarget}
try {
    # Reach the package page.
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    $local = [IntPtr]::Zero
    while ([DateTime]::UtcNow -lt $deadline -and $local -eq [IntPtr]::Zero) {
        Refresh-OwnedProcesses
        $local = Owned-Control $null 'Open local catalog*'
        if ($local -eq [IntPtr]::Zero) {
            $next = Owned-Control $null 'Next*'
            if ($next -ne [IntPtr]::Zero) { Click-Owned $next; Start-Sleep -Milliseconds 200 }
        }
        if ($local -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100 }
    }
    if ($local -eq [IntPtr]::Zero) { throw 'Package page was not reached.' }
    $wizard = [IntPtr]::Zero
    foreach ($window in [ProbeWindows]::Windows()) {
        if ((Own-Window $window) -and [ProbeWindows]::IsWindowVisible($window) -and [ProbeWindows]::Text($window) -like '*Setup*') { $wizard = $window; break }
    }
    if ($wizard -eq [IntPtr]::Zero) { throw 'Wizard window not identified.' }
    $script:taskWizard = $wizard
    [ProbeWindows]::Capture($wizard, (Join-Path $OutputDirectory 'package-page.png'))
    $download = Wait-Button 'Download selected packages'
    $receipt.download_disabled_without_selection = -not [ProbeWindows]::IsWindowEnabled($download)
    if (-not $receipt.download_disabled_without_selection) { throw 'Download enabled without a selection.' }
    # Open the local catalog.
    Click-Owned $local
    $dialog = Wait-Dialog
    $edit = [ProbeWindows]::GetDlgItem($dialog, 1148)
    if ($edit -ne [IntPtr]::Zero -and [ProbeWindows]::Class($edit) -ne 'Edit') {
        $edits = @([ProbeWindows]::Children($edit) | Where-Object { [ProbeWindows]::Class($_) -eq 'Edit' })
        if ($edits.Count -eq 1) { $edit = $edits[0] }
    }
    if ($edit -eq [IntPtr]::Zero -or [ProbeWindows]::Class($edit) -ne 'Edit') { $edit = [ProbeWindows]::GetDlgItem($dialog, 1152) }
    if ($edit -eq [IntPtr]::Zero -or -not (Own-Window $edit)) { throw 'File-name control unavailable.' }
    [void][ProbeWindows]::SetText($edit, 0xC, [IntPtr]::Zero, $Catalog)
    Click-Owned ([ProbeWindows]::GetDlgItem($dialog, 1))
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    $list = [IntPtr]::Zero
    do {
        $lists = @([ProbeWindows]::Children($wizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) -and [ProbeWindows]::Class($_) -eq 'TNewCheckListBox' })
        if ($lists.Count -eq 1) { $list = $lists[0] }
        if ($list -ne [IntPtr]::Zero -and [ProbeWindows]::Message($list,0x18B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq 13) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($list -eq [IntPtr]::Zero -or [ProbeWindows]::Message($list,0x18B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -ne 13) { throw 'Thirteen catalog rows were not displayed.' }
    # Explicit one-package selection so the review page marks it seen.
    $row = -1
    for ($index=0; $index -lt 13; $index++) { if ([ProbeWindows]::Row($list,$index) -like 'ru-ru *') { $row=$index; break } }
    if ($row -lt 0) { throw 'Russian package row missing.' }
    [void][ProbeWindows]::Message($list,0x186,[IntPtr]$row,[IntPtr]::Zero)
    [void][ProbeWindows]::PostMessage($list,0x100,[IntPtr]32,[IntPtr]::Zero)
    [void][ProbeWindows]::PostMessage($list,0x101,[IntPtr]32,[IntPtr]::Zero)
    Start-Sleep -Milliseconds 200
    if (-not [ProbeWindows]::IsWindowEnabled($download)) { throw 'Explicit selection did not enable download.' }
    [ProbeWindows]::Capture($wizard, (Join-Path $OutputDirectory 'catalog-selection.png'))
    # Download through the real helper (local artifact, no network).
    Click-Owned $download
    $confirmation = Wait-Dialog
    $confirmationText = @([ProbeWindows]::Children($confirmation) | ForEach-Object { [ProbeWindows]::Text($_) }) -join ' '
    if ($confirmationText -notlike '*Selected: 1 packages*') { throw 'Selected download confirmation mismatch.' }
    Click-Owned ([ProbeWindows]::GetDlgItem($confirmation, 6)) # IDYES
    # Wait for the download to be reviewed, then advance to the review page.
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    $reviewed = $false
    do {
        Refresh-OwnedProcesses
        $texts = @([ProbeWindows]::Children($wizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) } | ForEach-Object { [ProbeWindows]::Text($_) })
        if (($texts -join ' ') -like '*Verified:*') { $reviewed = $true; break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $reviewed) { throw 'Local download did not reach the reviewing phase.' }
    [ProbeWindows]::Capture($wizard, (Join-Path $OutputDirectory 'download-complete.png'))
    Click-Owned (Wait-Control $null 'Next*')
    $install = Wait-Button 'Install selected packages' 30
    if ($install -eq [IntPtr]::Zero) { throw 'Review page was not reached.' }
    [ProbeWindows]::Capture($wizard, (Join-Path $OutputDirectory 'review-page.png'))
    $memos = @([ProbeWindows]::Children($wizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) })
    $licenseText = ($memos | ForEach-Object { [ProbeWindows]::GetText($_, 131072) }) -join ' '
    $combos = @([ProbeWindows]::Children($wizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) -and [ProbeWindows]::Class($_) -in @('TNewComboBox','TComboBox') })
    $receipt.review_combo_count = if ($combos.Count -ge 1) { [ProbeWindows]::Message($combos[0],0x146,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() } else { -1 }
    $receipt.review_text_length = $licenseText.Length
    $receipt.review_has_package = $licenseText -like '*ru-RU*'
    $receipt.review_has_license = $licenseText -like '*MIT*' -or $licenseText -like '*Permission is hereby granted*'
    if (-not ($receipt.review_has_package -and $receipt.review_has_license)) { throw 'Review text did not include the package identity and license.' }
    # Install the reviewed package and confirm.
    Click-Owned $install
    Answer-Yes
    # Wait for the completion status, then finish the wizard.
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    $complete = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        Refresh-OwnedProcesses
        $text = (@([ProbeWindows]::Children($wizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) } | ForEach-Object { [ProbeWindows]::GetText($_, 131072) }) -join ' ')
        if ($text -like '*Packages installed*') { $complete = $true; break }
        Start-Sleep -Milliseconds 200
    }
    $receipt.install_completed_status = $complete
    if (-not $complete) { throw 'Package installation did not reach the completed status.' }
    [ProbeWindows]::Capture($wizard, (Join-Path $OutputDirectory 'installed.png'))
    # Advance through the install step and finish.
    $deadline = [DateTime]::UtcNow.AddSeconds(150)
    $finished = $false
    do {
        Refresh-OwnedProcesses
        $finish = Owned-Control $null 'Finish*'
        if ($finish -ne [IntPtr]::Zero -and [ProbeWindows]::IsWindowVisible($finish) -and [ProbeWindows]::IsWindowEnabled($finish)) { Click-Owned $finish; $finished = $true; break }
        $readyInstall = Owned-Button 'Install'
        if ($readyInstall -ne [IntPtr]::Zero -and [ProbeWindows]::IsWindowVisible($readyInstall) -and [ProbeWindows]::IsWindowEnabled($readyInstall)) { Click-Owned $readyInstall }
        else {
            $next = Owned-Control $null 'Next*'
            if ($next -ne [IntPtr]::Zero -and [ProbeWindows]::IsWindowEnabled($next)) { Click-Owned $next }
        }
        Start-Sleep -Milliseconds 300
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $finished) { throw 'Wizard Finish was not reached.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) { Refresh-OwnedProcesses; Start-Sleep -Milliseconds 200 }
    if (-not $process.HasExited) { throw 'Setup did not close after Finish.' }
    $receipt.normal_close = $true
    $receipt.install_target_present = (Test-Path -LiteralPath (Join-Path $installTarget 'AutoKeyboardLayot.exe'))
    if (-not $receipt.install_target_present) { throw 'Installed executable missing after Finish.' }
    $receipt.installed_app_hash = ((Get-FileHash -LiteralPath (Join-Path $installTarget 'AutoKeyboardLayot.exe') -Algorithm SHA256).Hash -eq $InstalledAppHash)
    if (-not $receipt.installed_app_hash) { throw 'Installed executable hash mismatch.' }
    # Restore the VM baseline: uninstall what this test installed, preserving the
    # profile/store by design, then remove only artifacts this test created.
    $registration = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{D913907B-2031-4C26-A199-BF60B0E51B6D}_is1'
    $receipt.profile_existed_before = $profileExisted
    if (Test-Path -LiteralPath $registration) {
        $value = (Get-ItemProperty -LiteralPath $registration -Name UninstallString).UninstallString
        $uninstaller = $value.Trim('"')
        if (-not $uninstaller.EndsWith('.exe',[StringComparison]::OrdinalIgnoreCase)) {
            $m = [regex]::Match($value,'(?i)("[^"]+\.exe"|\S+\.exe)'); if ($m.Success) { $uninstaller = $m.Value.Trim('"') }
        }
        $ui = New-Object Diagnostics.ProcessStartInfo
        $ui.FileName = $uninstaller; $ui.Arguments = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART'
        $ui.UseShellExecute = $false; $ui.CreateNoWindow = $true
        $up = [Diagnostics.Process]::Start($ui)
        $deadline = [DateTime]::UtcNow.AddSeconds(120)
        while (-not $up.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
        $receipt.uninstall_exit = if ($up.HasExited) { $up.ExitCode } else { $up.Kill(); -1 }
        Start-Sleep -Milliseconds 500
    }
    $receipt.registration_removed = -not (Test-Path -LiteralPath $registration)
    $receipt.app_removed = -not (Test-Path -LiteralPath (Join-Path $installTarget 'AutoKeyboardLayot.exe'))
    $receipt.config_preserved = (Test-Path -LiteralPath (Join-Path $profilePath 'config.ini'))
    $receipt.store_preserved = (Test-Path -LiteralPath (Join-Path $profilePath 'packages\CURRENT'))
    if (-not ($receipt.registration_removed -and $receipt.app_removed -and $receipt.config_preserved -and $receipt.store_preserved)) { throw 'Post-install uninstall did not restore a clean baseline with preserved profile/store.' }
    if (-not $profileExisted -and (Test-Path -LiteralPath $profilePath)) { Remove-Item -LiteralPath $profilePath -Recurse -Force }
    if (-not $fenceExisted -and (Test-Path -LiteralPath $fencePath)) { Remove-Item -LiteralPath $fencePath -Force }
    Remove-Item -LiteralPath $installTarget -Recurse -Force -ErrorAction SilentlyContinue
    $result = @{state='passed'; executable_sha256=$ExpectedSha256; network_used=$false; typing_acceptance=$false
        installation_target=$installTarget; checks=@('download_disabled_without_selection','local_catalog_13_rows','explicit_selection','real_local_download','review_license','install_confirmed','installed_app_hash','normal_close','uninstall_restored','profile_store_preserved')}
    $result.download_disabled_without_selection = $receipt.download_disabled_without_selection
    $result.review_has_package = $receipt.review_has_package
    $result.review_has_license = $receipt.review_has_license
    $result.installed_app_hash = $receipt.installed_app_hash
    $result.profile_existed_before = $profileExisted
    $result.uninstall_exit = $receipt.uninstall_exit
    $stream = [IO.File]::Open((Join-Path $OutputDirectory 'result.json'), [IO.FileMode]::CreateNew)
    try { $b = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 5)); $stream.Write($b,0,$b.Length); $stream.Flush($true) } finally { $stream.Dispose() }
    Write-Output 'NATIVE_INSTALLER_UI_OFFLINE_PASSED'
} catch {
    $receipt.error = $_.Exception.Message
    $receipt.test_stack = $_.ScriptStackTrace
    try { $receipt.snapshot = Snapshot-Windows } catch {}
    Write-Output $receipt.error
    try {
        $first = [ProbeWindows]::Windows() | Where-Object { (Own-Window $_) -and [ProbeWindows]::IsWindowVisible($_) } | Select-Object -First 1
        if ($first) { [ProbeWindows]::Capture($first, (Join-Path $OutputDirectory 'failure-window.png')) }
    } catch {}
    $receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'result.json') -Encoding UTF8
} finally {
    foreach ($owned in $script:taskProcesses.Values) { if (-not $owned.HasExited) { $owned.Kill(); $script:fixtureStopped=$true } }
}
if (-not (Test-Path -LiteralPath (Join-Path $OutputDirectory 'result.json'))) { throw 'Missing receipt.' }
$final = Get-Content -LiteralPath (Join-Path $OutputDirectory 'result.json') -Raw | ConvertFrom-Json
if ($final.state -ne 'passed') { exit 1 }
exit 0
