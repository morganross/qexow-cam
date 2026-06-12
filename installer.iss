[Setup]
AppName=Qexow CAM
AppVersion=2.1.3
DefaultDirName={pf}\Qexow CAM
DefaultGroupName=Qexow CAM
OutputDir=dist
OutputBaseFilename=QexowCamSetup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
ChangesEnvironment=yes
SetupIconFile=compiler:SetupClassicIcon.ico
CloseApplications=no


[Files]
; The ONE executable — cam.exe is a Node.js SEA containing all logic
Source: "dist\cam.exe"; DestDir: "{app}"; Flags: ignoreversion
; Systray helper binary (required for tray icon on Windows)
Source: "dist\tray_windows_release.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Start Menu: opens status window (signals running instance, or launches fresh)
Name: "{group}\Qexow CAM"; Filename: "{app}\cam.exe"; Parameters: "tray"
Name: "{group}\Uninstall Qexow CAM"; Filename: "{uninstallexe}"

[Registry]
; Add install dir to PATH so `cam` works from any terminal
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))
; Launch tray on Windows startup (user-level)
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Qexow CAM"; ValueData: """{app}\cam.exe"" tray"

[UninstallRun]
; Kill tray and daemon cleanly before uninstall
Filename: "taskkill"; Parameters: "/F /IM cam.exe /T"; Flags: runhidden; RunOnceId: "KillCam"
Filename: "taskkill"; Parameters: "/F /IM tray_windows_release.exe /T"; Flags: runhidden; RunOnceId: "KillTray"
Filename: "{app}\cam.exe"; Parameters: "uninstall-service"; Flags: runhidden; RunOnceId: "UninstallService"

[Run]
; After install: start tray (which auto-starts the daemon) — no cmd windows
Filename: "{app}\cam.exe"; Parameters: "tray"; Description: "Launch Qexow CAM"; Flags: postinstall nowait

[Code]
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OrigPath)
  then begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Param + ';', ';' + OrigPath + ';') = 0;
end;

function InitializeSetup(): Boolean;
var
  ResultCode: Integer;
begin
  // Kill any running instances to free file locks before installing
  Exec('taskkill.exe', '/F /IM cam.exe /T', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec('taskkill.exe', '/F /IM tray_windows_release.exe /T', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  // Remove any old cam-core.exe references (cleanup from old installs)
  Exec('taskkill.exe', '/F /IM cam-core.exe /T', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  // Remove old scheduled tasks
  Exec('schtasks.exe', '/Delete /TN QexowCam /F', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := True;
end;
