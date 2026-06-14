[Setup]
AppName=Qexow CAM
AppVersion=2.1.44
DefaultDirName={autopf}\Qexow CAM
DefaultGroupName=Qexow CAM
OutputDir=dist
OutputBaseFilename=QexowCamSetup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
SetupIconFile=compiler:SetupClassicIcon.ico
CloseApplications=force
RestartApplications=no
PrivilegesRequired=none
PrivilegesRequiredOverridesAllowed=dialog commandline

[Types]
Name: "full"; Description: "Full local installation (Recommended)"
Name: "custom"; Description: "Custom installation"; Flags: iscustom

[Tasks]
Name: "preserve_state"; Description: "Keep old CAM registry/state data (skip deleting old CAM registry/state data)"; Flags: unchecked

[Components]
Name: "daemon"; Description: "CAM Daemon Service & CLI"; Types: full custom; Flags: fixed
Name: "tray"; Description: "System Tray GUI & Shortcuts"; Types: full custom

[Files]
; The ONE executable — cam.exe is a Node.js SEA containing all logic
Source: "dist\cam.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: daemon
Source: "dist\qexow-cam-gui.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: tray
Source: "README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "boss.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "docs\howto-use-qexow-cam.md"; DestDir: "{app}\docs"; Flags: ignoreversion
Source: "docs\qexow-cam-plain-english.md"; DestDir: "{app}\docs"; Flags: ignoreversion

[Icons]
; Start Menu: opens the single user-facing Windows GUI.
Name: "{group}\Qexow CAM"; Filename: "{app}\qexow-cam-gui.exe"; Components: tray
Name: "{group}\Uninstall Qexow CAM"; Filename: "{uninstallexe}"

