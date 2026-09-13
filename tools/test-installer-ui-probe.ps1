param(
    [string]$Catalog,
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ExpectedSha256,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
if ($Executable -notlike '*ui-probe-NOT-FOR-INSTALLATION.exe' -or $ExpectedSha256 -notmatch '^[a-f0-9]{64}$' -or
    (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash -ne $ExpectedSha256) { throw 'Expected the exact installation-disabled probe.' }
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
$info = New-Object Diagnostics.ProcessStartInfo
$info.FileName = $Executable
$probeInstallTarget = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-probe-forbidden-' + [guid]::NewGuid().ToString('N'))
$info.Arguments = '/SP- /LANG=english /DIR="' + $probeInstallTarget + '" /LOG="' + (Join-Path $OutputDirectory 'inno.log') + '"'
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
    do {
        $dialog = Find-Dialog
        if ($dialog -ne [IntPtr]::Zero) { return $dialog }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'Owned dialog timeout.'
}
$receipt = @{state='failed'; executable_sha256=$ExpectedSha256; installation_attempted=$false; network_used=$false; controls=@()}
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    $found = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        Refresh-OwnedProcesses
        foreach ($window in [ProbeWindows]::Windows()) {
            if (-not (Own-Window $window) -or -not [ProbeWindows]::IsWindowVisible($window)) { continue }
            $controls = @([ProbeWindows]::Children($window) | Where-Object { [ProbeWindows]::IsWindowVisible($_) })
            $receipt.last_window = [ProbeWindows]::Text($window)
            $receipt.last_controls = @($controls | ForEach-Object { [ProbeWindows]::Text($_) } | Where-Object { $_ })
            $script:taskLastWindow = $window
            $local = @($controls | Where-Object { [ProbeWindows]::Text($_).Replace('&','') -like 'Open local catalog*' })
            if ($local.Count -gt 0) {
                $receipt.controls = @($controls | ForEach-Object { [ProbeWindows]::Text($_) } | Where-Object { $_ })
                [ProbeWindows]::Capture($window, (Join-Path $OutputDirectory 'package-page.png'))
                $found = $true
                $script:taskWizard = $window
                break
            }
            $next = @($controls | Where-Object { [ProbeWindows]::Text($_).Replace('&','') -match '^Next( >)?$' })
            if ($next.Count -eq 1) { Click-Owned $next[0]; Start-Sleep -Milliseconds 200 }
        }
        if ($found) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $found) { throw 'Package page was not reached.' }
    if (-not ($receipt.controls -contains 'Check package catalog') -or -not ($receipt.controls -contains 'Download selected packages')) { throw 'Missing package controls.' }
    if ($Catalog) {
        Click-Owned $local[0]
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
            $lists = @([ProbeWindows]::Children($script:taskWizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) -and [ProbeWindows]::Class($_) -eq 'TNewCheckListBox' })
            if ($lists.Count -eq 1) { $list = $lists[0] }
            if ($list -ne [IntPtr]::Zero -and [ProbeWindows]::Message($list,0x18B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq 13) { break }
            Start-Sleep -Milliseconds 100
        } while ([DateTime]::UtcNow -lt $deadline)
        if ($list -eq [IntPtr]::Zero -or [ProbeWindows]::Message($list,0x18B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -ne 13) { throw 'Thirteen catalog rows were not displayed.' }
        $row = -1
        for ($index=0; $index -lt 13; $index++) { if ([ProbeWindows]::Row($list,$index) -like 'ru-ru *') { $row=$index; break } }
        if ($row -lt 0) { throw 'Russian package row missing.' }
        [void][ProbeWindows]::Message($list,0x186,[IntPtr]$row,[IntPtr]::Zero)
        [void][ProbeWindows]::PostMessage($list,0x100,[IntPtr]32,[IntPtr]::Zero)
        [void][ProbeWindows]::PostMessage($list,0x101,[IntPtr]32,[IntPtr]::Zero)
        Start-Sleep -Milliseconds 200
        $download = @([ProbeWindows]::Children($script:taskWizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) -and [ProbeWindows]::Text($_) -eq 'Download selected packages' })
        if ($download.Count -ne 1 -or -not [ProbeWindows]::IsWindowEnabled($download[0])) { throw 'Explicit checkbox selection did not enable download.' }
        [ProbeWindows]::Capture($script:taskWizard, (Join-Path $OutputDirectory 'catalog-selection.png'))
        Click-Owned $download[0]
        $confirmation = Wait-Dialog
        $confirmationText = @([ProbeWindows]::Children($confirmation) | ForEach-Object { [ProbeWindows]::Text($_) }) -join ' '
        if ($confirmationText -notlike '*Selected: 1 packages*') { throw 'Selected download confirmation mismatch.' }
        Click-Owned ([ProbeWindows]::GetDlgItem($confirmation, 6)) # IDYES
        Start-Sleep -Milliseconds 200
        $blocked = Wait-Dialog
        $blockedText = @([ProbeWindows]::Children($blocked) | ForEach-Object { [ProbeWindows]::Text($_) }) -join ' '
        if ($blockedText -notlike '*Finish or cancel package selection*') { throw 'UI probe did not block preparation.' }
        $okButtons = @([ProbeWindows]::Children($blocked) | Where-Object { [ProbeWindows]::IsWindowVisible($_) -and [ProbeWindows]::Text($_).Replace('&','') -eq 'OK' })
        if ($okButtons.Count -ne 1) { throw 'Probe refusal OK button not unique.' }
        Click-Owned $okButtons[0]
        Start-Sleep -Milliseconds 200
        $log = Get-Content -LiteralPath (Join-Path $OutputDirectory 'inno.log')
        $tempLine = @($log | Where-Object { $_ -match 'Created temporary directory: ' })[-1]
        $tempDirectory = ($tempLine -split 'Created temporary directory: ',2)[1]
        if (-not $tempDirectory -or (Test-Path -LiteralPath (Join-Path $tempDirectory 'probe-profile\packages'))) { throw 'Probe profile was created or its path was not verified.' }
        $commands = @(Get-ChildItem -LiteralPath (Join-Path $tempDirectory 'package-session-1') -Filter 'request-*.json' | ForEach-Object { (Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json).command.action })
        if ($commands -contains 'confirm_download' -or $commands -contains 'confirm_install') { throw 'Probe sent a mutating command.' }
        $receipt.catalog_rows = 13
        $receipt.explicit_selection = 'ru-RU'
        $receipt.probe_preparation_refused = $true
        $receipt.helper_commands = $commands
        $receipt.real_store_absent = $true
    }
    $cancel = @([ProbeWindows]::Children($script:taskWizard) | Where-Object { [ProbeWindows]::IsWindowVisible($_) -and [ProbeWindows]::Text($_).Replace('&','') -eq 'Cancel' })
    if ($cancel.Count -ne 1) { throw 'Cancel control not unique.' }
    Click-Owned $cancel[0]
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        foreach ($window in [ProbeWindows]::Windows()) {
            if (-not (Own-Window $window) -or -not [ProbeWindows]::IsWindowVisible($window)) { continue }
            foreach ($control in [ProbeWindows]::Children($window)) {
                if ([ProbeWindows]::IsWindowVisible($control) -and [ProbeWindows]::Text($control).Replace('&','') -eq 'Yes') { Click-Owned $control }
            }
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $process.HasExited) { throw 'Probe did not close normally.' }
    if (Test-Path -LiteralPath $probeInstallTarget) { throw 'Probe created installation target.' }
    $receipt.state = 'passed'
    $receipt.normal_close = $true
} catch {
    $receipt.error = $_.Exception.Message
    $receipt.test_stack = $_.ScriptStackTrace
    Write-Output $receipt.error
    if ($script:taskLastWindow -and (Own-Window $script:taskLastWindow)) {
        try { [ProbeWindows]::Capture($script:taskLastWindow, (Join-Path $OutputDirectory 'failure-window.png')) } catch {}
    }
} finally {
    foreach ($ownedProcess in $script:taskProcesses.Values) {
        if (-not $ownedProcess.HasExited) { $ownedProcess.Kill(); $receipt.fixture_process_stopped_on_failure=$true }
    }
    $receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'result.json') -Encoding UTF8
}
if ($receipt.state -ne 'passed') { exit 1 }
Write-Output 'INSTALLER_UI_PROBE_NAVIGATION_PASSED'
