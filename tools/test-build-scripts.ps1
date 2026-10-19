param(
    [Parameter(Mandatory=$true)][string]$ScriptDirectory,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
# Acceptance of the native build and test scripts against the races they must survive, with
# stand-ins for cargo and for a test executable compiled here; nothing is built or installed:
#   receipt-under-reader   verify-windows.ps1 replaces result.json while another process holds it
#                          open; the replacement waits and the probe build still succeeds
#   monitor-under-lock     start-windows-build.ps1 -MonitorOnly meets a receipt that is locked for
#                          a while; it reads it again and reports the finished build
#   boundary-output        run-installer-boundary-tests.ps1 runs test executables that write more
#                          than a pipe holds; every check passes instead of timing out
# -ScriptDirectory holds the three scripts under test (the current ones or older copies).
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$work = Join-Path $OutputDirectory 'work'
$bin = Join-Path $work 'bin'
New-Item -ItemType Directory -Path $bin | Out-Null
$powershell = Join-Path $PSHOME 'powershell.exe'
$result = [ordered]@{state='failed'; scripts=$ScriptDirectory; cases=[ordered]@{}}

# cargo stand-in: answers --version after four seconds.
Add-Type -OutputType ConsoleApplication -OutputAssembly (Join-Path $bin 'cargo.exe') -TypeDefinition @'
public static class Cargo {
    public static int Main(string[] args) {
        System.Threading.Thread.Sleep(4000);
        System.Console.WriteLine("cargo 0.0.0 (stand-in)");
        return 0;
    }
}
'@
# Test executable stand-in: 256 KiB of output, then the summary the boundary runner expects.
Add-Type -OutputType ConsoleApplication -OutputAssembly (Join-Path $bin 'stubtests.exe') -TypeDefinition @'
public static class Tests {
    public static int Main(string[] args) {
        string filter = string.Join(" ", args);
        int count = filter.Contains("installer_lifecycle") ? 4 : filter.Contains("normal_close") ? 1 : 2;
        string line = new string('.', 1023);
        for (int i = 0; i < 256; i++) System.Console.WriteLine(line);
        System.Console.WriteLine("test result: ok. " + count + " passed; 0 failed; 0 ignored; 0 measured; 0 filtered out");
        return 0;
    }
}
'@

function Start-Helper([string]$Script) {
    $file = Join-Path $work ('helper-' + [guid]::NewGuid().ToString('N') + '.ps1')
    Set-Content -LiteralPath $file -Value $Script -Encoding UTF8
    return Start-Process -FilePath $powershell -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $file) -PassThru -WindowStyle Hidden
}

