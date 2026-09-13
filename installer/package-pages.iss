// Stateful helper remains out of process. Timer ABI follows:
// https://jrsoftware.org/ishelp/topic_isxfunc_createcallback.htm
var
  PackagePage, PackageReviewPage: TWizardPage;
  PackageList: TNewCheckListBox;
  PackageStatus: TNewStaticText;
  PackageCheck, PackageLocal, PackageDownload, PackageCancel, PackageInstall: TNewButton;
  PackageReviewList: TNewComboBox;
  PackageLicense: TNewMemo;
  PackageIds, PackageReviewFiles: TArrayOfString;
  PackageBytes: array of Integer;
  PackageSeen: array of Boolean;
  PackageDir, PackageIdentity, PackagePhase, PackageQueued, PackagePendingAction: String;
  PackageSequence, PackageAwaiting, PackageView, PackageSerial: Integer;
  PackageStartTick: DWORD;
  PackageTimer: UINT_PTR;
  PackageProcess: THandle;
  PackageStarted, PackageReady, PackageBusy, PackageComplete, PackageAfterSelect,
    PackageInstalling, PackageInTimer, PackageCancelRequested, PackageFault: Boolean;

function PackageSetTimer(Wnd: HWND; Event: UINT_PTR; Elapse: UINT; Callback: LongWord): UINT_PTR;
  external 'SetTimer@user32.dll stdcall';
function PackageKillTimer(Wnd: HWND; Event: UINT_PTR): Boolean;
  external 'KillTimer@user32.dll stdcall';
function PackageCurrentPid: DWORD;
  external 'GetCurrentProcessId@kernel32.dll stdcall';
function PackageTicks: DWORD;
  external 'GetTickCount@kernel32.dll stdcall';
function PackageOpenProcess(Access, Inherit, Pid: DWORD): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function PackageWait(Handle: THandle; Milliseconds: DWORD): DWORD;
  external 'WaitForSingleObject@kernel32.dll stdcall';

function PackageCount: Integer;
var I: Integer;
begin
  Result := 0;
  for I := 0 to PackageList.Items.Count - 1 do
    if PackageList.Checked[I] then Inc(Result);
end;

function PackageTotal: Int64;
var I: Integer;
begin
  Result := 0;
  for I := 0 to PackageList.Items.Count - 1 do
    if PackageList.Checked[I] then Result := Result + PackageBytes[I];
end;

procedure PackageControls;
var Available, Seen: Boolean; I: Integer;
begin
  Available := not PackageFault and not PackageBusy and (PackageAwaiting < 0);
  PackageCheck.Enabled := Available;
  PackageLocal.Enabled := Available;
  PackageList.Enabled := Available and (PackagePhase = 'selecting');
  PackageDownload.Enabled := PackageList.Enabled and (PackageCount > 0);
  PackageCancel.Enabled := PackageStarted and (PackagePhase <> 'installing');
  Seen := Length(PackageSeen) > 0;
  for I := 0 to GetArrayLength(PackageSeen) - 1 do Seen := Seen and PackageSeen[I];
  PackageInstall.Enabled := Available and (PackagePhase = 'reviewing') and Seen;
end;

function PackageSend(Command, Action: String): Boolean;
var Text, Temp, Target: String;
begin
  Result := False;
  if not PackageReady or (PackageAwaiting >= 0) then Exit;
  Text := '{"format":1,"session":"' + PackageIdentity + '","sequence":' + IntToStr(PackageSequence) + ',"command":' + Command + '}';
  Target := PackageDir + '\request-' + IntToStr(PackageSequence) + '.json';
  Temp := Target + '.writing';
  if FileExists(Target) or FileExists(Temp) then Exit;
  if not SaveStringToFile(Temp, UTF8Encode(Text), False) then Exit;
  if not RenameFile(Temp, Target) then Exit;
  PackageAwaiting := PackageSequence;
  Inc(PackageSequence);
  PackagePendingAction := Action;
  Result := True;
  PackageControls;
end;

