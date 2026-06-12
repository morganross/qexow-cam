[Setup]
AppName=Qexow CAM
AppVersion=2.1.5
DefaultDirName={autopf}\Qexow CAM
DefaultGroupName=Qexow CAM
OutputDir=dist
OutputBaseFilename=QexowCamSetup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
ChangesEnvironment=yes
SetupIconFile=compiler:SetupClassicIcon.ico
CloseApplications=no
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

[Icons]
; Start Menu: opens status window (signals running instance, or launches fresh)
Name: "{group}\Qexow CAM"; Filename: "{app}\cam.exe"; Parameters: "tray"; Components: tray
Name: "{group}\Uninstall Qexow CAM"; Filename: "{uninstallexe}"

[Registry]
; Add install dir to PATH so `cam` works from any terminal
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}')) and IsAdminInstallMode
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}')) and not IsAdminInstallMode
; Launch tray on Windows startup (user-level)
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Qexow CAM"; ValueData: """{app}\cam.exe"" tray"; Components: tray

[UninstallRun]
Filename: "{app}\cam.exe"; Parameters: "uninstall-service"; Flags: runhidden; RunOnceId: "UninstallService"

[Run]
; Record local startup metadata. This does not create scheduled tasks or shell scripts.
Filename: "{app}\cam.exe"; Parameters: "install-service"; StatusMsg: "Configuring startup service..."; Components: daemon; Flags: runhidden; Check: not IsHeadlessInstall
; After install: start tray (which auto-starts the daemon) — no cmd windows
Filename: "{app}\cam.exe"; Parameters: "tray"; Description: "Launch Qexow CAM System Tray"; Flags: postinstall nowait; Components: tray; Check: not IsHeadlessInstall

[Code]
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