[Registry]
; Add install dir to PATH so `cam` works from any terminal
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}')) and IsAdminInstallMode
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}')) and not IsAdminInstallMode
; Launch tray on Windows startup (user-level)
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Qexow CAM"; ValueData: """{app}\qexow-cam-gui.exe"""; Components: tray; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Qexow CAM GUI"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Qexow CAM Tray Proof"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Codex Agent Manager"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Codex Agent Manager Tray"; Flags: deletevalue

[InstallDelete]
Type: files; Name: "{app}\cam-tray.exe"
Type: files; Name: "{app}\qexow-tray-proof.exe"
Type: files; Name: "{app}\qexow-cam-gui.exe"
Type: files; Name: "{app}\tray_windows_release.exe"
Type: files; Name: "{app}\cam-core.exe"
Type: files; Name: "{app}\cam-bundle.cjs"
Type: files; Name: "{app}\daemon-entry.js"
Type: files; Name: "{app}\query_threads.py"
Type: files; Name: "{userstartup}\CodexAgentManager.cmd"
Type: files; Name: "{userstartup}\QexowCam.cmd"
Type: files; Name: "{userstartup}\Codex Agent Manager.cmd"
Type: files; Name: "{localappdata}\Qexow CAM\cam.exe"
Type: files; Name: "{localappdata}\Qexow CAM\qexow-tray-proof.exe"
Type: files; Name: "{localappdata}\Qexow CAM\qexow-cam-gui.exe"
Type: files; Name: "{localappdata}\Qexow CAM\cam-bundle.cjs"
Type: files; Name: "{localappdata}\Qexow CAM\daemon-entry.js"
Type: files; Name: "{localappdata}\Qexow CAM\query_threads.py"
Type: files; Name: "{localappdata}\Qexow CAM\cam-tray.exe"
Type: files; Name: "{localappdata}\Qexow CAM\tray_windows_release.exe"
Type: dirifempty; Name: "{localappdata}\Qexow CAM"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\cam.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\qexow-tray-proof.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\qexow-cam-gui.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\cam-bundle.cjs"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\daemon-entry.js"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\query_threads.py"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\cam-tray.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\tray_windows_release.exe"
Type: dirifempty; Name: "{localappdata}\Programs\Codex Agent Manager"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\cam.exe"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\qexow-cam-gui.exe"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\qexow-tray-proof.exe"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\cam-bundle.cjs"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\daemon-entry.js"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\query_threads.py"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\cam-tray.exe"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\tray_windows_release.exe"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\install-remote.sh"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\unins000.exe"
Type: files; Name: "{localappdata}\Programs\Qexow CAM\unins000.dat"
Type: dirifempty; Name: "{localappdata}\Programs\Qexow CAM"

[UninstallRun]
Filename: "taskkill"; Parameters: "/F /T /IM cam-tray.exe"; Flags: runhidden; RunOnceId: "KillOldTray"
Filename: "taskkill"; Parameters: "/F /T /IM qexow-tray-proof.exe"; Flags: runhidden; RunOnceId: "KillTrayProof"
Filename: "taskkill"; Parameters: "/F /T /IM qexow-cam-gui.exe"; Flags: runhidden; RunOnceId: "KillGui"
Filename: "taskkill"; Parameters: "/F /T /IM tray_windows_release.exe"; Flags: runhidden; RunOnceId: "KillOldTrayRelease"
Filename: "taskkill"; Parameters: "/F /T /IM cam-core.exe"; Flags: runhidden; RunOnceId: "KillOldCore"
Filename: "taskkill"; Parameters: "/F /T /IM cam.exe"; Flags: runhidden; RunOnceId: "KillCam"
Filename: "{app}\cam.exe"; Parameters: "uninstall-service"; Flags: runhidden; RunOnceId: "UninstallService"

[UninstallDelete]
Type: files; Name: "{userstartup}\CodexAgentManager.cmd"
Type: files; Name: "{userstartup}\QexowCam.cmd"
Type: files; Name: "{userstartup}\Codex Agent Manager.cmd"
Type: filesandordirs; Name: "{localappdata}\Programs\Qexow CAM"
Type: filesandordirs; Name: "{localappdata}\Programs\Codex Agent Manager"
Type: filesandordirs; Name: "{localappdata}\Qexow CAM"

[Code]
procedure KillProcess(ImageName: string);
var
  ResultCode: Integer;
begin
  Exec('taskkill.exe', '/F /T /IM ' + ImageName, '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure DeleteScheduledTask(TaskName: string);
var
  ResultCode: Integer;
begin
  Exec('schtasks.exe', '/Delete /TN "' + TaskName + '" /F', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure DeleteIfExists(PathName: string);
begin
  if FileExists(PathName) then begin
    DeleteFile(PathName);
  end;
end;

procedure DeleteValueIfExists(RootKey: Integer; const Subkey, ValueName: string);
begin
  if RegValueExists(RootKey, Subkey, ValueName) then begin
    RegDeleteValue(RootKey, Subkey, ValueName);
  end;
end;

procedure RemoveDirIfExists(PathName: string);
begin
  if DirExists(PathName) then begin
    DelTree(PathName, True, True, True);
  end;
end;

procedure DeleteLinkIfExists(PathName: string);
begin
  DeleteIfExists(PathName);
end;

function IsPathSeparator(CharValue: string): Boolean;
begin
  Result := (CharValue = '\') or (CharValue = '/');
end;

function NormalizePathForCompare(PathName: string): string;
begin
  Result := LowerCase(Trim(PathName));
  while (Length(Result) > 0) and IsPathSeparator(Copy(Result, Length(Result), 1)) do begin
    Delete(Result, Length(Result), 1);
  end;
end;

function PathEquals(const LeftValue, RightValue: string): Boolean;
begin
  Result := NormalizePathForCompare(LeftValue) = NormalizePathForCompare(RightValue);
end;

function ReadTrimmedFile(PathName: string): string;
var
  Value: AnsiString;
begin
  Result := '';
  if not FileExists(PathName) then begin
    exit;
  end;
  if LoadStringFromFile(PathName, Value) then begin
    Result := Trim(String(Value));
  end;
end;

function OverrideFilePath(FileName: string): string;
begin
  Result := ExpandConstant('{app}\' + FileName);
end;

function GetCamHomePath(): string;
begin
  Result := ExpandConstant('{param:CAMHOME|}');
  if Trim(Result) = '' then begin
    Result := GetEnv('CAM_HOME');
  end;
  if Trim(Result) = '' then begin
    Result := ReadTrimmedFile(OverrideFilePath('cam-home-override.txt'));
  end;
  if Trim(Result) = '' then begin
    Result := ExpandConstant('{%USERPROFILE}\.qexow-cam');
  end;
end;

function GetLegacyCamHomePath(): string;
begin
  Result := ExpandConstant('{param:LEGACYCAMHOME|}');
  if Trim(Result) = '' then begin
    Result := GetEnv('CAM_LEGACY_HOME');
  end;
  if Trim(Result) = '' then begin
    Result := ReadTrimmedFile(OverrideFilePath('legacy-cam-home-override.txt'));
  end;
  if Trim(Result) = '' then begin
    Result := ExpandConstant('{%USERPROFILE}\.codex-agent-manager');
  end;
end;

procedure PersistCleanupOverrides();
var
  CamHomeOverride: string;
  LegacyHomeOverride: string;
begin
  CamHomeOverride := Trim(ExpandConstant('{param:CAMHOME|}'));
  if CamHomeOverride = '' then begin
    CamHomeOverride := Trim(GetEnv('CAM_HOME'));
  end;
  LegacyHomeOverride := Trim(ExpandConstant('{param:LEGACYCAMHOME|}'));
  if LegacyHomeOverride = '' then begin
    LegacyHomeOverride := Trim(GetEnv('CAM_LEGACY_HOME'));
  end;

  if CamHomeOverride <> '' then begin
    SaveStringToFile(OverrideFilePath('cam-home-override.txt'), CamHomeOverride, False);
  end else begin
    DeleteIfExists(OverrideFilePath('cam-home-override.txt'));
  end;

  if LegacyHomeOverride <> '' then begin
    SaveStringToFile(OverrideFilePath('legacy-cam-home-override.txt'), LegacyHomeOverride, False);
  end else begin
    DeleteIfExists(OverrideFilePath('legacy-cam-home-override.txt'));
  end;
end;

function StripPathEntry(PathValue, EntryToRemove: string): string;
var
  RemainingValue: string;
  Segment: string;
  SeparatorPos: Integer;
  NextValue: string;
begin
  Result := '';
  if PathValue = '' then begin
    exit;
  end;
  RemainingValue := PathValue;
  NextValue := '';
  while True do begin
    SeparatorPos := Pos(';', RemainingValue);
    if SeparatorPos > 0 then begin
      Segment := Copy(RemainingValue, 1, SeparatorPos - 1);
      Delete(RemainingValue, 1, SeparatorPos);
    end else begin
      Segment := RemainingValue;
      RemainingValue := '';
    end;
    if Trim(Segment) <> '' then begin
      if not PathEquals(Segment, EntryToRemove) then begin
        if NextValue <> '' then begin
          NextValue := NextValue + ';';
        end;
        NextValue := NextValue + Segment;
      end;
    end;
    if RemainingValue = '' then begin
      break;
    end;
  end;
  Result := NextValue;
end;

procedure RemovePathEntryFromHive(RootKey: Integer; const Subkey, EntryToRemove: string);
var
  PathValue: string;
  UpdatedValue: string;
begin
  if not RegQueryStringValue(RootKey, Subkey, 'Path', PathValue) then begin
    exit;
  end;
  UpdatedValue := StripPathEntry(PathValue, EntryToRemove);
  if UpdatedValue <> PathValue then begin
    RegWriteExpandStringValue(RootKey, Subkey, 'Path', UpdatedValue);
  end;
end;

procedure RemoveKnownPathEntries();
begin
  RemovePathEntryFromHive(HKEY_CURRENT_USER, 'Environment', ExpandConstant('{app}'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{app}'));
  RemovePathEntryFromHive(HKEY_CURRENT_USER, 'Environment', ExpandConstant('{localappdata}\Programs\Qexow CAM'));
  RemovePathEntryFromHive(HKEY_CURRENT_USER, 'Environment', ExpandConstant('{localappdata}\Programs\Codex Agent Manager'));
  RemovePathEntryFromHive(HKEY_CURRENT_USER, 'Environment', ExpandConstant('{localappdata}\Qexow CAM'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{localappdata}\Programs\Qexow CAM'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{localappdata}\Programs\Codex Agent Manager'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{localappdata}\Qexow CAM'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{pf}\Qexow CAM'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{pf32}\Qexow CAM'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{pf}\Codex Agent Manager'));
  RemovePathEntryFromHive(HKEY_LOCAL_MACHINE, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment', ExpandConstant('{pf32}\Codex Agent Manager'));
end;

procedure ResetCamRuntimeStateForInstall();
var
  CamHome: string;
begin
  CamHome := GetCamHomePath();

  // Reinstall starts with a clean runtime map and message/test history.
  DeleteIfExists(CamHome + '\agents.json');
  DeleteIfExists(CamHome + '\mailbox.jsonl');
  DeleteIfExists(CamHome + '\events.jsonl');
  DeleteIfExists(CamHome + '\tests.jsonl');
  DeleteIfExists(CamHome + '\daemon.pid');
  DeleteIfExists(CamHome + '\daemon.json');
  DeleteIfExists(CamHome + '\tray.lock');
  DeleteIfExists(CamHome + '\service.json');
  RemoveDirIfExists(CamHome + '\logs');
end;

procedure ResetCamProcessMarkersOnly();
var
  CamHome: string;
  LegacyHome: string;
begin
  CamHome := GetCamHomePath();
  LegacyHome := GetLegacyCamHomePath();

  DeleteIfExists(CamHome + '\daemon.pid');
  DeleteIfExists(CamHome + '\daemon.json');
  DeleteIfExists(CamHome + '\tray.lock');
  DeleteIfExists(CamHome + '\service.json');
  DeleteIfExists(LegacyHome + '\daemon.pid');
  DeleteIfExists(LegacyHome + '\daemon.json');
  DeleteIfExists(LegacyHome + '\tray.lock');
  DeleteIfExists(LegacyHome + '\service.json');
end;

procedure RemoveLegacyCamLaunchPoints();
begin
  DeleteScheduledTask('CodexAgentManager');
  DeleteScheduledTask('Codex Agent Manager');
  DeleteScheduledTask('QexowCam');
  DeleteScheduledTask('Qexow CAM');

  DeleteIfExists(ExpandConstant('{userstartup}\CodexAgentManager.cmd'));
  DeleteIfExists(ExpandConstant('{userstartup}\QexowCam.cmd'));
  DeleteIfExists(ExpandConstant('{userstartup}\Codex Agent Manager.cmd'));
  DeleteLinkIfExists(ExpandConstant('{userstartup}\Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{commonstartup}\Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{userdesktop}\Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{commondesktop}\Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{userdesktop}\Codex Agent Manager.lnk'));
  DeleteLinkIfExists(ExpandConstant('{commondesktop}\Codex Agent Manager.lnk'));
  DeleteLinkIfExists(ExpandConstant('{group}\Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{group}\Uninstall Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{userprograms}\Qexow CAM\Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{userprograms}\Qexow CAM\Uninstall Qexow CAM.lnk'));
  DeleteLinkIfExists(ExpandConstant('{userprograms}\Codex Agent Manager\Codex Agent Manager.lnk'));
  DeleteLinkIfExists(ExpandConstant('{userprograms}\Codex Agent Manager\Uninstall Codex Agent Manager.lnk'));
  RemoveDirIfExists(ExpandConstant('{userprograms}\Qexow CAM'));
  RemoveDirIfExists(ExpandConstant('{userprograms}\Codex Agent Manager'));
  RemoveDirIfExists(ExpandConstant('{commonprograms}\Qexow CAM'));
  RemoveDirIfExists(ExpandConstant('{commonprograms}\Codex Agent Manager'));

  DeleteValueIfExists(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Qexow CAM');
  DeleteValueIfExists(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Qexow CAM GUI');
  DeleteValueIfExists(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Qexow CAM Tray Proof');
  DeleteValueIfExists(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Codex Agent Manager');
  DeleteValueIfExists(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Codex Agent Manager Tray');
  DeleteValueIfExists(HKEY_LOCAL_MACHINE, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Qexow CAM');
  DeleteValueIfExists(HKEY_LOCAL_MACHINE, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Qexow CAM GUI');
  DeleteValueIfExists(HKEY_LOCAL_MACHINE, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Qexow CAM Tray Proof');
  DeleteValueIfExists(HKEY_LOCAL_MACHINE, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Codex Agent Manager');
  DeleteValueIfExists(HKEY_LOCAL_MACHINE, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Codex Agent Manager Tray');

  RemoveKnownPathEntries();
end;

procedure RemoveKnownInstallRoots();
begin
  RemoveDirIfExists(ExpandConstant('{pf}\Qexow CAM'));
  RemoveDirIfExists(ExpandConstant('{localappdata}\Programs\Qexow CAM'));
  RemoveDirIfExists(ExpandConstant('{localappdata}\Programs\Codex Agent Manager'));
  RemoveDirIfExists(ExpandConstant('{localappdata}\Qexow CAM'));
  RemoveDirIfExists(ExpandConstant('{pf32}\Qexow CAM'));
  RemoveDirIfExists(ExpandConstant('{pf32}\Codex Agent Manager'));
  RemoveDirIfExists(ExpandConstant('{pf}\Codex Agent Manager'));
end;

procedure FullWipeCamHomes();
begin
  RemoveDirIfExists(GetCamHomePath());
  RemoveDirIfExists(GetLegacyCamHomePath());
end;

function ShouldPreservePriorState(): Boolean;
begin
  Result := WizardIsTaskSelected('preserve_state');
end;

procedure KillKnownCamProcesses();
begin
  KillProcess('qexow-tray-proof.exe');
  KillProcess('qexow-cam-gui.exe');
  KillProcess('cam.exe');
  KillProcess('cam-core.exe');
  KillProcess('cam-tray.exe');
  KillProcess('tray_windows_release.exe');
end;

function IsHeadlessInstall(): Boolean;
var
  i: Integer;
begin
  Result := False;
  for i := 1 to ParamCount do begin
    if CompareText(ParamStr(i), '/headless') = 0 then begin
      Result := True;
      exit;
    end;
  end;
end;

function InitializeSetup(): Boolean;
begin
  // Stop every known CAM executable name before files are replaced.
  KillKnownCamProcesses();

  Result := True;
end;

procedure LaunchInstalledCamIfNeeded();
var
  ResultCode: Integer;
begin
  // Headless means no visible GUI, not a dead install. Always record startup
  // metadata and start the correct runtime path after files land.
  Exec(ExpandConstant('{app}\cam.exe'), 'install-service', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  if IsHeadlessInstall() then begin
    Exec(ExpandConstant('{app}\cam.exe'), 'daemon launch --headless --wait-seconds 30', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end else begin
    Exec(ExpandConstant('{app}\qexow-cam-gui.exe'), '', '', SW_SHOWNORMAL, ewNoWait, ResultCode);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then begin
    RemoveLegacyCamLaunchPoints();
    RemoveKnownInstallRoots();

    if ShouldPreservePriorState() then begin
      ResetCamProcessMarkersOnly();
    end else begin
      FullWipeCamHomes();
    end;

    PersistCleanupOverrides();
    exit;
  end;
  if CurStep = ssPostInstall then begin
    LaunchInstalledCamIfNeeded();
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep <> usUninstall then begin
    exit;
  end;

  KillKnownCamProcesses();
  RemoveLegacyCamLaunchPoints();
  FullWipeCamHomes();
end;

function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
  Hive: Integer;
  Subkey: string;
begin
  if IsAdminInstallMode then begin
    Hive := HKEY_LOCAL_MACHINE;
    Subkey := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end else begin
    Hive := HKEY_CURRENT_USER;
    Subkey := 'Environment';
  end;
  if not RegQueryStringValue(Hive, Subkey, 'Path', OrigPath) then begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Param + ';', ';' + OrigPath + ';') = 0;
end;
