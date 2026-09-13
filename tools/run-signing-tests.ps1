param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ExpectedSha256,
    [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
if ($ExpectedSha256 -notmatch '^[a-f0-9]{64}$') { throw 'Invalid hash.' }
if ((Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash -ne $ExpectedSha256) { throw 'Artifact hash mismatch.' }
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = $Executable
$start.Arguments = '--test-threads=1 native::tests::'
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($start)
$stdoutTask = $process.StandardOutput.ReadToEndAsync()
$stderrTask = $process.StandardError.ReadToEndAsync()
if (-not $process.WaitForExit(15000)) {
    $process.Kill()
    throw 'Only the signing test process was stopped after timeout.'
}
$stdout = $stdoutTask.GetAwaiter().GetResult()
$stderr = $stderrTask.GetAwaiter().GetResult()
$process.WaitForExit()
$passed = $process.ExitCode -eq 0 -and $stdout -match 'test result: ok\. 3 passed; 0 failed;'
$result = [ordered]@{passed=$passed; expected_tests=3; exit_code=$process.ExitCode; sha256=$ExpectedSha256; production_key_used=$false; stdout=$stdout; stderr=$stderr}
$json = $result | ConvertTo-Json -Depth 4
$stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
try {
    $bytes = [Text.Encoding]::UTF8.GetBytes($json)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
} finally { $stream.Dispose() }
Write-Output $stdout
Write-Output $stderr
if (-not $passed) { exit 1 }
Write-Output 'NATIVE_SIGNING_FIXTURE_TESTS_PASSED'
