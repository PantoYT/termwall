; termwall installer (Inno Setup 6). Built by installer\build.ps1, which stages the files and
; an embeddable Python with psutil into build\stage first. Per-user install, no admin rights:
; %LOCALAPPDATA%\Programs\termwall. Silent installs (winget) work with /VERYSILENT.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6F1D2C0B-8E54-4C47-9B6A-3C2E7D9F4A11}
AppName=termwall
AppVersion={#AppVersion}
AppVerName=termwall {#AppVersion}
AppPublisher=PantoYT
AppPublisherURL=https://github.com/PantoYT/termwall
AppSupportURL=https://github.com/PantoYT/termwall/issues
AppUpdatesURL=https://github.com/PantoYT/termwall/releases
DefaultDirName={autopf}\termwall
DisableDirPage=auto
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE
OutputDir=..\build
OutputBaseFilename=termwall-setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=termwall
UninstallDisplayIcon={app}\python\pythonw.exe
CloseApplications=force
ChangesEnvironment=yes
RestartApplications=no

[Files]
Source: "..\build\stage\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Registry]
; the stats server at logon, no console window
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "termwall"; \
  ValueData: """{app}\python\pythonw.exe"" ""{app}\termwall_api.py"""; Flags: uninsdeletevalue

[Run]
; Wallpaper Engine lists termwall from a junction in its projects folder (skipped if WE isn't there)
Filename: "{app}\python\python.exe"; Parameters: """{app}\termwall_api.py"" --link-we"; Flags: runhidden waituntilterminated
; and Lively Wallpaper's library (skipped if Lively isn't there)
Filename: "{app}\python\python.exe"; Parameters: """{app}\termwall_api.py"" --link-lively"; Flags: runhidden waituntilterminated
Filename: "{app}\python\pythonw.exe"; Parameters: """{app}\termwall_api.py"""; Flags: runhidden nowait

[UninstallRun]
Filename: "{app}\python\python.exe"; Parameters: """{app}\termwall_api.py"" --stop"; Flags: runhidden waituntilterminated; RunOnceId: "stop"
Filename: "{app}\python\python.exe"; Parameters: """{app}\termwall_api.py"" --unlink-we"; Flags: runhidden waituntilterminated; RunOnceId: "unlinkwe"
Filename: "{app}\python\python.exe"; Parameters: """{app}\termwall_api.py"" --unlink-lively"; Flags: runhidden waituntilterminated; RunOnceId: "unlinklively"

[UninstallDelete]
Type: files; Name: "{app}\token.js"
Type: files; Name: "{app}\theme.json"
Type: files; Name: "{app}\termwall.json"
Type: files; Name: "{app}\termwall.json.old"
Type: files; Name: "{app}\termwall.toml"
Type: files; Name: "{app}\share.json"
Type: filesandordirs; Name: "{app}\__pycache__"

[Code]
const
  EnvKey = 'Environment';

// `termwall` in new terminals: {app} on the user's PATH (termwall.cmd lives there)
procedure PathAdd(Dir: string);
var
  Paths: string;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Paths) then Paths := '';
  if Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0 then exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then Paths := Paths + ';';
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Paths + Dir);
end;

procedure PathRemove(Dir: string);
var
  Paths: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Paths) then exit;
  Paths := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if P = 0 then exit;
  Delete(Paths, P, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2);
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then PathAdd(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then PathRemove(ExpandConstant('{app}'));
end;

// An update replaces python.exe and friends: stop the running server of this install first.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Result := '';
  if FileExists(ExpandConstant('{app}\python\python.exe')) then
    Exec(ExpandConstant('{app}\python\python.exe'), '"' + ExpandConstant('{app}\termwall_api.py') + '" --stop',
         '', SW_HIDE, ewWaitUntilTerminated, Code);
end;
