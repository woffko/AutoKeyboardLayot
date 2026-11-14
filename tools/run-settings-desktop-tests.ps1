param(
    [Parameter(Mandatory = $true)][string]$TestExecutable,
    [Parameter(Mandatory = $true)][string]$LogDirectory,
    [string]$Name = '',
    [int]$TimeoutSeconds = 300
)
# Runs the settings window's desktop acceptance: the ignored tests of
# windows_agent::settings_window::desktop_acceptance in the agent's test executable
# (cargo test --no-run --bin AutoKeyboardLayot), in an interactive session of a test machine such
# as the test VM's. Never on a desktop in use: the tests open the settings window, click, type
# and take screenshots, and some download published packages. Each test opens the window once
# in its process, so each runs in a process of its own, by its exact name with
# --include-ignored. -Name keeps the tests whose name contains it. Logs, one per test, and
# result.json go to a new -LogDirectory; the exit code is 1 when any test failed.
$ErrorActionPreference = 'Stop'
$module = 'windows_agent::settings_window::desktop_acceptance::'
if (Test-Path -LiteralPath $LogDirectory) { throw 'Use a new log directory.' }
New-Item -ItemType Directory -Path $LogDirectory | Out-Null
$executable = (Resolve-Path -LiteralPath $TestExecutable).Path
# A test sets its own profile; one that fails before that finds this one, never the user's.
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-settings-desktop-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null

function Run-Tests([string]$Arguments, [int]$Seconds) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $executable
    $info.Arguments = $Arguments
    $info.WorkingDirectory = $fixture
    $info.EnvironmentVariables['LOCALAPPDATA'] = $fixture
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($info)
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $timedOut = -not $process.WaitForExit($Seconds * 1000)
    if ($timedOut) { $process.Kill() }
    $process.WaitForExit()
    return @{exit_code=$process.ExitCode; timed_out=$timedOut; seconds=[int]$clock.Elapsed.TotalSeconds;
        output=($stdoutTask.GetAwaiter().GetResult() + $stderrTask.GetAwaiter().GetResult())}
}

$list = Run-Tests '--list --ignored' 60
[IO.File]::WriteAllText((Join-Path $LogDirectory 'list.txt'), $list.output)
if ($list.exit_code -ne 0) { throw 'The test executable did not list its tests.' }
$names = @($list.output -split "`r?`n" | ForEach-Object {
    if ($_ -match ('^(' + [regex]::Escape($module) + '\w+): test$')) { $matches[1] }
} | Where-Object { $_.Substring($module.Length).Contains($Name) })
if ($names.Count -eq 0) { throw 'No settings window desktop test is listed.' }

$tests = New-Object Collections.ArrayList
foreach ($test in $names) {
    $short = $test.Substring($module.Length)
    $run = Run-Tests ('--include-ignored --exact ' + $test + ' --nocapture --test-threads=1') $TimeoutSeconds
    [IO.File]::WriteAllText((Join-Path $LogDirectory ($short + '.txt')), $run.output)
    $passed = $run.exit_code -eq 0 -and $run.output -match 'test result: ok\. 1 passed; 0 failed'
    Write-Output ($(if ($passed) { 'PASS ' } else { 'FAIL ' }) + $short + ' (' + $run.seconds + ' s)')
    [void]$tests.Add([ordered]@{name=$short; passed=$passed; exit_code=$run.exit_code; timed_out=$run.timed_out;
        seconds=$run.seconds})
}
$failed = @($tests | Where-Object { -not $_.passed })
$receipt = [ordered]@{state=$(if ($failed.Count -eq 0) { 'passed' } else { 'failed' }); total=$names.Count;
    failed=$failed.Count; fixture=$fixture; tests=@($tests)}
$receipt | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $LogDirectory 'result.json') -Encoding UTF8
if ($failed.Count -gt 0) {
    Write-Output ('SETTINGS_DESKTOP_TESTS_FAILED ' + $failed.Count + ' of ' + $names.Count)
    exit 1
}
Write-Output ('SETTINGS_DESKTOP_TESTS_PASSED ' + $names.Count)
