[Setup]
AppName=Codex Agent Manager
AppVersion=0.1.0
DefaultDirName={pf}\Codex Agent Manager
DefaultGroupName=Codex Agent Manager
OutputDir=dist
OutputBaseFilename=CodexAgentManagerSetup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
ChangesEnvironment=yes
SetupIconFile=compiler:SetupClassicIcon.ico

[Files]
Source: "dist\cam.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "dist\codex-send.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Uninstall Codex Agent Manager"; Filename: "{uninstallexe}"

[Registry]
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\cam.exe"; Parameters: "install-service"; Description: "Install background daemon service"; Flags: postinstall runhidden

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
