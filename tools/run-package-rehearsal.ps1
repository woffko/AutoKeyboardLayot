param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ExpectedSha256,
    [Parameter(Mandatory=$true)][string]$ArtifactDirectory,
    [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
if ($ExpectedSha256 -notmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash -ne $ExpectedSha256) { throw 'Artifact hash mismatch.' }
if (Test-Path -LiteralPath $Receipt) { throw 'Use a new receipt path.' }
foreach ($path in @($ArtifactDirectory, $Receipt)) {
    if ($path.Contains('"') -or $path.Contains("`r") -or $path.Contains("`n")) { throw 'Invalid argument path.' }
}
$start = New-Object Diagnostics.ProcessStartInfo
$start.FileName = $Executable
$start.Arguments = '"' + $ArtifactDirectory + '" "' + $Receipt + '"'
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($start)
$stdoutTask = $process.StandardOutput.ReadToEndAsync()
$stderrTask = $process.StandardError.ReadToEndAsync()
if (-not $process.WaitForExit(20000)) {
    $process.Kill()
    throw 'Only the isolated rehearsal process was stopped after timeout; inspect its temporary state.'
}
$stdout = $stdoutTask.GetAwaiter().GetResult()
$stderr = $stderrTask.GetAwaiter().GetResult()
$process.WaitForExit()
Write-Output $stdout
Write-Output $stderr
if ($process.ExitCode -ne 0 -or $stdout.Trim() -ne 'ISOLATED_REAL_UI_PACKAGE_REHEARSAL_PASSED') { throw 'Native rehearsal failed.' }
$result = Get-Content -LiteralPath $Receipt -Raw | ConvertFrom-Json
if ($result.state -ne 'passed' -or $result.packages -ne 13 -or -not $result.temporary_store_removed -or $result.live_profile_modified) { throw 'Unexpected rehearsal receipt.' }
Write-Output 'NATIVE_ISOLATED_PACKAGE_REHEARSAL_VERIFIED'
