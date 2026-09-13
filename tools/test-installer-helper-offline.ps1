param(
    [Parameter(Mandatory=$true)][string]$Helper,
    [Parameter(Mandatory=$true)][string]$HelperSha256,
    [Parameter(Mandatory=$true)][string]$Initializer,
    [Parameter(Mandatory=$true)][string]$InitializerSha256,
    [Parameter(Mandatory=$true)][string]$Catalog,
    [Parameter(Mandatory=$true)][string]$PackagesDirectory,
    [Parameter(Mandatory=$true)][string]$Receipt,
    [string]$PackageId = 'ru-RU',
    [string]$Asset = 'ru-RU-r1.aklp'
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Receipt already exists.' }
foreach ($item in @(@($Helper, $HelperSha256), @($Initializer, $InitializerSha256))) {
    if ($item[1] -notmatch '^[a-f0-9]{64}$' -or (Get-FileHash -LiteralPath $item[0] -Algorithm SHA256).Hash -ne $item[1]) { throw 'Executable hash mismatch.' }
}
# Emulate the setup-side leases: the real installer acquires and HOLDS both
# singleton mutexes before the package download. The helper requires them to be
# owned by another thread, so this fixture owns them for the session.
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
    $info.FileName = $Path
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
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
function Wait-Idle($Reply) {
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    while ($Reply['helper.busy'] -eq '1') {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Worker timeout.' }
        Start-Sleep -Milliseconds 25
        $Reply = Send-Request @{action='poll'}
    }
    return $Reply
}

$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-helper-offline-' + [guid]::NewGuid().ToString('N'))
$catalogDir = Join-Path $fixtureRoot 'catalog'
$binDir = Join-Path $fixtureRoot 'bin'
$store = Join-Path $fixtureRoot 'packages'
New-Item -ItemType Directory -Path $catalogDir | Out-Null
New-Item -ItemType Directory -Path $binDir | Out-Null
Copy-Item -LiteralPath $Catalog -Destination (Join-Path $catalogDir 'catalog.aklc')
Get-ChildItem -LiteralPath $PackagesDirectory -Filter '*.aklp' | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $catalogDir $_.Name)
}
$localCatalog = Join-Path $catalogDir 'catalog.aklc'
$localHelper = Join-Path $binDir 'installer-package-helper.exe'
$localInitializer = Join-Path $binDir 'initialize_package_store.exe'
Copy-Item -LiteralPath $Helper -Destination $localHelper
Copy-Item -LiteralPath $Initializer -Destination $localInitializer

$initializerProcess = Start-Native $localInitializer ('"' + $store + '"')
if (-not $initializerProcess.WaitForExit(8000) -or $initializerProcess.ExitCode -ne 0) { throw 'Fixture initialization failed.' }
$initialCurrent = (Get-FileHash -LiteralPath (Join-Path $store 'CURRENT') -Algorithm SHA256).Hash
$packagePath = Join-Path $catalogDir $Asset
if (-not (Test-Path -LiteralPath $packagePath)) { throw 'Selected package asset missing.' }
$packageBlob = 'blob-' + (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash.ToLowerInvariant() + '.aklp'

$script:taskIdentity = [guid]::NewGuid().ToString('N')
$script:taskRequestNumber = 1
$script:taskSessionDirectory = Join-Path $fixtureRoot 'session'
$process = Start-Native $localHelper ([string]$PID + ' "' + $script:taskSessionDirectory + '" "' + $store + '" ' + $script:taskIdentity)
try {
    Wait-File (Join-Path $script:taskSessionDirectory 'reply-0.ini')
    $reply = Send-Request @{action='check_catalog';local_file=$localCatalog}
    $reply = Wait-Idle $reply
    if ($reply['helper.state'] -ne 'selecting' -or $reply['catalog.count'] -ne '13') { throw 'Local catalog page mismatch.' }
    $view = [long]$reply['helper.view']
    $reply = Send-Request @{action='select';view=$view;ids=@($PackageId)}
    if ($reply['helper.operation'] -ne 'ok' -or $reply['catalog.selected'] -ne '1') { throw 'Selection failed.' }
    $view = [long]$reply['helper.view']
    $reply = Send-Request @{action='confirm_download';view=$view}
    $reply = Wait-Idle $reply
    if ($reply['helper.result'] -ne 'ok' -or $reply['helper.state'] -ne 'reviewing') { throw ('Local download failed: ' + $reply['helper.result'] + '/' + $reply['helper.reason']) }
    $view = [long]$reply['helper.view']
    $reply = Send-Request @{action='confirm_install';view=$view}
    $reply = Wait-Idle $reply
    if ($reply['helper.result'] -ne 'ok' -or $reply['helper.state'] -ne 'idle') { throw ('Local install failed: ' + $reply['helper.result'] + '/' + $reply['helper.reason']) }
    Send-Request @{action='close'} $false
    if (-not $process.WaitForExit(8000) -or $process.ExitCode -ne 0) { throw 'Helper did not close normally.' }
    $currentNow = (Get-FileHash -LiteralPath (Join-Path $store 'CURRENT') -Algorithm SHA256).Hash
    if ($currentNow -eq $initialCurrent) { throw 'Install did not change the store pointer.' }
    if (-not (Test-Path -LiteralPath (Join-Path $store $packageBlob))) { throw 'Installed package blob missing.' }
    $result = @{state='passed'; fixture_root=$fixtureRoot; helper_sha256=$HelperSha256; initializer_sha256=$InitializerSha256;
        checks=@('setup_leases_owned','local_signed_catalog','explicit_selection','local_artifact_no_network','download_prepared','install_committed','store_pointer_advanced','package_blob_present','normal_close');
        package_id=$PackageId; asset=$Asset; network_used=$false; live_profile_modified=$false; temporary_fixture_retained=$true}
    $stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew)
    try { $bytes = [Text.Encoding]::UTF8.GetBytes(($result | ConvertTo-Json -Depth 4)); $stream.Write($bytes,0,$bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
    Write-Output 'NATIVE_INSTALLER_HELPER_OFFLINE_INSTALL_PASSED'
} finally {
    if (-not $process.HasExited) { $process.Kill(); Write-Output 'Only the fixture helper process was stopped; fixture retained for inspection.' }
    if ($agentLease -ne [IntPtr]::Zero) { [void][AutoKeyboardLayotFixture.Leases]::CloseHandle($agentLease) }
    if ($settingsLease -ne [IntPtr]::Zero) { [void][AutoKeyboardLayotFixture.Leases]::CloseHandle($settingsLease) }
}
