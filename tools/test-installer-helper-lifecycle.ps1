param(
    [Parameter(Mandatory=$true)][string]$Helper,
    [Parameter(Mandatory=$true)][string]$HelperSha256,
    [Parameter(Mandatory=$true)][string]$Initializer,
    [Parameter(Mandatory=$true)][string]$InitializerSha256,
    [Parameter(Mandatory=$true)][string]$Catalog,
    [Parameter(Mandatory=$true)][string]$PackagesDirectory,
    [Parameter(Mandatory=$true)][string]$Receipt
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
foreach ($item in @(@($Helper, $HelperSha256), @($Initializer, $InitializerSha256))) {
    if ($item[1] -notmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath $item[0] -Algorithm SHA256).Hash -ne $item[1]) { throw 'Executable hash mismatch.' }
}
Add-Type -Namespace AutoKeyboardLayotFixture -Name Leases -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
public static extern System.IntPtr CreateMutex(System.IntPtr attributes, bool initialOwner, string name);
[DllImport("kernel32.dll", SetLastError=true)]
public static extern bool CloseHandle(System.IntPtr handle);
'@
$agentLease = [AutoKeyboardLayotFixture.Leases]::CreateMutex([IntPtr]::Zero, $true, 'Local\AutoKeyboardLayot.Agent')
$settingsLease = [AutoKeyboardLayotFixture.Leases]::CreateMutex([IntPtr]::Zero, $true, 'Local\AutoKeyboardLayot.Settings.Singleton')

