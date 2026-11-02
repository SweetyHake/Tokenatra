; Tokenatra Inno Setup script (Rust edition)
; Install Inno Setup from https://jrsoftware.org/isdl.php first
; Build: cargo build --release (in rust/) -> rust\target\release\tokenatra.exe

#define MyAppName "Tokenatra"
#ifndef MyAppVersion
; Fallback: CI передаёт версию через /DMyAppVersion (from rust/src/consts.rs VERSION)
#define MyAppVersion "26.4.0"
#endif
#define MyAppPublisher "SweetyHake"
#define MyAppURL "https://github.com/SweetyHake/Tokenatra"
#define MyAppExeName "Tokenatra.exe"

[Setup]
AppId={{F0A1B2C3-D4E5-6789-ABCD-EF0123456789}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
UninstallDisplayName={#MyAppName}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=dist\installer
OutputBaseFilename=Tokenatra_v{#MyAppVersion}_windows_x86_64_setup
SetupIconFile=icon.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#MyAppExeName}
PrivilegesRequired=admin

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: checkedonce

[Files]
; Один нативный бинарник + ресурсы. Никакого Python-окружения.
Source: "rust\target\release\tokenatra.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "static\*"; DestDir: "{app}\static"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "templates\*"; DestDir: "{app}\templates"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "token_rings\*"; DestDir: "{app}\token_rings"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "presets\*"; DestDir: "{app}\presets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "logo.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "mask.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "example.png"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Code]
function IsSilentUpdate: Boolean;
var
  CmdTail: String;
begin
  CmdTail := Uppercase(GetCmdTail);
  Result := (Pos('/SILENT', CmdTail) > 0) or (Pos('/VERYSILENT', CmdTail) > 0);
end;

function InitializeUninstall(): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  // При деинсталляции молча убиваем запущенное приложение (0 = успешно).
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /T /IM {#MyAppExeName}',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  if ResultCode = 0 then
    Sleep(1000);
end;

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
