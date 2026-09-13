param(
    [ValidateSet('package', 'catalog')][string]$Kind = 'package',
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ExecutableSha256,
    [Parameter(Mandatory=$true)][string]$InputFile,
    [Parameter(Mandatory=$true)][string]$InputSha256,
    [Parameter(Mandatory=$true)][string]$OutputFile
)
$ErrorActionPreference = 'Stop'
foreach ($hash in @($ExecutableSha256, $InputSha256)) {
    if ($hash -notmatch '^[a-f0-9]{64}$') { throw 'Invalid expected hash.' }
}
if ((Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash -ne $ExecutableSha256) { throw 'Signer executable hash mismatch.' }
if ((Get-FileHash -LiteralPath $InputFile -Algorithm SHA256).Hash -ne $InputSha256) { throw 'Reviewed input hash mismatch.' }
if (Test-Path -LiteralPath $OutputFile) { throw 'Output already exists; inspect, do not overwrite.' }
foreach ($path in @($InputFile, $OutputFile)) {
    if ($path.Contains('"') -or $path.Contains("`r") -or $path.Contains("`n")) { throw 'Invalid path argument.' }
}
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = $Executable
$start.Arguments = $Kind + ' "' + $InputFile + '" "' + $OutputFile + '"'
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($start)
$stdoutTask = $process.StandardOutput.ReadToEndAsync()
$stderrTask = $process.StandardError.ReadToEndAsync()
if (-not $process.WaitForExit(15000)) {
    $process.Kill()
    throw 'Signer timeout; inspect output before any retry.'
}
$stdout = $stdoutTask.GetAwaiter().GetResult()
$stderr = $stderrTask.GetAwaiter().GetResult()
$process.WaitForExit()
if ($process.ExitCode -ne 0 -or $stdout.Trim() -ne 'SIGNED_OUTPUT_VERIFIED') {
    Write-Output $stderr
    throw 'Signing failed; no automatic retry.'
}
[ordered]@{state='signed'; input_sha256=$InputSha256; signer_executable_sha256=$ExecutableSha256; output_sha256=(Get-FileHash -LiteralPath $OutputFile -Algorithm SHA256).Hash.ToLowerInvariant(); output_bytes=(Get-Item -LiteralPath $OutputFile).Length} | ConvertTo-Json
