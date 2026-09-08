param(
    [string]$ResultPath = (Join-Path (Split-Path $PSScriptRoot -Parent) 'target\notepad-uia-probe-result.json')
)
$ErrorActionPreference = 'Stop'
$resultDirectory = Split-Path ([IO.Path]::GetFullPath($ResultPath)) -Parent
New-Item -ItemType Directory -Path $resultDirectory -Force | Out-Null
& (Join-Path $PSScriptRoot 'notepad-uia-probe.ps1') -ResultPath $ResultPath
