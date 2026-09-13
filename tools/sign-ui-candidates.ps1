param(
    [Parameter(Mandatory=$true)][string]$Directory,
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ExecutableSha256
)
$ErrorActionPreference = 'Stop'
$entries = Get-Content -LiteralPath (Join-Path $Directory 'remaining-inputs.json') -Raw | ConvertFrom-Json
if ($entries.Count -ne 12) { throw 'Expected the twelve remaining reviewed candidates.' }
$seen = @{}
foreach ($entry in $entries) {
    if ($entry.input -notmatch '^[a-z]{2}-[A-Z]{2}\.unsigned\.json$' -or $seen.ContainsKey($entry.input)) { throw 'Invalid or duplicate input name.' }
    $seen[$entry.input] = $true
    $output = $entry.input.Replace('.unsigned.json', '-r1.aklp')
    if (Test-Path -LiteralPath (Join-Path $Directory $output)) { throw 'Existing output requires inspection.' }
    if ((Get-FileHash -LiteralPath (Join-Path $Directory $entry.input) -Algorithm SHA256).Hash -ne $entry.sha256) { throw 'Input hash mismatch.' }
}
foreach ($entry in $entries) {
    & (Join-Path $PSScriptRoot 'sign-package.ps1') -Executable $Executable -ExecutableSha256 $ExecutableSha256 -InputFile (Join-Path $Directory $entry.input) -InputSha256 $entry.sha256 -OutputFile (Join-Path $Directory $entry.input.Replace('.unsigned.json', '-r1.aklp'))
}
Write-Output 'REMAINING_UI_CANDIDATES_SIGNED'
