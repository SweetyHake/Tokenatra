; Tokenatra Inno Setup script
; Install Inno Setup from https://jrsoftware.org/isdl.php first

#define MyAppName "Tokenatra"
#ifndef MyAppVersion
; Fallback для локальной сборки без /DMyAppVersion; CI передаёт версию из version.py
#define MyAppVersion "26.1.6"
#endif
#define MyAppPublisher "SweetyHake"
#define MyAppURL "https://github.com/SweetyHake/Tokenatra"
#define MyAppExeName "Tokenatra.exe"

[Setup]
AppId={{F0A1B2C3-D4E5-6789-ABCD-EF0123456789}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
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
Source: "dist\Tokenatra\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Code]
function IsPythonInstalled: Boolean;
var
  ResultCode: Integer;
begin
  Result := Exec('cmd.exe', '/C python --version', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) and (ResultCode = 0);
end;

function IsSilentUpdate: Boolean;
var
  CmdTail: String;
begin
  CmdTail := Uppercase(GetCmdTail);
  Result := (Pos('/SILENT', CmdTail) > 0) or (Pos('/VERYSILENT', CmdTail) > 0);
end;

[Run]
; При тихой установке (/SILENT — автообновление из приложения) pip-шаги пропускаются:
; пакеты не нужны замороженному exe, а тянуть их на каждом апдейте долго.
Filename: "{cmd}"; Parameters: "/C python -m pip install numpy Pillow flask pywebview psutil imageio-ffmpeg --quiet"; StatusMsg: "Installing Python packages..."; Flags: runhidden; Check: IsPythonInstalled and not IsSilentUpdate
Filename: "{cmd}"; Parameters: "/C python -m pip install onnxruntime-directml --quiet || python -m pip install onnxruntime --quiet"; StatusMsg: "Installing AI backend..."; Flags: runhidden; Check: IsPythonInstalled and not IsSilentUpdate
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
