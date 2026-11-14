param(
    [Parameter(Mandatory=$true)][string]$Script,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
# Acceptance of deploy-host.ps1 on a Windows machine without touching its real installation: every
# scenario has a LOCALAPPDATA of its own, and the agent is a stand-in compiled here with the observer
# window and graceful-close property of the real one (close-delay.txt beside it delays its close).
#   success           the agent closes, the candidate is installed and started, the backup is kept
#   copy-restored     the installed file is held for six seconds: copying the candidate fails, the
#                     restore waits for the file, and the previous agent runs again
#   restore-fails     the file is held for forty seconds: the error names the original failure and the
#                     backup, and no agent is started from a half-restored file
#   slow-close        the agent closes twenty seconds after WM_CLOSE: it runs again from the unchanged
#                     executable and nothing is replaced
#   confirm           -Confirm answered Yes, then No to anything else: the one question is asked once and
#                     the deployment completes with its backup; answered No, nothing changes
# -Script is the deploy-host.ps1 under test (the current one or an older copy). No other
# AutoKeyboardLayot process may run on the machine.
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new output directory.' }
if (@(Get-Process -Name AutoKeyboardLayot -ErrorAction SilentlyContinue).Count -gt 0) {
    throw 'An AutoKeyboardLayot process runs on this machine; stop it first.'
}
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$work = Join-Path $OutputDirectory 'work'
$powershell = Join-Path $PSHOME 'powershell.exe'
$result = [ordered]@{state='failed'; script=$Script; scenarios=[ordered]@{}}

$agentSource = @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
public static class AGENT_CLASS {
    public const string Build = "AGENT_BUILD";
    delegate IntPtr WndProc(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp);
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    struct WNDCLASS { public uint style; public WndProc proc; public int clsExtra, wndExtra; public IntPtr instance, icon, cursor, background; public string menu, name; }
    [StructLayout(LayoutKind.Sequential)]
    struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam, lParam; public uint time; public int x, y; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern ushort RegisterClass(ref WNDCLASS c);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr CreateWindowEx(uint ex, string cls, string title, uint style, int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr param);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool SetProp(IntPtr hwnd, string name, IntPtr data);
    [DllImport("user32.dll")] static extern IntPtr DefWindowProc(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp);
    [DllImport("user32.dll")] static extern bool DestroyWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern void PostQuitMessage(int code);
    [DllImport("user32.dll")] static extern int GetMessage(out MSG msg, IntPtr hwnd, uint first, uint last);
    [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG msg);
    [DllImport("user32.dll")] static extern IntPtr DispatchMessage(ref MSG msg);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] static extern IntPtr GetModuleHandle(string name);
    static readonly WndProc procedure = Window;
    static int closeDelay;
    static IntPtr Window(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp) {
        if (msg == 0x10) { Thread.Sleep(closeDelay * 1000); DestroyWindow(hwnd); return IntPtr.Zero; }
        if (msg == 0x2) { PostQuitMessage(0); return IntPtr.Zero; }
        return DefWindowProc(hwnd, msg, wp, lp);
    }
    public static int Main(string[] args) {
        if (args.Length > 0 && args[0] == "--verify-profile") return 0;
        string delay = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "close-delay.txt");
        if (File.Exists(delay)) closeDelay = int.Parse(File.ReadAllText(delay).Trim());
        var windowClass = new WNDCLASS { proc = procedure, instance = GetModuleHandle(null), name = "AutoKeyboardLayot.ObserverWindow" };
        RegisterClass(ref windowClass);
        IntPtr window = CreateWindowEx(0, windowClass.name, "", 0, 0, 0, 0, 0, IntPtr.Zero, IntPtr.Zero, windowClass.instance, IntPtr.Zero);
        SetProp(window, "AutoKeyboardLayot.GracefulClose.v1", new IntPtr(1));
        MSG msg;
        while (GetMessage(out msg, IntPtr.Zero, 0, 0) > 0) { TranslateMessage(ref msg); DispatchMessage(ref msg); }
        return 0;
    }
}
'@
foreach ($build in 'previous', 'candidate') {
    $directory = Join-Path $work $build
    New-Item -ItemType Directory -Path $directory | Out-Null
    Add-Type -OutputType WindowsApplication -OutputAssembly (Join-Path $directory 'AutoKeyboardLayot.exe') `
        -TypeDefinition ($agentSource.Replace('AGENT_CLASS', 'Agent_' + $build).Replace('AGENT_BUILD', $build))
}
$previous = Join-Path $work 'previous\AutoKeyboardLayot.exe'
$candidate = Join-Path $work 'candidate\AutoKeyboardLayot.exe'
function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
$previousHash = Get-Sha256 $previous
$candidateHash = Get-Sha256 $candidate

function New-Profile([string]$Name) {
    $root = Join-Path $work $Name
    $install = Join-Path $root 'Programs\AutoKeyboardLayot'
    New-Item -ItemType Directory -Path $install | Out-Null
    Copy-Item -LiteralPath $previous -Destination (Join-Path $install 'AutoKeyboardLayot.exe')
    return $root
}
function Installed([string]$Root) { Join-Path $Root 'Programs\AutoKeyboardLayot\AutoKeyboardLayot.exe' }
function Agents { @(Get-Process -Name AutoKeyboardLayot -ErrorAction SilentlyContinue) }
function Stop-Agents { foreach ($process in Agents) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue; $process.WaitForExit(10000) | Out-Null } }
function Start-Agent([string]$Root) {
    $process = Start-Process -FilePath (Installed $Root) -PassThru
    Start-Sleep -Seconds 2
    return $process
}
# Holds the installed executable open for reading only (no writing or deleting) for $Seconds.
function Hold-File([string]$Path, [int]$Seconds) {
    $file = Join-Path $work ('hold-' + [guid]::NewGuid().ToString('N') + '.ps1')
    $ready = $file + '.ready'
    Set-Content -LiteralPath $file -Encoding UTF8 -Value @"
`$stream = [IO.File]::Open('$Path', 'Open', 'Read', 'Read')
New-Item -ItemType File -Path '$ready' | Out-Null
Start-Sleep -Seconds $Seconds
`$stream.Dispose()
"@
    $holder = Start-Process -FilePath $powershell -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $file) -PassThru -WindowStyle Hidden
    while (-not (Test-Path -LiteralPath $ready)) { Start-Sleep -Milliseconds 50 }
    return $holder
}
function Run-Deploy([string]$Root, [string[]]$Extra, [string]$Answers) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $powershell
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$Script`"", '-Source', "`"$candidate`"",
        '-ExpectedSha256', $candidateHash, '-BackupSuffix', 'acceptance') + $Extra
    $info.Arguments = $arguments -join ' '
    $info.EnvironmentVariables['LOCALAPPDATA'] = $Root
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($info)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if ($Answers) { $process.StandardInput.Write($Answers) }
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(240000)) { $process.Kill() }
    $process.WaitForExit()
    return @{exit=$process.ExitCode; output=($stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult())}
}
function Record([string]$Name, $Run, [string]$Root, [hashtable]$Checks) {
    $backup = (Installed $Root) + '.before-acceptance'
    $entry = [ordered]@{exit=$Run.exit; installed=(Get-Sha256 (Installed $Root)); backup=(Test-Path -LiteralPath $backup);
        agents=@(Agents | ForEach-Object { $_.Path }); checks=[ordered]@{}}
    foreach ($check in $Checks.GetEnumerator()) { $entry.checks[$check.Key] = [bool]$check.Value }
    $entry.passed = @($entry.checks.Values | Where-Object { -not $_ }).Count -eq 0
    $entry.output = $Run.output
    $result.scenarios[$Name] = $entry
}

try {
    # success
    $root = New-Profile 'success'
    $null = Start-Agent $root
    $run = Run-Deploy $root @() ''
    Start-Sleep -Seconds 1
    $agents = @(Agents)
    Record 'success' $run $root @{
        exit_0 = ($run.exit -eq 0); deployed = ($run.output -like '*DEPLOYED_PID=*');
        candidate_installed = ((Get-Sha256 (Installed $root)) -eq $candidateHash);
        backup_is_previous = ((Get-Sha256 ((Installed $root) + '.before-acceptance')) -eq $previousHash);
        one_agent_from_the_installation = ($agents.Count -eq 1 -and $agents[0].Path -eq (Installed $root))}
    Stop-Agents

    # copy-restored and restore-fails
    foreach ($case in @(@{name='copy-restored'; hold=6}, @{name='restore-fails'; hold=40})) {
        $root = New-Profile $case.name
        $null = Start-Agent $root
        $holder = Hold-File (Installed $root) $case.hold
        $run = Run-Deploy $root @() ''
        $agentsDuringHold = @(Agents)
        $holder.WaitForExit()
        if ($case.name -eq 'copy-restored') {
            $checks = @{
                failed = ($run.exit -ne 0); rolled_back = ($run.output -like '*Deployment failed and was rolled back*');
                previous_installed = ((Get-Sha256 (Installed $root)) -eq $previousHash);
                previous_agent_runs = ($agentsDuringHold.Count -eq 1 -and $agentsDuringHold[0].Path -eq (Installed $root))}
        } else {
            # PowerShell wraps a long error message, so it is compared without its white space.
            $backup = ((Installed $root) + '.before-acceptance') -replace '\s', ''
            $message = $run.output -replace '\s', ''
            $checks = @{
                failed = ($run.exit -ne 0); names_the_restore_failure = ($message -like '*restoringthepreviousexecutablefailed*');
                names_the_original_failure = ($message -like '*Deploymentfailed(*');
                names_the_backup = ($message -like ('*Copy' + $backup + 'over*'));
                no_agent_started = ($agentsDuringHold.Count -eq 0);
                previous_still_installed = ((Get-Sha256 (Installed $root)) -eq $previousHash)}
        }
        Record $case.name $run $root $checks
        Stop-Agents
    }

    # slow-close
    $root = New-Profile 'slow-close'
    Set-Content -LiteralPath (Join-Path (Split-Path (Installed $root)) 'close-delay.txt') -Value '20'
    $null = Start-Agent $root
    $started = [DateTime]::UtcNow
    $run = Run-Deploy $root @() ''
    while (([DateTime]::UtcNow - $started).TotalSeconds -lt 26) { Start-Sleep -Milliseconds 500 }
    $agents = @(Agents)
    Record 'slow-close' $run $root @{
        failed = ($run.exit -ne 0); says_started_again = ($run.output -like '*started again from the unchanged executable*');
        previous_installed = ((Get-Sha256 (Installed $root)) -eq $previousHash);
        no_backup = (-not (Test-Path -LiteralPath ((Installed $root) + '.before-acceptance')));
        agent_runs_again = ($agents.Count -eq 1 -and $agents[0].Path -eq (Installed $root))}
    Stop-Agents

    # confirm
    $root = New-Profile 'confirm-yes'
    $null = Start-Agent $root
    $run = Run-Deploy $root @('-Confirm') ("Y`r`n" + ("N`r`n" * 20))
    Start-Sleep -Seconds 1
    Record 'confirm-yes' $run $root @{
        exit_0 = ($run.exit -eq 0); deployed = ($run.output -like '*DEPLOYED_PID=*');
        candidate_installed = ((Get-Sha256 (Installed $root)) -eq $candidateHash);
        backup_is_previous = ((Test-Path -LiteralPath ((Installed $root) + '.before-acceptance')) -and
            (Get-Sha256 ((Installed $root) + '.before-acceptance')) -eq $previousHash)}
    Stop-Agents
    $root = New-Profile 'confirm-no'
    $null = Start-Agent $root
    $run = Run-Deploy $root @('-Confirm') ("N`r`n" * 20)
    $agents = @(Agents)
    Record 'confirm-no' $run $root @{
        previous_installed = ((Get-Sha256 (Installed $root)) -eq $previousHash);
        no_backup = (-not (Test-Path -LiteralPath ((Installed $root) + '.before-acceptance')));
        agent_untouched = ($agents.Count -eq 1)}
    Stop-Agents

    $failed = @($result.scenarios.Values | Where-Object { -not $_.passed })
    $result.state = if ($failed.Count -eq 0) { 'passed' } else { 'failed' }
} catch {
    $result.error = $_.Exception.Message
    $result.line = $_.InvocationInfo.ScriptLineNumber
} finally {
    Stop-Agents
    $result | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'result.json') -Encoding UTF8
}
$summary = [ordered]@{state=$result.state}
foreach ($scenario in $result.scenarios.GetEnumerator()) { $summary[$scenario.Key] = $scenario.Value.passed }
$summary | ConvertTo-Json -Compress
if ($result.state -ne 'passed') { exit 1 }
