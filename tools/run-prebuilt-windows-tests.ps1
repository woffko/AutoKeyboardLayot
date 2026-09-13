param(
    [Parameter(Mandatory=$true)][string]$TestDirectory,
    [Parameter(Mandatory=$true)][string]$LogDirectory,
    [Parameter(Mandatory=$true)][string]$CoreExecutable,
    [Parameter(Mandatory=$true)][string]$AdapterExecutable
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $LogDirectory) { throw 'Use a new test log directory.' }
New-Item -ItemType Directory -Path $LogDirectory | Out-Null
$testProfile = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-prebuilt-tests-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $testProfile | Out-Null
$previousLocalAppData = $env:LOCALAPPDATA
$receipt = [ordered]@{state='running'; started_utc=[DateTime]::UtcNow.ToString('o'); tests=@(); error=$null}
try {
    $env:LOCALAPPDATA = $testProfile
    foreach ($entry in @(@{name='core';file=$CoreExecutable;expected=208}, @{name='adapter';file=$AdapterExecutable;expected=100})) {
        $executable = Join-Path $TestDirectory $entry.file
        $hash = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash
        $stdout = Join-Path $LogDirectory ($entry.name + '.stdout.log')
        $stderr = Join-Path $LogDirectory ($entry.name + '.stderr.log')
        $process = Start-Process -FilePath $executable -ArgumentList '--test-threads=1' -PassThru -NoNewWindow `
            -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        $null = $process.Handle
        if (-not $process.WaitForExit(20000)) {
            $process.Kill()
            throw ($entry.name + ' test executable timed out; only that test process was stopped')
        }
        $process.WaitForExit()
        $exitCode = $process.ExitCode
        $output = Get-Content -LiteralPath $stdout -Raw
        $pattern = 'test result: ok\. ' + $entry.expected + ' passed; 0 failed;'
        $receipt.tests += @{name=$entry.name;file=$executable;sha256=$hash;exit_code=$exitCode;expected=$entry.expected}
        if ($null -eq $exitCode -or $exitCode -ne 0 -or $output -notmatch $pattern) {
            Get-Content -LiteralPath $stdout
            Get-Content -LiteralPath $stderr
            throw ($entry.name + ' failed or its expected test summary is missing')
        }
        Write-Output ($entry.name + ': ' + $entry.expected + ' tests passed; exit=0')
    }
    $receipt.state = 'succeeded'
} catch {
    $receipt.state = 'failed'
    $receipt.error = $_.Exception.Message
    Write-Output $receipt.error
} finally {
    $env:LOCALAPPDATA = $previousLocalAppData
    $receipt.finished_utc = [DateTime]::UtcNow.ToString('o')
    $receipt | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $LogDirectory 'result.json') -Encoding UTF8
}
if ($receipt.state -ne 'succeeded') { exit 1 }
Write-Output 'AUTOKEY_PREBUILT_WINDOWS_TESTS_OK'