function Start-Native([string]$Path, [string]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Path; $info.Arguments = $Arguments
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    return [Diagnostics.Process]::Start($info)
}
function Wait-File([string]$Path, [int]$Seconds = 8) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while (-not (Test-Path -LiteralPath $Path)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw ('Reply timeout: ' + $Path) }
        Start-Sleep -Milliseconds 20
    }
}
function Read-Ini([string]$Path) {
    $values = @{}; $section = ''
    foreach ($line in [IO.File]::ReadAllLines($Path)) {
        if ($line -match '^\[([^\]]+)\]$') { $section = $matches[1] }
        elseif ($line -match '^([^=]+)=(.*)$') { $values[$section + '.' + $matches[1]] = $matches[2] }
    }
    return $values
}
function Send-Request($Command, [bool]$Wait = $true) {
    $number = $script:taskRequestNumber; $script:taskRequestNumber++
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
function Wait-Idle($Reply) {
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    while ($Reply['helper.busy'] -eq '1') {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Worker timeout.' }
        Start-Sleep -Milliseconds 25
        $Reply = Send-Request @{action='poll'}
    }
    return $Reply
}
function Start-Catalog {
    $reply = Send-Request @{action='check_catalog';local_file=$script:localCatalog}
    return Wait-Idle $reply
}
function BlobName([string]$Path) { return 'blob-' + (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() + '.aklp' }

$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-helper-lifecycle-' + [guid]::NewGuid().ToString('N'))
$catalogDir = Join-Path $fixtureRoot 'catalog'; $binDir = Join-Path $fixtureRoot 'bin'; $store = Join-Path $fixtureRoot 'packages'
New-Item -ItemType Directory -Path $catalogDir | Out-Null
New-Item -ItemType Directory -Path $binDir | Out-Null
Copy-Item -LiteralPath $Catalog -Destination (Join-Path $catalogDir 'catalog.aklc')
Get-ChildItem -LiteralPath $PackagesDirectory -Filter '*.aklp' | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $catalogDir $_.Name) }
$script:localCatalog = Join-Path $catalogDir 'catalog.aklc'
$localHelper = Join-Path $binDir 'installer-package-helper.exe'; $localInitializer = Join-Path $binDir 'initialize_package_store.exe'
Copy-Item -LiteralPath $Helper -Destination $localHelper
Copy-Item -LiteralPath $Initializer -Destination $localInitializer
$init = Start-Native $localInitializer ('"' + $store + '"')
if (-not $init.WaitForExit(8000) -or $init.ExitCode -ne 0) { throw 'Fixture initialization failed.' }
$initialCurrent = (Get-FileHash -LiteralPath (Join-Path $store 'CURRENT') -Algorithm SHA256).Hash
$ruAsset = Join-Path $catalogDir 'ru-RU-r1.aklp'; $etAsset = Join-Path $catalogDir 'et-EE-r1.aklp'; $frAsset = Join-Path $catalogDir 'fr-FR-r1.aklp'; $deAsset = Join-Path $catalogDir 'de-DE-r1.aklp'
$ruBlob = BlobName $ruAsset; $etBlob = BlobName $etAsset; $frBlob = BlobName $frAsset; $deBlob = BlobName $deAsset
$result = [ordered]@{state='failed'; fixture_root=$fixtureRoot; helper_sha256=$HelperSha256; initializer_sha256=$InitializerSha256; checks=[ordered]@{}; network_used=$false; live_profile_modified=$false}
$script:taskIdentity = [guid]::NewGuid().ToString('N'); $script:taskRequestNumber = 1
$script:taskSessionDirectory = Join-Path $fixtureRoot 'session'
$process = Start-Native $localHelper ([string]$PID + ' "' + $script:taskSessionDirectory + '" "' + $store + '" ' + $script:taskIdentity)
try {
    Wait-File (Join-Path $script:taskSessionDirectory 'reply-0.ini')
    # 1. Zero selection: a download confirmation with no package is a no-op.
    $r = Start-Catalog
    if ($r['helper.state'] -ne 'selecting' -or $r['catalog.count'] -ne '13') { throw 'Catalog page mismatch.' }
    $view = [long]$r['helper.view']
    $r = Send-Request @{action='confirm_download';view=$view}
    $result.checks.zero_selection_no_transaction = ($r['helper.operation'] -eq 'ok') -and ((Get-FileHash -LiteralPath (Join-Path $store 'CURRENT') -Algorithm SHA256).Hash -eq $initialCurrent)
    if (-not $result.checks.zero_selection_no_transaction) { throw 'Zero selection changed the store.' }
    # 2. Multiple selection downloads and installs both local artifacts.
    $r = Start-Catalog; $view = [long]$r['helper.view']
    $r = Send-Request @{action='select';view=$view;ids=@('ru-RU','et-EE')}
    if ($r['helper.operation'] -ne 'ok' -or $r['catalog.selected'] -ne '2') { throw 'Multiple selection failed.' }
    $view = [long]$r['helper.view']
    $r = Send-Request @{action='confirm_download';view=$view}; $r = Wait-Idle $r
    if ($r['helper.result'] -ne 'ok' -or $r['helper.state'] -ne 'reviewing') { throw ('Multiple download failed: ' + $r['helper.result'] + '/' + $r['helper.reason']) }
    $view = [long]$r['helper.view']
    $r = Send-Request @{action='confirm_install';view=$view}; $r = Wait-Idle $r
    $result.checks.multiple_install = ($r['helper.result'] -eq 'ok') -and ($r['helper.state'] -eq 'idle') -and
        (Test-Path -LiteralPath (Join-Path $store $ruBlob)) -and (Test-Path -LiteralPath (Join-Path $store $etBlob)) -and
        (-not (Test-Path -LiteralPath (Join-Path $store $deBlob)))
    if (-not $result.checks.multiple_install) { throw 'Multiple install did not commit exactly the selected packages.' }
    # 3. Cancel revokes the selection without any install.
    $r = Start-Catalog; $view = [long]$r['helper.view']
    $r = Send-Request @{action='select';view=$view;ids=@('de-DE')}
    $view = [long]$r['helper.view']
    $r = Send-Request @{action='cancel'}
    $result.checks.cancel_revokes = ($r['helper.state'] -eq 'idle')
    $r = Send-Request @{action='confirm_install';view=$view}
    $result.checks.cancel_revokes = $result.checks.cancel_revokes -and ($r['helper.operation'] -eq 'rejected') -and (-not (Test-Path -LiteralPath (Join-Path $store $deBlob)))
    if (-not $result.checks.cancel_revokes) { throw 'Cancel did not revoke the selection.' }
    # 4. A tampered local artifact fails with a specific reason and leaves no blob.
    $frOriginal = [IO.File]::ReadAllBytes($frAsset)
    [IO.File]::WriteAllBytes($frAsset, ($frOriginal + [byte]0))
    $r = Start-Catalog; $view = [long]$r['helper.view']
    $r = Send-Request @{action='select';view=$view;ids=@('fr-FR')}
    $view = [long]$r['helper.view']
    $r = Send-Request @{action='confirm_download';view=$view}; $r = Wait-Idle $r
    $result.checks.tampered_download = ($r['helper.result'] -eq 'failed') -and ($r['helper.reason'] -eq 'verification') -and ($r['helper.state'] -eq 'idle') -and (-not (Test-Path -LiteralPath (Join-Path $store $frBlob)))
    if (-not $result.checks.tampered_download) { throw ('Tampered local artifact was not rejected as expected: ' + $r['helper.result'] + '/' + $r['helper.reason'] + '/' + $r['helper.state']) }
    # 5. Retry after restoring the artifact succeeds and commits only FR.
    [IO.File]::WriteAllBytes($frAsset, $frOriginal)
    $r = Start-Catalog; $view = [long]$r['helper.view']
    $r = Send-Request @{action='select';view=$view;ids=@('fr-FR')}
    $view = [long]$r['helper.view']
    $r = Send-Request @{action='confirm_download';view=$view}; $r = Wait-Idle $r
    if ($r['helper.result'] -ne 'ok' -or $r['helper.state'] -ne 'reviewing') { throw ('Retry download failed: ' + $r['helper.result'] + '/' + $r['helper.reason']) }
    $view = [long]$r['helper.view']
    $r = Send-Request @{action='confirm_install';view=$view}; $r = Wait-Idle $r
    $result.checks.retry_install = ($r['helper.result'] -eq 'ok') -and ($r['helper.state'] -eq 'idle') -and (Test-Path -LiteralPath (Join-Path $store $frBlob))
    if (-not $result.checks.retry_install) { throw 'Retry install did not commit the restored package.' }
    Send-Request @{action='close'} $false
    if (-not $process.WaitForExit(8000) -or $process.ExitCode -ne 0) { throw 'Helper did not close normally.' }
    $result.state = 'passed'
    $stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew)
    try { $bytes = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 6)); $stream.Write($bytes,0,$bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
    Write-Output 'NATIVE_INSTALLER_HELPER_LIFECYCLE_PASSED'
} finally {
    if (-not $process.HasExited) { $process.Kill(); Write-Output 'Only the fixture helper process was stopped; fixture retained for inspection.' }
    if ($agentLease -ne [IntPtr]::Zero) { [void][AutoKeyboardLayotFixture.Leases]::CloseHandle($agentLease) }
    if ($settingsLease -ne [IntPtr]::Zero) { [void][AutoKeyboardLayotFixture.Leases]::CloseHandle($settingsLease) }
}
