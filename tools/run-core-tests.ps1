param(
    [Parameter(Mandatory=$true)][string]$ArtifactManifest,
    [Parameter(Mandatory=$true)][string]$TestDirectory,
    [Parameter(Mandatory=$true)][string]$LogDirectory
)
$ErrorActionPreference = 'Stop'
$manifest = Get-Content -LiteralPath $ArtifactManifest -Raw | ConvertFrom-Json
$artifact = $manifest.artifacts.core
if ($manifest.variant -ne 'base' -or $artifact.file -notmatch '^[A-Za-z0-9_-]+\.exe$' -or $artifact.sha256 -notmatch '^[a-f0-9]{64}$') { throw 'Invalid core artifact receipt.' }
$executable = Join-Path $TestDirectory $artifact.file
if ((Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash -ne $artifact.sha256) { throw 'Core hash mismatch.' }
if (Test-Path -LiteralPath $LogDirectory) { throw 'Use a new log directory.' }
New-Item -ItemType Directory -Path $LogDirectory | Out-Null
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-core-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
function Run-Core([string]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $executable
    $info.Arguments = $Arguments
    $info.WorkingDirectory = $fixture
    $info.EnvironmentVariables['LOCALAPPDATA'] = $fixture
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($info)
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(25000)) { $process.Kill(); throw 'Only the core test process was stopped after timeout.' }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    $process.WaitForExit()
    return @{exit_code=$process.ExitCode;stdout=$stdout;stderr=$stderr}
}
$list = Run-Core '--list'
[IO.File]::WriteAllText((Join-Path $LogDirectory 'list.txt'), $list.stdout)
if ($list.exit_code -ne 0 -or $list.stdout -notmatch '(\d+) tests, 0 benchmarks') { throw 'Core test listing failed.' }
$expected = [int]$matches[1]
if ($expected -lt 250 -or $expected -gt 1000) { throw 'Unexpected core test count.' }
$run = Run-Core '--test-threads=1'
[IO.File]::WriteAllText((Join-Path $LogDirectory 'stdout.txt'), $run.stdout)
[IO.File]::WriteAllText((Join-Path $LogDirectory 'stderr.txt'), $run.stderr)
$ok = $run.exit_code -eq 0 -and $run.stdout -match 'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored; 0 measured; 0 filtered out'
$passed = 0; $ignored = 0
if ($ok) { $passed=[int]$matches[1]; $ignored=[int]$matches[2]; $ok=($passed + $ignored -eq $expected) }
$receipt = @{state=$(if($ok){'passed'}else{'failed'}); expected=$expected; passed=$passed; ignored=$ignored;
    exit_code=$run.exit_code; sha256=$artifact.sha256; fixture=$fixture; core_only=$true; typing_acceptance=$false}
$receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $LogDirectory 'result.json') -Encoding UTF8
if (-not $ok) { Write-Output $run.stdout; Write-Output $run.stderr; exit 1 }
Write-Output ('WINDOWS_CORE_TESTS_PASSED ' + $passed + '; ignored=' + $ignored)
