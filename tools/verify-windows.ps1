param(
    [Parameter(Mandatory = $true)]
    [string]$TargetDirectory,
    [Parameter(Mandatory = $true)]
    [string]$LogDirectory,
    [string]$CargoExecutable = 'cargo.exe',
    [switch]$ProbeOnly
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$env:CARGO_TARGET_DIR = $TargetDirectory
New-Item -ItemType Directory -Path $LogDirectory -Force | Out-Null
$receipt = [ordered]@{
    state = 'running'
    stage = 'starting'
    phase = 'starting'
    child_pid = $null
    child_exit_code = $null
    started_utc = [DateTime]::UtcNow.ToString('o')
    finished_utc = $null
    artifact = $null
    sha256 = $null
    error = $null
}

function Write-Receipt {
    $temporary = Join-Path $LogDirectory 'result.tmp'
    $receipt | ConvertTo-Json | Set-Content -LiteralPath $temporary -Encoding UTF8
    Move-Item -LiteralPath $temporary -Destination (Join-Path $LogDirectory 'result.json') -Force
}

function Invoke-CargoStage([string]$Stage, [string[]]$CargoArguments) {
    $receipt.stage = $Stage
    $receipt.phase = 'launching'
    $receipt.child_pid = $null
    $receipt.child_exit_code = $null
    Write-Receipt
    $standardOutput = Join-Path $LogDirectory ($Stage + '.stdout.log')
    $standardError = Join-Path $LogDirectory ($Stage + '.stderr.log')
    $process = Start-Process -FilePath $CargoExecutable -ArgumentList $CargoArguments `
        -PassThru -NoNewWindow -RedirectStandardOutput $standardOutput `
        -RedirectStandardError $standardError
    # Start-Process -Wait waits for the whole process tree. Compiler telemetry
    # helpers can remain alive after cargo and every test have already exited.
    # Cargo itself owns the compiler/test lifecycle; wait on its handle only.
    $null = $process.Handle
    $receipt.child_pid = $process.Id
    $receipt.phase = 'waiting-for-cargo'
    Write-Receipt
    $process.WaitForExit()
    $receipt.child_exit_code = $process.ExitCode
    if ($null -eq $receipt.child_exit_code) { throw ($Stage + ' returned no exit code') }
    $receipt.phase = 'cargo-exited'
    Write-Receipt
    # Keep verbose output in the stage files. A detached verifier must not
    # depend on the lifetime/capacity of an inherited console or SSH pipe.
    if ($receipt.child_exit_code -ne 0) {
        throw ($Stage + ' failed, exit code ' + $receipt.child_exit_code)
    }
}

# Adapter unit tests must not read or edit the interactive user's profile.
# Retain the temporary directory for inspection; this script never cleans it.
$testProfile = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-tests-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $testProfile | Out-Null
$originalLocalAppData = $env:LOCALAPPDATA

try {
    if ($ProbeOnly) {
        Invoke-CargoStage 'probe' @('--version')
        $receipt.state = 'succeeded'
        return
    }
    Invoke-CargoStage 'format' @('fmt', '--all', '--', '--check')

    $env:LOCALAPPDATA = $testProfile
    Invoke-CargoStage 'tests' @('test', '--locked', '--all-targets')
    $env:LOCALAPPDATA = $originalLocalAppData

    Invoke-CargoStage 'clippy' @('clippy', '--locked', '--all-targets', '--all-features', '--', '-D', 'warnings')

    Invoke-CargoStage 'release' @('build', '--locked', '--release')

    $artifact = Join-Path $TargetDirectory 'release\AutoKeyboardLayot.exe'
    $artifactInfo = Get-Item -LiteralPath $artifact
    $artifactHash = (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash
    Write-Output ('Artifact: ' + $artifactInfo.FullName)
    Write-Output ('Size: ' + $artifactInfo.Length)
    Write-Output ('SHA256: ' + $artifactHash)
    $receipt.artifact = $artifactInfo.FullName
    $receipt.sha256 = $artifactHash
    $receipt.state = 'succeeded'
    Write-Output 'AUTOKEY_WINDOWS_VERIFIED'
} catch {
    $receipt.state = 'failed'
    $receipt.error = $_.Exception.Message
    Write-Output $receipt.error
} finally {
    $env:LOCALAPPDATA = $originalLocalAppData
    $receipt.finished_utc = [DateTime]::UtcNow.ToString('o')
    Write-Receipt
}
if ($receipt.state -ne 'succeeded') { exit 1 }