procedure PackageFailed;
begin
  PackageStatus.Caption := CustomMessage('AkPackageFailed');
  PackageAfterSelect := False;
  PackageInstalling := False;
  PackageComplete := False;
end;

procedure PackageListClick(Sender: TObject);
begin
  PackageStatus.Caption := FmtMessage(CustomMessage('AkPackageTotal'), [IntToStr(PackageCount), IntToStr(PackageTotal)]);
  PackageControls;
end;

procedure PackageReviewClick(Sender: TObject);
var I: Integer; Text: AnsiString;
begin
  I := PackageReviewList.ItemIndex;
  if (I < 0) or (I >= GetArrayLength(PackageReviewFiles)) then Exit;
  if LoadStringFromFile(PackageDir + '\' + PackageReviewFiles[I], Text) then begin
    PackageLicense.Text := UTF8Decode(Text);
    PackageSeen[I] := True;
  end else PackageFailed;
  PackageControls;
end;

procedure PackageReadReply(FileName: String);
var Count, I, Size: Integer; Section, Id, Caption, Name, ResultText, ErrorText: String; Enabled, Restart: Boolean;
begin
  if (GetIniString('helper', 'session', '', FileName) <> PackageIdentity) or
     (GetIniInt('helper', 'sequence', -1, -1, 4096, FileName) <> PackageAwaiting) then RaiseException('Package reply mismatch');
  PackageAwaiting := -1;
  PackageView := GetIniInt('helper', 'view', 0, 0, 100000, FileName);
  PackagePhase := GetIniString('helper', 'state', '', FileName);
  PackageBusy := GetIniString('helper', 'busy', '1', FileName) = '1';
  ResultText := GetIniString('helper', 'result', '', FileName);
  if (GetIniString('helper', 'operation', '', FileName) <> 'ok') or
     (ResultText = 'failed') or (ResultText = 'commit_uncertain') then begin
    PackageFailed;
    PackageControls;
    Exit;
  end;
  if PackagePhase = 'selecting' then begin
    Count := GetIniInt('catalog', 'count', -1, -1, 64, FileName);
    if Count < 0 then RaiseException('Package row count');
    PackageList.Items.Clear;
    SetArrayLength(PackageIds, Count);
    SetArrayLength(PackageBytes, Count);
    for I := 0 to Count - 1 do begin
      Section := 'package' + IntToStr(I);
      Id := GetIniString(Section, 'id', '', FileName);
      Size := GetIniInt(Section, 'bytes', -1, -1, 67108864, FileName);
      if (Id = '') or (Size < 1) then RaiseException('Invalid package row');
      PackageIds[I] := Id;
      PackageBytes[I] := Size;
      Caption := Id + ' — ' + FmtMessage(CustomMessage('AkPackageRow'), [GetIniString(Section, 'revision', '', FileName), IntToStr(Size), GetIniString(Section, 'ui_locale', '', FileName)]);
      if GetIniString(Section, 'input', '0', FileName) = '1' then Caption := Caption + ' ' + CustomMessage('AkPackageInput');
      Enabled := GetIniString(Section, 'compatible', '0', FileName) = '1';
      if not Enabled then Caption := Caption + ' ' + CustomMessage('AkPackageUnavailable');
      PackageList.AddCheckBox(Caption, '', 0, GetIniString(Section, 'selected', '0', FileName) = '1', Enabled, False, False, nil);
    end;
    PackageListClick(PackageList);
  end;
  if (PackagePhase = 'reviewing') and (Length(PackageReviewFiles) = 0) then begin
    Count := GetIniInt('review', 'count', -1, -1, 64, FileName);
    if Count < 1 then RaiseException('Missing review');
    SetArrayLength(PackageReviewFiles, Count);
    SetArrayLength(PackageSeen, Count);
    PackageReviewList.Items.Clear;
    for I := 0 to Count - 1 do begin
      Name := GetIniString('review', 'file' + IntToStr(I), '', FileName);
      if Name <> 'review-' + IntToStr(PackageView) + '-' + IntToStr(I) + '.txt' then RaiseException('Invalid review name');
      PackageReviewFiles[I] := Name;
      PackageSeen[I] := False;
      PackageReviewList.Items.Add(IntToStr(I + 1));
    end;
    PackageReviewList.ItemIndex := 0;
    PackageReviewClick(PackageReviewList);
    PackageStatus.Caption := FmtMessage(CustomMessage('AkPackageReview'), [IntToStr(PackageCount), IntToStr(PackageTotal)]);
  end;
  if PackageAfterSelect and (PackagePhase = 'selecting') then begin
    PackageAfterSelect := False;
    if MsgBox(FmtMessage(CustomMessage('AkPackageTotal'), [IntToStr(PackageCount), IntToStr(PackageTotal)]), mbConfirmation, MB_YESNO) = IDYES then begin
      Restart := False;
      ErrorText := PrepareToInstall(Restart);
      if ErrorText <> '' then MsgBox(ErrorText, mbError, MB_OK)
      else if not PackageSend('{"action":"confirm_download","view":' + IntToStr(PackageView) + '}', 'download') then PackageFailed;
    end;
  end;
  if PackageInstalling and not PackageBusy and (PackagePhase = 'idle') and (ResultText = 'ok') then begin
    PackageInstalling := False;
    PackageComplete := True;
    PackageStatus.Caption := CustomMessage('AkPackageDone');
    PackageLicense.Text := CustomMessage('AkPackageDone');
  end;
  if PackagePendingAction = 'cancel' then begin
    PackageList.Items.Clear;
    SetArrayLength(PackageReviewFiles, 0);
    SetArrayLength(PackageSeen, 0);
    PackageComplete := False;
    PackageAfterSelect := False;
    PackageLicense.Clear;
  end;
  if PackageBusy then PackageStatus.Caption := CustomMessage('AkPackageBusy');
  PackageControls;
end;

procedure PackageTimerProc(Wnd: HWND; Msg: UINT; Event: UINT_PTR; Time: DWORD);
var FileName: String; Pid: Integer;
begin
  if PackageInTimer or not PackageStarted then Exit;
  PackageInTimer := True;
  try
   try
    if (PackageProcess <> 0) and (PackageWait(PackageProcess, 0) <> 258) then begin
      NativeCloseHandle(PackageProcess); PackageProcess := 0;
      PackageBusy := False; PackageAwaiting := -1; PackageStarted := False; PackageReady := False;
      PackageFailed; PackageControls; Exit;
    end;
    if not PackageReady then begin
      FileName := PackageDir + '\reply-0.ini';
      if not FileExists(FileName) then begin
        if PackageTicks - PackageStartTick > 15000 then begin
          PackageFault := True; PackageStarted := False; PackageAwaiting := -1;
          PackageFailed; PackageControls;
        end;
        Exit;
      end;
      if GetIniString('helper', 'session', '', FileName) <> PackageIdentity then RaiseException('Helper session mismatch');
      Pid := GetIniInt('helper', 'pid', 0, 0, 2147483647, FileName);
      PackageProcess := PackageOpenProcess($100000, 0, Pid);
      if PackageProcess = 0 then RaiseException('Helper process unavailable');
      PackageReady := True; PackageAwaiting := -1;
      if not PackageSend(PackageQueued, 'check') then PackageFailed;
      Exit;
    end;
    if PackageAwaiting >= 0 then begin
      FileName := PackageDir + '\reply-' + IntToStr(PackageAwaiting) + '.ini';
      if FileExists(FileName) then PackageReadReply(FileName);
    end;
    if PackageCancelRequested and (PackageAwaiting < 0) and (PackagePhase <> 'installing') then begin
      PackageCancelRequested := False;
      PackageSend('{"action":"cancel"}', 'cancel');
    end else if PackageBusy and (PackageAwaiting < 0) then PackageSend('{"action":"poll"}', 'poll');
  except
    PackageFault := True;
    PackageFailed;
    PackageControls;
   end;
  finally
    PackageInTimer := False;
  end;
end;

procedure PackageStart(Command: String);
var Code: Integer; StoreRoot: String;
begin
  PackageComplete := False;
  SetArrayLength(PackageReviewFiles, 0); SetArrayLength(PackageSeen, 0);
  if PackageReady then begin PackageSend(Command, 'check'); Exit; end;
  Inc(PackageSerial);
  PackageDir := ExpandConstant('{tmp}') + '\package-session-' + IntToStr(PackageSerial);
  PackageIdentity := Copy(GetSHA256OfString(PackageDir), 1, 32);
  PackageSequence := 1; PackageAwaiting := 0; PackageQueued := Command;
#ifdef InstallerUiProbe
  StoreRoot := ExpandConstant('{tmp}\probe-profile\packages');
#else
  StoreRoot := ExpandConstant('{localappdata}\AutoKeyboardLayot\packages');
#endif
  ExtractTemporaryFile('installer-package-helper.exe');
  if not Exec(ExpandConstant('{tmp}\installer-package-helper.exe'), IntToStr(PackageCurrentPid) + ' "' + PackageDir + '" "' + StoreRoot + '" ' + PackageIdentity, '', SW_HIDE, ewNoWait, Code) then begin PackageAwaiting := -1; PackageFailed; Exit; end;
  PackageStarted := True;
  PackageStartTick := PackageTicks;
  PackageControls;
end;

procedure PackageCheckClick(Sender: TObject);
begin PackageStart('{"action":"check_catalog"}'); end;

procedure PackageLocalClick(Sender: TObject);
var FileName: String;
begin
  FileName := '';
  if GetOpenFileName(CustomMessage('AkPackageLocal'), FileName, '', 'Catalog (*.aklc)|*.aklc', 'aklc') then begin
    StringChangeEx(FileName, '\', '\\', True);
    StringChangeEx(FileName, '"', '\"', True);
    PackageStart('{"action":"check_catalog","local_file":"' + FileName + '"}');
  end;
end;

procedure PackageDownloadClick(Sender: TObject);
var I: Integer; Ids: String;
begin
  Ids := '';
  for I := 0 to PackageList.Items.Count - 1 do if PackageList.Checked[I] then begin
    if Ids <> '' then Ids := Ids + ',';
    Ids := Ids + '"' + PackageIds[I] + '"';
  end;
  PackageAfterSelect := True;
  if not PackageSend('{"action":"select","view":' + IntToStr(PackageView) + ',"ids":[' + Ids + ']}', 'select') then begin PackageAfterSelect := False; PackageFailed; end;
end;

procedure PackageCancelClick(Sender: TObject);
begin
  PackageCancelRequested := True;
end;

procedure PackageInstallClick(Sender: TObject);
begin
  if MsgBox(FmtMessage(CustomMessage('AkPackageReview'), [IntToStr(PackageCount), IntToStr(PackageTotal)]), mbConfirmation, MB_YESNO) <> IDYES then Exit;
  PackageInstalling := PackageSend('{"action":"confirm_install","view":' + IntToStr(PackageView) + '}', 'install');
end;

function PackageButton(Page: TWizardPage; Caption: String; Left, Top, Width: Integer; Click: TNotifyEvent): TNewButton;
begin
  Result := TNewButton.Create(WizardForm); Result.Parent := Page.Surface;
  Result.SetBounds(Left, Top, Width, ScaleY(25)); Result.Caption := Caption; Result.OnClick := Click;
end;

procedure InitializeWizard;
var Width: Integer;
begin
  PackageAwaiting := -1;
  PackagePage := CreateCustomPage(wpSelectTasks, CustomMessage('AkPackageTitle'), CustomMessage('AkPackageIntro'));
  PackageReviewPage := CreateCustomPage(PackagePage.ID, CustomMessage('AkPackageInstall'), CustomMessage('AkPackageIntro'));
  Width := (PackagePage.SurfaceWidth - ScaleX(12)) div 3;
  PackageCheck := PackageButton(PackagePage, CustomMessage('AkPackageCheck'), 0, 0, Width, @PackageCheckClick);
  PackageLocal := PackageButton(PackagePage, CustomMessage('AkPackageLocal'), Width + ScaleX(6), 0, Width, @PackageLocalClick);
  PackageDownload := PackageButton(PackagePage, CustomMessage('AkPackageDownload'), 2 * (Width + ScaleX(6)), 0, Width, @PackageDownloadClick);
  PackageList := TNewCheckListBox.Create(WizardForm); PackageList.Parent := PackagePage.Surface;
  PackageList.SetBounds(0, ScaleY(32), PackagePage.SurfaceWidth, PackagePage.SurfaceHeight - ScaleY(105));
  PackageList.OnClickCheck := @PackageListClick;
  PackageStatus := TNewStaticText.Create(WizardForm); PackageStatus.Parent := PackagePage.Surface;
  PackageStatus.SetBounds(0, PackagePage.SurfaceHeight - ScaleY(67), PackagePage.SurfaceWidth, ScaleY(65));
  PackageStatus.AutoSize := False; PackageStatus.WordWrap := True; PackageStatus.Caption := CustomMessage('AkPackageIntro');
  PackageReviewList := TNewComboBox.Create(WizardForm); PackageReviewList.Parent := PackageReviewPage.Surface;
  PackageReviewList.SetBounds(0, 0, PackageReviewPage.SurfaceWidth, ScaleY(24)); PackageReviewList.Style := csDropDownList;
  PackageReviewList.OnChange := @PackageReviewClick;
  PackageLicense := TNewMemo.Create(WizardForm); PackageLicense.Parent := PackageReviewPage.Surface;
  PackageLicense.SetBounds(0, ScaleY(30), PackageReviewPage.SurfaceWidth, PackageReviewPage.SurfaceHeight - ScaleY(65));
  PackageLicense.ReadOnly := True; PackageLicense.ScrollBars := ssVertical;
  PackageInstall := PackageButton(PackageReviewPage, CustomMessage('AkPackageInstall'), 0, PackageReviewPage.SurfaceHeight - ScaleY(28), PackageReviewPage.SurfaceWidth div 2 - ScaleX(4), @PackageInstallClick);
  PackageCancel := PackageButton(PackageReviewPage, CustomMessage('AkPackageCancel'), PackageReviewPage.SurfaceWidth div 2 + ScaleX(4), PackageReviewPage.SurfaceHeight - ScaleY(28), PackageReviewPage.SurfaceWidth div 2 - ScaleX(4), @PackageCancelClick);
  PackageControls;
  PackageTimer := PackageSetTimer(0, 0, 200, CreateCallback(@PackageTimerProc));
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := not PackageFault and not PackageBusy and (PackageAwaiting < 0);
  if (CurPageID = PackagePage.ID) and (PackageCount > 0) and not PackageComplete then Result := Result and (PackagePhase = 'reviewing');
  if (CurPageID = PackageReviewPage.ID) and (PackageCount > 0) then Result := Result and PackageComplete;
  if not Result then MsgBox(CustomMessage('AkPackagePending'), mbInformation, MB_OK);
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin Result := (PageID = PackageReviewPage.ID) and (PackageCount = 0); end;

procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  if PackageBusy or (PackageAwaiting >= 0) then begin
    Cancel := False; Confirm := False;
    if PackagePhase <> 'installing' then PackageCancelRequested := True;
    PackageStatus.Caption := CustomMessage('AkPackagePending');
  end;
end;

procedure StopPackageHelper;
var WaitResult: DWORD;
begin
  if PackageTimer <> 0 then PackageKillTimer(0, PackageTimer);
  if PackageReady and (PackageAwaiting < 0) then PackageSend('{"action":"close"}', 'close');
  if PackageProcess <> 0 then begin
    // Never release the installer leases merely because a timeout elapsed.
    // Ordinary UI closure is admitted only after busy/pending work has ended;
    // exceptional teardown must still observe the exact process terminating.
    repeat
      WaitResult := PackageWait(PackageProcess, 250);
      if WaitResult = 258 then WizardForm.Refresh;
    until WaitResult <> 258;
    if WaitResult <> 0 then begin
      Log('Package helper termination could not be verified');
      RaiseException(CustomMessage('AkPackageFailed'));
    end;
    NativeCloseHandle(PackageProcess); PackageProcess := 0;
  end;
end;
