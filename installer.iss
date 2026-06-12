[Setup]
AppName=Qexow CAM
AppVersion=2.1.12
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

[Components]
Name: "daemon"; Description: "CAM Daemon Service & CLI"; Types: full custom; Flags: fixed
Name: "tray"; Description: "System Tray GUI & Shortcuts"; Types: full custom

[Files]
; The ONE executable — cam.exe is a Node.js SEA containing all logic
Source: "dist\cam.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: daemon
Source: "dist\qexow-cam-gui.exe"; DestDir: "{app}"; Flags: ignoreversion; Components: tray
Source: "src\query_threads.py"; DestDir: "{app}"; Flags: ignoreversion; Components: tray

[Icons]
; Start Menu: opens the single user-facing Windows GUI.
Name: "{group}\Qexow CAM"; Filename: "{app}\qexow-cam-gui.exe"; Components: tray
Name: "{group}\Uninstall Qexow CAM"; Filename: "{uninstallexe}"

[Registry]
; Add install dir to PATH so `cam` works from any terminal
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}')) and IsAdminInstallMode
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}')) and not IsAdminInstallMode
; Launch tray on Windows startup (user-level)
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Qexow CAM"; ValueData: """{app}\qexow-cam-gui.exe"""; Components: tray
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
Type: files; Name: "{userstartup}\CodexAgentManager.cmd"
Type: files; Name: "{userstartup}\QexowCam.cmd"
Type: files; Name: "{userstartup}\Codex Agent Manager.cmd"
Type: files; Name: "{localappdata}\Qexow CAM\cam.exe"
Type: files; Name: "{localappdata}\Qexow CAM\qexow-tray-proof.exe"
Type: files; Name: "{localappdata}\Qexow CAM\qexow-cam-gui.exe"
Type: files; Name: "{localappdata}\Qexow CAM\cam-bundle.cjs"
Type: files; Name: "{localappdata}\Qexow CAM\daemon-entry.js"
Type: files; Name: "{localappdata}\Qexow CAM\cam-tray.exe"
Type: files; Name: "{localappdata}\Qexow CAM\tray_windows_release.exe"
Type: dirifempty; Name: "{localappdata}\Qexow CAM"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\cam.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\qexow-tray-proof.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\qexow-cam-gui.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\cam-bundle.cjs"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\daemon-entry.js"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\cam-tray.exe"
Type: files; Name: "{localappdata}\Programs\Codex Agent Manager\tray_windows_release.exe"
Type: dirifempty; Name: "{localappdata}\Programs\Codex Agent Manager"

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

[Run]
; Record local startup metadata. This does not create scheduled tasks or shell scripts.
Filename: "{app}\cam.exe"; Parameters: "install-service"; StatusMsg: "Configuring startup service..."; Components: daemon; Flags: runhidden; Check: not IsHeadlessInstall
; After install: start tray (which auto-starts the daemon) — no cmd windows
Filename: "{app}\qexow-cam-gui.exe"; Description: "Launch Qexow CAM GUI"; Flags: nowait; Components: tray; Check: not IsHeadlessInstall

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

procedure KillLegacyNodeDaemon();
var
  ResultCode: Integer;
begin
  Exec('powershell.exe',
    '-NoProfile -ExecutionPolicy Bypass -Command "Get-CimInstance Win32_Process | Where-Object { $_.Name -eq ''node.exe'' -and $_.CommandLine -like ''*Qexow CAM*daemon-entry.js*'' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure DeleteIfExists(PathName: string);
begin
  if FileExists(PathName) then begin
    DeleteFile(PathName);
  end;
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
  KillProcess('qexow-tray-proof.exe');
  KillProcess('qexow-cam-gui.exe');
  KillProcess('cam.exe');
  KillProcess('cam-core.exe');
  KillProcess('cam-tray.exe');
  KillProcess('tray_windows_release.exe');
  KillLegacyNodeDaemon();

  // Remove old task/startup launch points so only the current tray command starts.
  DeleteScheduledTask('CodexAgentManager');
  DeleteScheduledTask('Codex Agent Manager');
  DeleteScheduledTask('QexowCam');
  DeleteIfExists(ExpandConstant('{userstartup}\CodexAgentManager.cmd'));
  DeleteIfExists(ExpandConstant('{userstartup}\QexowCam.cmd'));
  DeleteIfExists(ExpandConstant('{userstartup}\Codex Agent Manager.cmd'));

  Result := True;
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
