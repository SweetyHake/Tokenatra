; Tokenatra Inno Setup script
; Install Inno Setup from https://jrsoftware.org/isdl.php first
; Build: cargo build --release (in rust/) -> rust\target\release\tokenatra.exe

#define MyAppName "Tokenatra"
#ifndef MyAppVersion
; Fallback: CI передаёт версию через /DMyAppVersion (from rust/src/consts.rs VERSION)
#define MyAppVersion "26.5.1"
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
Name: "registermcp"; Description: "Register MCP server for AI agents (Claude, Cursor, Antigravity, OpenCode)"; GroupDescription: "AI integration:"

[Files]
; Один нативный бинарник + MCP-прокси + ресурсы. Никакого Python-окружения.
Source: "rust\target\release\tokenatra.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "rust\target\release\TokenatraMCP.exe"; DestDir: "{app}"; DestName: "TokenatraMCP.exe"; Flags: ignoreversion
Source: "static\*"; DestDir: "{app}\static"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "templates\*"; DestDir: "{app}\templates"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "token_rings\*"; DestDir: "{app}\token_rings"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "presets\*"; DestDir: "{app}\presets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "logo.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "mask.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "example.png"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; хвосты старой Python-сборки (PyInstaller)
Type: filesandordirs; Name: "{app}\_internal"

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

// Миграция с Python-установки: config.json и модели жили в {app},
// Конфиг и модели хранятся в %APPDATA%\Tokenatra. Переносим один раз.
procedure MigrateFromPython;
var
  appDir, dataDir, srcFile, dstFile: String;
  FindRec: TFindRec;
begin
  appDir := ExpandConstant('{app}');
  dataDir := ExpandConstant('{userappdata}\Tokenatra');
  ForceDirectories(dataDir);

  if (FileExists(appDir + '\config.json')) and (not FileExists(dataDir + '\config.json')) then
    CopyFile(appDir + '\config.json', dataDir + '\config.json', False);

  if DirExists(appDir + '\models') then
  begin
    ForceDirectories(dataDir + '\models');
    if FindFirst(appDir + '\models\*.onnx', FindRec) then
    try
      repeat
        srcFile := appDir + '\models\' + FindRec.Name;
        dstFile := dataDir + '\models\' + FindRec.Name;
        if not FileExists(dstFile) then
          CopyFile(srcFile, dstFile, False);
      until not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    MigrateFromPython;
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
Filename: "{app}\TokenatraMCP.exe"; Parameters: "--install-mcp"; Tasks: registermcp; Flags: runhidden waituntilterminated
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
