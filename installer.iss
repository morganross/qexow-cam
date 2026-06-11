[Setup]
AppName=Codex Agent Manager
AppVersion=1.6.1
DefaultDirName={pf}\Codex Agent Manager
DefaultGroupName=Codex Agent Manager
OutputDir=dist
OutputBaseFilename=CodexAgentManagerSetup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
ChangesEnvironment=yes
SetupIconFile=compiler:SetupClassicIcon.ico
CloseApplications=no


[Files]
Source: "dist\cam.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\cam-tray.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\daemon-entry.js"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Codex Agent Manager"; Filename: "{app}\cam-tray.exe"
Name: "{group}\Uninstall Codex Agent Manager"; Filename: "{uninstallexe}"

[Registry]
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Codex Agent Manager Tray"; ValueData: "{app}\cam-tray.exe"

[UninstallRun]
Filename: "taskkill"; Parameters: "/F /IM cam-tray.exe"; Flags: runhidden; RunOnceId: "KillTray"
Filename: "taskkill"; Parameters: "/F /IM cam.exe"; Flags: runhidden; RunOnceId: "KillCam"
Filename: "{app}\cam.exe"; Parameters: "uninstall-service"; Flags: runhidden; RunOnceId: "UninstallService"

[Run]
Filename: "{app}\cam.exe"; Parameters: "install-service"; Description: "Install background daemon service"; Flags: postinstall runhidden
Filename: "{app}\cam-tray.exe"; Description: "Launch System Tray Icon"; Flags: postinstall nowait

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
  // Force kill any running tray and daemon instances to free file locks
  Exec('taskkill.exe', '/F /IM cam-tray.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec('taskkill.exe', '/F /IM cam.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  
  // Try to remove the scheduled task if it exists so we can recreate it fresh
  Exec('schtasks.exe', '/Delete /TN CodexAgentManager /F', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  
  Result := True;
end;

