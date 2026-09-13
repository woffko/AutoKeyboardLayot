param(
    [Parameter(Mandatory=$true)][string]$Helper,
    [Parameter(Mandatory=$true)][string]$HelperSha256,
    [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
if ($HelperSha256 -notmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath $Helper -Algorithm SHA256).Hash -ne $HelperSha256) { throw 'Helper hash mismatch.' }
function Start-Native([string]$Path, [string]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Path; $info.Arguments = $Arguments
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    return [Diagnostics.Process]::Start($info)
}
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-helper-exit-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$session = Join-Path $fixture 'session'
$store = Join-Path $fixture 'packages'
$localHelper = Join-Path $fixture 'installer-package-helper.exe'
Copy-Item -LiteralPath $Helper -Destination $localHelper
$result = [ordered]@{state='failed'; fixture_root=$fixture; helper_sha256=$HelperSha256; live_profile_modified=$false; network_used=$false}
$sacrifice = $null; $helperProcess = $null
try {
    # A sacrificial parent stands in for the installer process.
    $sacrifice = Start-Native 'powershell.exe' '-NoProfile -NonInteractive -Command "Start-Sleep -Seconds 120"'
    Start-Sleep -Milliseconds 700
    $identity = [guid]::NewGuid().ToString('N')
    $helperProcess = Start-Native $localHelper ([string]$sacrifice.Id + ' "' + $session + '" "' + $store + '" ' + $identity)
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath (Join-Path $session 'reply-0.ini')) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 50 }
    if (-not (Test-Path -LiteralPath (Join-Path $session 'reply-0.ini'))) {
        $result.helper_exited_early = $helperProcess.HasExited
        if ($helperProcess.HasExited) { $result.helper_early_exit_code = $helperProcess.ExitCode }
        throw 'Helper did not become ready.'
    }
    $result.helper_ready = $true
    $result.helper_pid = $helperProcess.Id
    $sacrifice.Kill(); $sacrifice.WaitForExit(5000)
    $result.parent_terminated = $sacrifice.HasExited
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not $helperProcess.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    $result.helper_exited_on_parent_loss = $helperProcess.HasExited
    if ($helperProcess.HasExited) { $result.helper_exit_code = $helperProcess.ExitCode }
    if (-not ($result.helper_ready -and $result.parent_terminated -and $result.helper_exited_on_parent_loss)) { throw 'Helper did not exit after parent loss.' }
    $result.state = 'passed'
} catch {
    $result.error = $_.Exception.Message
} finally {
    if ($sacrifice -and -not $sacrifice.HasExited) { $sacrifice.Kill() }
    if ($helperProcess -and -not $helperProcess.HasExited) { $helperProcess.Kill() }
    $stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew)
    try { $b = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 4)); $stream.Write($b,0,$b.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}
if ($result.state -ne 'passed') { exit 1 }
Write-Output 'NATIVE_INSTALLER_HELPER_PARENT_EXIT_PASSED'