function Run-Script([string]$Name, [string[]]$Arguments, [int]$Seconds) {
    $out = Join-Path $OutputDirectory ($Name + '.out.txt')
    $process = Start-Process -FilePath $powershell -ArgumentList (@('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $ScriptDirectory $Name)) + $Arguments) `
        -PassThru -NoNewWindow -RedirectStandardOutput $out -RedirectStandardError ($out + '.err')
    $null = $process.Handle  # without it, ExitCode stays empty
    if (-not $process.WaitForExit($Seconds * 1000)) { $process.Kill(); return @{exit=-1; output='timed out'} }
    return @{exit=$process.ExitCode; output=((Get-Content -LiteralPath $out -Raw) + (Get-Content -LiteralPath ($out + '.err') -Raw))}
}

try {
    # receipt-under-reader
    $logs = Join-Path $work 'receipt-logs'
    New-Item -ItemType Directory -Path $logs | Out-Null
    $receipt = Join-Path $logs 'result.json'
    $holder = Start-Helper @"
`$path = '$receipt'
while (-not (Test-Path -LiteralPath `$path)) { Start-Sleep -Milliseconds 10 }
`$stream = [IO.File]::Open(`$path, 'Open', 'Read', 'Read')
Start-Sleep -Seconds 7
`$stream.Dispose()
"@
    $run = Run-Script 'verify-windows.ps1' @('-TargetDirectory', (Join-Path $work 'target'), '-LogDirectory', $logs, '-CargoExecutable', (Join-Path $bin 'cargo.exe'), '-ProbeOnly') 60
    $holder.WaitForExit()
    $state = $null
    try { $state = (Get-Content -LiteralPath $receipt -Raw | ConvertFrom-Json).state } catch {}
    $result.cases['receipt-under-reader'] = [ordered]@{exit=$run.exit; receipt_state=$state; passed=($run.exit -eq 0 -and $state -eq 'succeeded')}

    # monitor-under-lock
    $logs = Join-Path $work 'monitor-logs'
    New-Item -ItemType Directory -Path $logs | Out-Null
    $receipt = Join-Path $logs 'result.json'
    '{"state":"running","stage":"tests"}' | Set-Content -LiteralPath $receipt -Encoding UTF8
    $locked = Join-Path $logs 'locked'
    $builder = Start-Helper @"
`$stream = [IO.File]::Open('$receipt', 'Open', 'ReadWrite', 'None')
New-Item -ItemType File -Path '$locked' | Out-Null
Start-Sleep -Seconds 5
`$stream.Dispose()
'{"state":"succeeded","stage":"release"}' | Set-Content -LiteralPath '$receipt' -Encoding UTF8
Start-Sleep -Seconds 20
"@
    while (-not (Test-Path -LiteralPath $locked)) { Start-Sleep -Milliseconds 50 }
    @{ProcessId=$builder.Id; ReturnValue=0} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $logs 'launch.json') -Encoding UTF8
    $previousPath = $env:Path
    $env:Path = $bin + ';' + $env:Path
    try { $run = Run-Script 'start-windows-build.ps1' @('-TargetDirectory', (Join-Path $work 'target'), '-LogDirectory', $logs, '-MonitorOnly') 60 }
    finally { $env:Path = $previousPath }
    if (-not $builder.HasExited) { $builder.Kill() }
    $result.cases['monitor-under-lock'] = [ordered]@{exit=$run.exit; verified=($run.output -like '*AUTOKEY_WINDOWS_VERIFIED*');
        passed=($run.exit -eq 0 -and $run.output -like '*AUTOKEY_WINDOWS_VERIFIED*')}

    # boundary-output
    $tests = Join-Path $work 'tests'
    New-Item -ItemType Directory -Path $tests | Out-Null
    Copy-Item -LiteralPath (Join-Path $bin 'stubtests.exe') -Destination $tests
    $hash = (Get-FileHash -LiteralPath (Join-Path $tests 'stubtests.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
    $artifact = @{file='stubtests.exe'; sha256=$hash}
    $manifest = Join-Path $work 'manifest.json'
    @{variant='base'; artifacts=@{core=$artifact; base_integration=$artifact; adapter=$artifact}} | ConvertTo-Json -Depth 4 |
        Set-Content -LiteralPath $manifest -Encoding UTF8
    $run = Run-Script 'run-installer-boundary-tests.ps1' @('-ArtifactManifest', $manifest, '-TestDirectory', $tests, '-LogDirectory', (Join-Path $work 'boundary-logs')) 120
    $result.cases['boundary-output'] = [ordered]@{exit=$run.exit; passed=($run.exit -eq 0 -and $run.output -like '*INSTALLER_BOUNDARY_TESTS_PASSED*');
        timed_out=($run.output -like '*timed out*')}

    $failed = @($result.cases.Values | Where-Object { -not $_.passed })
    $result.state = if ($failed.Count -eq 0) { 'passed' } else { 'failed' }
} catch {
    $result.error = $_.Exception.Message
} finally {
    $result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'result.json') -Encoding UTF8
}
$result | ConvertTo-Json -Depth 4
if ($result.state -ne 'passed') { exit 1 }
