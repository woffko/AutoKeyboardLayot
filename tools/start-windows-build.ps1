param(
    [Parameter(Mandatory = $true)][string]$TargetDirectory,
    [Parameter(Mandatory = $true)][string]$LogDirectory,
    [switch]$ProbeOnly,
    [switch]$LaunchOnly,
    [switch]$MonitorOnly
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path $PSScriptRoot -Parent
$verification = Join-Path $PSScriptRoot 'verify-windows.ps1'
$cargoExecutable = (Get-Command cargo.exe -ErrorAction Stop).Source
$powershellExecutable = Join-Path $PSHOME 'powershell.exe'

# WMI creates the build independently of the SSH/Longrun process tree. A
# monitor disconnect therefore cannot kill the compilation or erase its logs.
if ($LaunchOnly -and $MonitorOnly) { throw 'Choose either launch or monitor mode.' }
if ($MonitorOnly) {
    $launch = Get-Content -LiteralPath (Join-Path $LogDirectory 'launch.json') -Raw | ConvertFrom-Json
} else {
    if (Test-Path -LiteralPath $LogDirectory) {
        throw 'Use a fresh log directory for each build; previous evidence is retained.'
    }
    New-Item -ItemType Directory -Path $LogDirectory | Out-Null
    $commandLine = '"{0}" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{1}" -TargetDirectory "{2}" -LogDirectory "{3}" -CargoExecutable "{4}"' -f `
        $powershellExecutable, $verification, $TargetDirectory, $LogDirectory, $cargoExecutable
    if ($ProbeOnly) { $commandLine += ' -ProbeOnly' }
    $launch = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine = $commandLine
        CurrentDirectory = $repository
    }
    if ($launch.ReturnValue -ne 0) { throw ('Build launch failed: ' + $launch.ReturnValue) }
    $launch | Select-Object ProcessId,ReturnValue | ConvertTo-Json |
        Set-Content -LiteralPath (Join-Path $LogDirectory 'launch.json') -Encoding UTF8
}
Write-Output ('Build PID: ' + $launch.ProcessId)
Write-Output ('Build logs: ' + $LogDirectory)
if ($LaunchOnly) { exit 0 }

$deadline = [DateTime]::UtcNow.AddMinutes(50)
$lastStage = ''
while ([DateTime]::UtcNow -lt $deadline) {
    $resultPath = Join-Path $LogDirectory 'result.json'
    if (Test-Path -LiteralPath $resultPath) {
        $receipt = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
        if ($receipt.stage -ne $lastStage) {
            $lastStage = $receipt.stage
            Write-Output ('Stage: ' + $lastStage)
        }
        if ($receipt.state -ne 'running') {
            Get-Content -LiteralPath $resultPath
            if ($receipt.state -eq 'succeeded') {
                if ($ProbeOnly) {
                    Get-Content -LiteralPath (Join-Path $LogDirectory 'probe.stdout.log')
                    Write-Output 'AUTOKEY_WINDOWS_LAUNCH_PROBE_OK'
                } else {
                    Write-Output 'AUTOKEY_WINDOWS_VERIFIED'
                }
                exit 0
            }
            Get-Content -LiteralPath (Join-Path $LogDirectory ($receipt.stage + '.stderr.log')) -Tail 60
            exit 1
        }
    }
    if (-not (Get-Process -Id $launch.ProcessId -ErrorAction SilentlyContinue)) {
        throw 'Build process ended without a terminal receipt; inspect the retained stage logs.'
    }
    Start-Sleep -Seconds 3
}
throw 'Monitor timed out. The independent build and its logs are retained on the VM.'
