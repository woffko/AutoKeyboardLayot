param(
    [Parameter(Mandatory = $true)]
    [string]$ResultPath
)
# Opens a fixture in Notepad and checks through UI Automation that the caret can be placed at its end
# and the seven characters before it read back. Run it in an interactive session; it writes the
# result to -ResultPath and prints it. A probe that fails records the step it failed in and exits 1.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

if (@(Get-Process notepad -ErrorAction SilentlyContinue | Where-Object {
    $_.MainWindowHandle -ne 0
}).Count -ne 0) {
    throw 'Refusing to run while another visible Notepad window exists.'
}

$fixture = Join-Path $env:TEMP 'AutoKeyboardLayot-uia-probe.txt'
[System.IO.File]::WriteAllText($fixture, 'ghbdtn ')
$started = $null
# The step that runs, recorded with a failure; $editor tells whether TextPattern was found.
$step = 'start Notepad'
$editor = $null
$failed = $false

try {
    $started = Start-Process -FilePath 'notepad.exe' -ArgumentList ('"' + $fixture + '"') -PassThru
    $step = 'find the probe window'
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    $windowProcess = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 100
        $candidates = @(Get-Process notepad -ErrorAction SilentlyContinue | Where-Object {
            $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle -like '*AutoKeyboardLayot-uia-probe*'
        })
        if ($candidates.Count -eq 1) {
            $windowProcess = $candidates[0]
            break
        }
    }
    if ($null -eq $windowProcess) {
        throw 'Notepad probe window was not found.'
    }

    $step = 'find the TextPattern element'
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($windowProcess.MainWindowHandle)
    $condition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::IsTextPatternAvailableProperty,
        $true
    )
    $editor = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
    if ($null -eq $editor) {
        throw 'Notepad exposes no TextPattern element.'
    }

    $step = 'place the caret at the end'
    $editor.SetFocus()
    $pattern = [System.Windows.Automation.TextPattern]$editor.GetCurrentPattern(
        [System.Windows.Automation.TextPattern]::Pattern
    )
    $document = $pattern.DocumentRange
    $caret = $document.Clone()
    $caret.MoveEndpointByRange(
        [System.Windows.Automation.Text.TextPatternRangeEndpoint]::Start,
        $document,
        [System.Windows.Automation.Text.TextPatternRangeEndpoint]::End
    )
    $caret.Select()
    Start-Sleep -Milliseconds 100

    $step = 'read the text before the caret'
    $selection = $pattern.GetSelection()
    $singleSelection = $selection.Count -eq 1
    $isCaret = $false
    $moved = 0
    $match = $false
    if ($singleSelection) {
        $selected = $selection[0]
        $isCaret = $selected.CompareEndpoints(
            [System.Windows.Automation.Text.TextPatternRangeEndpoint]::Start,
            $selected,
            [System.Windows.Automation.Text.TextPatternRangeEndpoint]::End
        ) -eq 0
        if ($isCaret) {
            $range = $selected.Clone()
            $moved = $range.MoveEndpointByUnit(
                [System.Windows.Automation.Text.TextPatternRangeEndpoint]::Start,
                [System.Windows.Automation.Text.TextUnit]::Character,
                -7
            )
            $match = $moved -eq -7 -and $range.GetText(-1) -eq 'ghbdtn '
        }
    }

    $step = 'write the result'
    [pscustomobject]@{
        ProcessId = $windowProcess.Id
        TextPattern = $true
        SingleSelection = $singleSelection
        Caret = $isCaret
        MovedCharacters = $moved
        ExactTailMatch = $match
    } | ConvertTo-Json | Set-Content -LiteralPath $ResultPath -Encoding UTF8
}
catch {
    $failed = $true
    [pscustomobject]@{
        Error = $_.Exception.Message
        FailedStep = $step
        TextPattern = ($null -ne $editor)
        ExactTailMatch = $false
    } | ConvertTo-Json | Set-Content -LiteralPath $ResultPath -Encoding UTF8
}
finally {
    @(Get-Process notepad -ErrorAction SilentlyContinue | Where-Object {
        $_.MainWindowTitle -like '*AutoKeyboardLayot-uia-probe*'
    }) | ForEach-Object {
        if (-not $_.CloseMainWindow()) {
            Stop-Process -Id $_.Id -Force
        }
    }
    Remove-Item -LiteralPath $fixture -Force -ErrorAction SilentlyContinue
}
Get-Content -LiteralPath $ResultPath
if ($failed) { exit 1 }
