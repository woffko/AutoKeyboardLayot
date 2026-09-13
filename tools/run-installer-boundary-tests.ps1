param(
    [Parameter(Mandatory=$true)][string]$ArtifactManifest,
    [Parameter(Mandatory=$true)][string]$TestDirectory,
    [Parameter(Mandatory=$true)][string]$LogDirectory
)
$ErrorActionPreference = 'Stop'
$manifest = Get-Content -LiteralPath $ArtifactManifest -Raw | ConvertFrom-Json
if ($manifest.variant -ne 'base') { throw 'Expected base test artifacts.' }
if (Test-Path -LiteralPath $LogDirectory) { throw 'Use a new log directory.' }
New-Item -ItemType Directory -Path $LogDirectory | Out-Null
$profileDirectory = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-boundary-tests-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $profileDirectory | Out-Null
$previousLocalAppData = $env:LOCALAPPDATA
$checks = @(
    @{Tag='core';Filter='base_build';Expected=2},
    @{Tag='base_integration';Filter='';Expected=2},
    @{Tag='adapter';Filter='windows_agent::installer_lifecycle::tests::';Expected=4},
    @{Tag='adapter';Filter='windows_agent::tests::graceful_shutdown';Expected=2},
    @{Tag='adapter';Filter='normal_close_requires_valid_unchanged_settings';Expected=1}
)
$receipt = [ordered]@{state='running'; full_suite=$false; typing_acceptance=$false; tests=@(); error=$null}
try {
    $env:LOCALAPPDATA = $profileDirectory
    $index = 0
    foreach ($check in $checks) {
        $artifact = $manifest.artifacts.($check.Tag)
        if ($artifact.file -notmatch '^[A-Za-z0-9_-]+\.exe$' -or $artifact.sha256 -notmatch '^[a-f0-9]{64}$') {
            throw 'Invalid test artifact record.'
        }
        $path = Join-Path $TestDirectory $artifact.file
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $artifact.sha256) {
            throw 'Test executable hash mismatch.'
        }
        $startInfo = New-Object System.Diagnostics.ProcessStartInfo
        $startInfo.FileName = $path
        $startInfo.Arguments = '--test-threads=1 ' + $check.Filter
        $startInfo.WorkingDirectory = $profileDirectory
        $startInfo.UseShellExecute = $false
        $startInfo.CreateNoWindow = $true
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        $process = [System.Diagnostics.Process]::Start($startInfo)
        if (-not $process.WaitForExit(10000)) {
            $process.Kill()
            throw 'A selected test process timed out; only that test process was stopped.'
        }
        $stdout = $process.StandardOutput.ReadToEnd()
        $stderr = $process.StandardError.ReadToEnd()
        $process.WaitForExit()
        [IO.File]::WriteAllText((Join-Path $LogDirectory ($index.ToString() + '.stdout.txt')), $stdout)
        [IO.File]::WriteAllText((Join-Path $LogDirectory ($index.ToString() + '.stderr.txt')), $stderr)
        $receipt.tests += @{tag=$check.Tag; filter=$check.Filter; expected=$check.Expected; exit_code=$process.ExitCode; sha256=$artifact.sha256}
        if ($process.ExitCode -ne 0 -or $stdout -notmatch ('test result: ok\. ' + $check.Expected + ' passed; 0 failed;')) {
            Write-Output $stdout
            Write-Output $stderr
            throw 'Selected test result did not match its expected nonzero test count.'
        }
        Write-Output ($check.Tag + ': ' + $check.Expected + ' selected tests passed')
        $index++
    }
    $receipt.state = 'succeeded'
} catch {
    $receipt.state = 'failed'
    $receipt.error = $_.Exception.Message
    Write-Output $receipt.error
} finally {
    $env:LOCALAPPDATA = $previousLocalAppData
    $receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $LogDirectory 'result.json') -Encoding UTF8
}
if ($receipt.state -ne 'succeeded') { exit 1 }
Write-Output 'INSTALLER_BOUNDARY_TESTS_PASSED_NOT_TYPING_ACCEPTANCE'
