[Setup]
AppName=Qexow CAM
AppVersion=2.0.0
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
Source: "dist\cam.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Qexow CAM"; Filename: "{app}\cam.exe"
Name: "{group}\Uninstall Qexow CAM"; Filename: "{uninstallexe}"

[Registry]
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Qexow CAM"; ValueData: "{app}\cam.exe"

[UninstallRun]
Filename: "taskkill"; Parameters: "/F /IM cam.exe"; Flags: runhidden; RunOnceId: "KillCam"
Filename: "taskkill"; Parameters: "/F /IM cam-core.exe"; Flags: runhidden; RunOnceId: "KillCamCore"
Filename: "{app}\cam.exe"; Parameters: "uninstall-service"; Flags: runhidden; RunOnceId: "UninstallService"

[Run]
Filename: "{app}\cam.exe"; Parameters: "install-service"; Description: "Install background daemon service"; Flags: postinstall runhidden
Filename: "{app}\cam.exe"; Description: "Launch Qexow CAM Application"; Flags: postinstall nowait

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
  // Force kill any running instances to free file locks
  Exec('taskkill.exe', '/F /IM cam.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec('taskkill.exe', '/F /IM cam-core.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  
  // Try to remove the scheduled task if it exists so we can recreate it fresh
  Exec('schtasks.exe', '/Delete /TN QexowCam /F', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  
  Result := True;
end;
