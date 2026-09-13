param(
    [switch]$FreshProfile,
    [Parameter(Mandatory=$true)][string]$Helper,
    [Parameter(Mandatory=$true)][string]$HelperSha256,
    [Parameter(Mandatory=$true)][string]$Initializer,
    [Parameter(Mandatory=$true)][string]$InitializerSha256,
    [Parameter(Mandatory=$true)][string]$Catalog,
    [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
foreach ($item in @(@($Helper, $HelperSha256), @($Initializer, $InitializerSha256))) {
    if ($item[1] -notmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath $item[0] -Algorithm SHA256).Hash -ne $item[1]) { throw 'Executable hash mismatch.' }
}
function Start-Native([string]$Path, [string]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Path
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    return [Diagnostics.Process]::Start($info)
}
function Wait-File([string]$Path) {
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    while (-not (Test-Path -LiteralPath $Path)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Reply timeout.' }
        Start-Sleep -Milliseconds 20
    }
}
function Read-Ini([string]$Path) {
    $values = @{}
    $section = ''
    foreach ($line in [IO.File]::ReadAllLines($Path)) {
        if ($line -match '^\[([^\]]+)\]$') { $section = $matches[1] }
        elseif ($line -match '^([^=]+)=(.*)$') { $values[$section + '.' + $matches[1]] = $matches[2] }
    }
    return $values
}
function Send-Request($Command, [bool]$Wait = $true) {
    $number = $script:taskRequestNumber
    $script:taskRequestNumber++
    $request = @{format=1; session=$script:taskIdentity; sequence=$number; command=$Command} | ConvertTo-Json -Depth 5 -Compress
    $path = Join-Path $script:taskSessionDirectory ('request-' + $number + '.json')
    [IO.File]::WriteAllText($path + '.writing', $request, (New-Object Text.UTF8Encoding($false)))
    [IO.File]::Move($path + '.writing', $path)
    if (-not $Wait) { return }
    $replyPath = Join-Path $script:taskSessionDirectory ('reply-' + $number + '.ini')
    Wait-File $replyPath
    $reply = Read-Ini $replyPath
    if ($reply['helper.session'] -ne $script:taskIdentity -or $reply['helper.sequence'] -ne [string]$number) { throw 'Reply correlation mismatch.' }
    return $reply
}

$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-helper-fixture-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
$store = Join-Path $fixtureRoot 'packages'
if (-not $FreshProfile) {
    $initializerProcess = Start-Native $Initializer ('"' + $store + '"')
    if (-not $initializerProcess.WaitForExit(8000) -or $initializerProcess.ExitCode -ne 0) { throw 'Fixture initialization failed.' }
    $initialHash = (Get-FileHash -LiteralPath (Join-Path $store 'CURRENT') -Algorithm SHA256).Hash
}
$script:taskIdentity = [guid]::NewGuid().ToString('N')
$script:taskRequestNumber = 1
$script:taskSessionDirectory = Join-Path $fixtureRoot 'session'
$process = Start-Native $Helper ([string]$PID + ' "' + $script:taskSessionDirectory + '" "' + $store + '" ' + $script:taskIdentity)
try {
    Wait-File (Join-Path $script:taskSessionDirectory 'reply-0.ini')
    $reply = Send-Request @{action='check_catalog';local_file=$Catalog}
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    while ($reply['helper.busy'] -eq '1') {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Catalog worker timeout.' }
        Start-Sleep -Milliseconds 25
        $reply = Send-Request @{action='poll'}
    }
    if ($reply['helper.state'] -ne 'selecting' -or $reply['catalog.count'] -ne '13' -or $reply['catalog.selected'] -ne '0') { throw 'Catalog page mismatch.' }
    $oldView = [long]$reply['helper.view']
    $reply = Send-Request @{action='select';view=$oldView;ids=@('ru-RU')}
    if ($reply['helper.operation'] -ne 'ok' -or $reply['catalog.selected'] -ne '1' -or [long]$reply['helper.view'] -eq $oldView) { throw 'Selection failed.' }
    # Deliberately stale approval MUST refuse before lease checks or networking.
    $reply = Send-Request @{action='confirm_download';view=$oldView}
    if ($reply['helper.operation'] -ne 'rejected' -or $reply['helper.busy'] -ne '0') { throw 'Stale approval accepted.' }
    $reply = Send-Request @{action='cancel'}
    if ($reply['helper.state'] -ne 'idle') { throw 'Cancel did not revoke selection.' }
    Send-Request @{action='close'} $false
    if (-not $process.WaitForExit(8000) -or $process.ExitCode -ne 0) { throw 'Helper did not close normally.' }
    if ($FreshProfile) {
        if (Test-Path -LiteralPath $store) { throw 'Preview created the real profile store.' }
    } elseif ((Get-FileHash -LiteralPath (Join-Path $store 'CURRENT') -Algorithm SHA256).Hash -ne $initialHash) { throw 'Read-only preview modified store.' }
    $result = @{state='passed'; fixture_root=$fixtureRoot; helper_sha256=$HelperSha256; initializer_sha256=$InitializerSha256;
        checks=@('handshake','local_signed_catalog','clear_selection','explicit_selection','stale_approval_refused','cancel','normal_close','store_unchanged');
        fresh_profile=[bool]$FreshProfile; network_used=$false; live_profile_modified=$false; temporary_fixture_retained=$true}
    $stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew)
    try { $bytes = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 4)); $stream.Write($bytes,0,$bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
    Write-Output 'NATIVE_INSTALLER_HELPER_PREVIEW_PASSED'
} finally {
    if (-not $process.HasExited) { $process.Kill(); Write-Output 'Only the fixture helper process was stopped; fixture retained for inspection.' }
}
