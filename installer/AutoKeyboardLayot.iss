; Incremental installer source, not a release-ready multilingual installer.
; Required build inputs must be supplied explicitly with /D definitions.
#ifndef AppExecutable
  #error Supply AppExecutable pointing to the reviewed Windows executable
#endif
#ifndef BundleNotices
  #error Supply BundleNotices with notices for all bundled dependencies and data
#endif
#ifndef AppVersion
  #error Supply AppVersion matching the reviewed binary
#endif
#ifndef PackageHelper
  #error Supply PackageHelper pointing to the reviewed installer-package-helper executable
#endif

[Setup]
#ifdef InstallerUiProbe
AppId={{6F8B6B8A-B02B-4CF2-96C2-3360C6A2FE24}
AppName=AutoKeyboardLayot UI probe - installation disabled
#else
AppId={{D913907B-2031-4C26-A199-BF60B0E51B6D}
AppName=AutoKeyboardLayot
#endif
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\AutoKeyboardLayot
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupMutex=Local\AutoKeyboardLayot.Setup
CloseApplications=no
RestartApplications=no
DisableProgramGroupPage=yes
UsePreviousTasks=yes
OutputDir=output
#ifdef InstallerUiProbe
OutputBaseFilename=AutoKeyboardLayot-{#AppVersion}-ui-probe-NOT-FOR-INSTALLATION
#else
OutputBaseFilename=AutoKeyboardLayot-{#AppVersion}-setup-experimental
#endif
UninstallDisplayIcon={app}\AutoKeyboardLayot.exe
SetupIconFile={#SourcePath}\..\assets\app.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
LanguageDetectionMethod=uilanguage
UsePreviousLanguage=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl,messages\en.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl,messages\en.isl,messages\ru.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl,messages\en.isl,messages\de.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl,messages\en.isl,messages\es.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl,messages\en.isl,messages\fr.isl"
Name: "portuguese_br"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl,messages\en.isl,messages\pt-BR.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl,messages\en.isl,messages\ja.isl"
Name: "arabic"; MessagesFile: "compiler:Languages\Arabic.isl,messages\en.isl,messages\ar.isl"
Name: "chinese_simplified"; MessagesFile: "vendor\inno\ChineseSimplified.isl,messages\en.isl,messages\zh-CN.isl"
Name: "indonesian"; MessagesFile: "vendor\inno\Indonesian.isl,messages\en.isl,messages\id.isl"
Name: "urdu"; MessagesFile: "vendor\inno\Urdu.isl,messages\en.isl,messages\ur.isl"
Name: "estonian"; MessagesFile: "vendor\inno\Estonian.isl,messages\en.isl,messages\et.isl"
Name: "hindi"; MessagesFile: "vendor\inno\Hindi.isl,messages\en.isl,messages\hi.isl"
Name: "bengali"; MessagesFile: "vendor\inno\Bengali.isl,messages\en.isl,messages\bn.isl"

[Tasks]
Name: "startup"; Description: "{cm:AkStartupTask}"; Flags: unchecked

[Files]
Source: "{#PackageHelper}"; DestName: "installer-package-helper.exe"; Flags: dontcopy
Source: "{#AppExecutable}"; DestDir: "{app}"; DestName: "AutoKeyboardLayot.exe"; Flags: ignoreversion
Source: "{#BundleNotices}"; DestDir: "{app}"; DestName: "THIRD-PARTY-NOTICES.txt"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\AutoKeyboardLayot\AutoKeyboardLayot"; Filename: "{app}\AutoKeyboardLayot.exe"
Name: "{userprograms}\AutoKeyboardLayot\{cm:AkSettingsShortcut}"; Filename: "{app}\AutoKeyboardLayot.exe"; Parameters: "--settings"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "AutoKeyboardLayot"; ValueData: """{app}\AutoKeyboardLayot.exe"""; Tasks: startup; Flags: uninsdeletevalue

; No [Run], [InstallDelete], or [UninstallDelete]: setup never starts conversion
; and never enrolls the separate user-data directory for recursive deletion.

[Code]
const
  AgentMutexName = 'Local\AutoKeyboardLayot.Agent';
  SettingsMutexName = 'Local\AutoKeyboardLayot.Settings.Singleton';
  AlreadyExists = 183;
  SharingViolation = 32;
  NotModularBase = 23;
var
  AgentLease, SettingsLease, InstallationFence: THandle;

function NativeCreateMutex(Attributes, InitialOwner: LongWord; Name: String): THandle;
  external 'CreateMutexW@kernel32.dll stdcall';
function NativeCloseHandle(Handle: THandle): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';
function NativeReleaseMutex(Handle: THandle): Boolean;
  external 'ReleaseMutex@kernel32.dll stdcall';
function NativeCreateFence(Name: String; Access, Share, Security, Creation, Flags: LongWord; Template: THandle): THandle;
  external 'CreateFileW@kernel32.dll stdcall';
function NativeFenceSize(Handle: THandle; var HighSize: LongWord): LongWord;
  external 'GetFileSize@kernel32.dll stdcall';
function NativeFenceType(Handle: THandle): LongWord;
  external 'GetFileType@kernel32.dll stdcall';
function NativeFenceAttributes(Name: String): LongWord;
  external 'GetFileAttributesW@kernel32.dll stdcall';

function PrepareToInstall(var NeedsRestart: Boolean): String; forward;
#include "package-pages.iss"

procedure ReleaseLeases;
begin
  if SettingsLease <> 0 then begin
    NativeReleaseMutex(SettingsLease);
    NativeCloseHandle(SettingsLease);
    SettingsLease := 0;
  end;
  if AgentLease <> 0 then begin
    NativeReleaseMutex(AgentLease);
    NativeCloseHandle(AgentLease);
    AgentLease := 0;
  end;
  // Keep cross-session exclusion until the local instance leases are released.
  if InstallationFence <> 0 then begin
    NativeCloseHandle(InstallationFence);
    InstallationFence := 0;
  end;
end;

function InstallationFenceName: String;
begin
  Result := ExpandConstant('{localappdata}\AutoKeyboardLayot.installation.lock');
end;

// Returns '' once the fence is held, otherwise the reason. A sharing violation means
// another setup, uninstall, agent or settings of this user holds the file (possibly in
// another session); any other failure means the file cannot serve as the fence.
function AcquireInstallationFence: String;
var HighSize, Attributes, ErrorCode: LongWord;
begin
  Result := '';
  // OPEN_ALWAYS without truncation, GENERIC_READ|WRITE, no sharing. New agents
  // and settings retain read-only sharing handles for their complete lifetime.
  InstallationFence := NativeCreateFence(InstallationFenceName, $C0000000, 0, 0, 4, $00200080, 0);
  ErrorCode := DLLGetLastError;
  if InstallationFence = THandle($FFFFFFFF) then begin
    InstallationFence := 0;
    if ErrorCode = SharingViolation then
      Result := CustomMessage('AkFenceBusy')
    else
      Result := FmtMessage(CustomMessage('AkFenceInvalid'), [InstallationFenceName, IntToStr(ErrorCode)]);
    Exit;
  end;
  HighSize := 0;
  Attributes := NativeFenceAttributes(InstallationFenceName);
  if (InstallationFence = 0) or (NativeFenceType(InstallationFence) <> 1) or
      ((Attributes and $410) <> 0) or (NativeFenceSize(InstallationFence, HighSize) <> 0) or (HighSize <> 0) then begin
    if InstallationFence <> 0 then
      NativeCloseHandle(InstallationFence);
    InstallationFence := 0;
    Result := FmtMessage(CustomMessage('AkFenceInvalid'), [InstallationFenceName, '0']);
  end;
end;

function AcquireLease(Name: String; var Handle: THandle): Boolean;
var
  ErrorCode: LongWord;
begin
  Handle := NativeCreateMutex(0, 1, Name);
  ErrorCode := DLLGetLastError;
  Result := (Handle <> 0) and (ErrorCode <> AlreadyExists);
end;

function StopAndLease(Helper: String): String;
var
  ExitCode: Integer;
begin
  Result := '';
  if (AgentLease <> 0) and (SettingsLease <> 0) and (InstallationFence <> 0) then
    Exit;
  ReleaseLeases;
  // Without the installed program (quarantined, deleted, or an interrupted upgrade) nothing
  // can run from it, so there is nothing to close; the leases and the fence below still
  // keep a program from elsewhere and other setups out.
  if FileExists(Helper) then begin
    if not Exec(Helper, '--prepare-upgrade "' + ExpandConstant('{app}') + '"',
        '', SW_HIDE, ewWaitUntilTerminated, ExitCode) then begin
      Result := CustomMessage('AkCloseHelperFailed');
      Exit;
    end;
    if ExitCode <> 0 then begin
      Result := FmtMessage(CustomMessage('AkCloseBusy'), [IntToStr(ExitCode)]);
      Exit;
    end;
  end else
    Log('The installed program is missing; no running instance needs to be closed.');
  if not AcquireLease(AgentMutexName, AgentLease) then begin
    ReleaseLeases;
    Result := CustomMessage('AkAgentRestarted');
    Exit;
  end;
  if not AcquireLease(SettingsMutexName, SettingsLease) then begin
    ReleaseLeases;
    Result := CustomMessage('AkSettingsRestarted');
    Exit;
  end;
  Result := AcquireInstallationFence;
  if Result <> '' then
    ReleaseLeases;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Helper: String;
  ExitCode: Integer;
begin
#ifdef InstallerUiProbe
  // A native UI probe can never reach profile initialization or file replacement.
  Result := CustomMessage('AkPackagePending');
  Exit;
#endif
  ExtractTemporaryFile('AutoKeyboardLayot.exe');
  Helper := ExpandConstant('{tmp}\AutoKeyboardLayot.exe');
  if not Exec(Helper, '--verify-modular-base', '', SW_HIDE,
      ewWaitUntilTerminated, ExitCode) then begin
    Result := CustomMessage('AkBaseProbeFailed');
    Exit;
  end;
  if ExitCode = NotModularBase then begin
    Result := CustomMessage('AkNotModularBase');
    Exit;
  end;
  if ExitCode <> 0 then begin
    Result := FmtMessage(CustomMessage('AkBaseProbeCode'), [IntToStr(ExitCode)]);
    Exit;
  end;
  Result := StopAndLease(Helper);
  if Result <> '' then
    Exit;
  if not Exec(Helper, '--initialize-installation', '', SW_HIDE,
      ewWaitUntilTerminated, ExitCode) then
    Result := CustomMessage('AkProfileInitFailed')
  else if ExitCode = 22 then
    Result := CustomMessage('AkLegacyMigrationRequired')
  else if ExitCode <> 0 then
    Result := FmtMessage(CustomMessage('AkProfileReadFailed'), [IntToStr(ExitCode)]);
  if Result <> '' then
    ReleaseLeases;
end;

procedure DeinitializeSetup;
begin
  StopPackageHelper;
  ReleaseLeases;
end;

function InitializeUninstall: Boolean;
var
  ErrorText: String;
begin
  ErrorText := StopAndLease(ExpandConstant('{app}\AutoKeyboardLayot.exe'));
  Result := ErrorText = '';
  if not Result then
    MsgBox(ErrorText, mbError, MB_OK);
end;

procedure DeinitializeUninstall;
begin
  ReleaseLeases;
end;
