; Bopogram installer (Inno Setup 6). Built by build.ps1:
;   ISCC /DOut=<folder> [/DPack=<bopogram-models-*.zip>] bopogram.iss
; With Pack the model pack goes into the installer (the "full" installer,
; for a first install); without it the user adds a pack in the settings
; window. Nothing here uses the network.
;
; What it does (as crates\install\install.ps1 did):
;   - stops the user's engine host (it saves the personal model first)
;   - copies the programs to Program Files\Bopogram (the 32-bit DLL in x86\);
;     files that running applications have loaded are renamed and deleted
;     at the next restart, so new applications get the new version at once
;   - registers the text service (regsvr32, 64- and 32-bit)
;   - writes bopogram-host.json: the engine's configuration is each user's
;     %LOCALAPPDATA%\Bopogram\data\models\engine.json (bopogram pack)
;   - as the signed-in user: copies the personal data of an older install
;     (a runtime bundle's userdata\), installs the model pack, adds the
;     keyboard to Traditional Chinese (Taiwan), starts the host
;   - Start menu shortcut to the settings window; starts the host at sign-in
; Uninstalling reverses it and asks whether to delete the user's data.

#define AppVersion "0.2.0"
#ifndef Out
  #define Out "."
#endif
#ifndef Pack
  #define Pack ""
#endif
#if Pack != ""
  #define Suffix "-full"
#else
  #define Suffix ""
#endif
#define Release "..\target\release"
#define Release32 "..\target\i686-pc-windows-msvc\release"

[Setup]
AppId={{6C1D0F5E-3B7A-4E52-9A8C-2D4F1B7E9A31}
AppName=Bopogram
AppVersion={#AppVersion}
AppVerName=Bopogram {#AppVersion}
AppPublisher=Bopogram
AppPublisherURL=https://github.com/aqua0801/bopogram
VersionInfoVersion={#AppVersion}
; The DLL is loaded into every application, elevated ones too: it must be
; in a folder only administrators can write to.
DefaultDirName={autopf}\Bopogram
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#Out}
OutputBaseFilename=bopogram-setup-{#AppVersion}{#Suffix}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\bopogram-ime\res\bopogram.ico
UninstallDisplayIcon={app}\bopogram.ico
UninstallDisplayName=Bopogram 注音輸入法
LicenseFile=..\..\LICENSE
; We stop the host ourselves; other applications keep the DLL they loaded.
CloseApplications=no
RestartIfNeededByRun=no
; HKCU is touched only to remove what a script install left for the same user,
; and the user data question on uninstall; both are meant for the user running setup.
UsedUserAreasWarning=no

[Languages]
Name: "cht"; MessagesFile: "ChineseTraditional.isl"

[Messages]
cht.WelcomeLabel2=這會在電腦上安裝 Bopogram 注音輸入法 [name/ver]。%n%n輸入法完全在本機執行，沒有任何網路功能。

[Files]
Source: "{#Release}\bopogram_ime.dll"; DestDir: "{app}"; Flags: ignoreversion regserver uninsrestartdelete
Source: "{#Release32}\bopogram_ime.dll"; DestDir: "{app}\x86"; Flags: ignoreversion regserver 32bit uninsrestartdelete
Source: "{#Release}\bopogram-host.exe"; DestDir: "{app}"; Flags: ignoreversion uninsrestartdelete
Source: "{#Release}\bopogram-settings.exe"; DestDir: "{app}"; Flags: ignoreversion uninsrestartdelete
Source: "{#Release}\bopogram.exe"; DestDir: "{app}"; Flags: ignoreversion uninsrestartdelete
Source: "..\bopogram-ime\res\bopogram.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "keyboard.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\target\THIRD-PARTY-LICENSES.html"; DestDir: "{app}"; Flags: ignoreversion
#if Pack != ""
Source: "{#Pack}"; DestDir: "{tmp}"; DestName: "models.zip"; Flags: deleteafterinstall nocompression
#endif

[InstallDelete]
; Left by the script installs (crates\install\) and by earlier updates.
Type: files; Name: "{app}\uninstall.ps1"
Type: files; Name: "{app}\switch-engine.ps1"
Type: files; Name: "{app}\common.ps1"
Type: files; Name: "{app}\*.old-*"
Type: files; Name: "{app}\x86\*.old-*"

[Icons]
Name: "{autoprograms}\Bopogram 設定"; Filename: "{app}\bopogram-settings.exe"; WorkingDir: "{app}"; IconFilename: "{app}\bopogram.ico"

[Registry]
; The host starts at every user's sign-in, so that the engine is ready before
; the first key (the input method is installed for all users).
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Bopogram"; ValueData: """{app}\bopogram-host.exe"""; Flags: uninsdeletevalue
; A script install started it from the user's own Run key.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Bopogram"; Flags: deletevalue dontcreatekey

[Run]
Filename: "{app}\bopogram-settings.exe"; Description: "開啟 Bopogram 設定"; Flags: postinstall runasoriginaluser nowait skipifsilent unchecked

[UninstallRun]
Filename: "{app}\bopogram-host.exe"; Parameters: "--stop"; Flags: runhidden waituntilterminated; RunOnceId: "StopHost"
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\keyboard.ps1"" remove"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveKeyboard"

[UninstallDelete]
Type: files; Name: "{app}\bopogram-host.json"
Type: files; Name: "{app}\*.old-*"
Type: files; Name: "{app}\x86\*.old-*"
Type: dirifempty; Name: "{app}\x86"
Type: dirifempty; Name: "{app}"

[Code]
const
  MOVEFILE_DELAY_UNTIL_REBOOT = 4;

function MoveFileEx(Existing: String; Replacement: String; Flags: Cardinal): Boolean;
  external 'MoveFileExW@kernel32.dll stdcall';

var
  OldUserdata: String;

{ Ask this user's host to stop (it saves the personal model) before files are replaced. }
procedure StopHost();
var
  Code: Integer;
begin
  if FileExists(ExpandConstant('{app}\bopogram-host.exe')) then
    Exec(ExpandConstant('{app}\bopogram-host.exe'), '--stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

{ A file that running applications have loaded cannot be replaced, but it can
  be renamed: the old copy goes at the next restart, and the new one is used
  by everything started from now on. }
procedure MoveAsideIfLocked(Path: String);
var
  Old: String;
begin
  if FileExists(Path) and not DeleteFile(Path) then begin
    Old := Path + '.old-' + GetDateTimeString('yyyymmddhhnnss', #0, #0);
    if RenameFile(Path, Old) then
      MoveFileEx(Old, '', MOVEFILE_DELAY_UNTIL_REBOOT);
  end;
end;

{ The "config" of the bopogram-host.json an older install wrote, if any. }
function OldConfig(): String;
var
  Text: AnsiString;
  S: String;
  P: Integer;
begin
  Result := '';
  if not LoadStringFromFile(ExpandConstant('{app}\bopogram-host.json'), Text) then exit;
  S := String(Text);
  P := Pos('"config"', S);
  if P = 0 then exit;
  Delete(S, 1, P + Length('"config"') - 1);
  P := Pos('"', S);
  if P = 0 then exit;
  Delete(S, 1, P);
  P := Pos('"', S);
  if P = 0 then exit;
  S := Copy(S, 1, P - 1);
  StringChangeEx(S, '\\', '\', True);
  Result := S;
end;

procedure AfterFiles(); forward;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Config: String;
begin
  if CurStep = ssPostInstall then AfterFiles();
  if CurStep = ssInstall then begin
    StopHost();
    { A script install's runtime bundle: its personal data moves to the new data folder. }
    Config := OldConfig();
    OldUserdata := '';
    if (Config <> '') and (CompareText(ExtractFileName(Config), 'config.json') = 0) then
      if DirExists(ExtractFileDir(Config) + '\userdata') then
        OldUserdata := ExtractFileDir(Config) + '\userdata';
    MoveAsideIfLocked(ExpandConstant('{app}\bopogram_ime.dll'));
    MoveAsideIfLocked(ExpandConstant('{app}\x86\bopogram_ime.dll'));
    MoveAsideIfLocked(ExpandConstant('{app}\bopogram-host.exe'));
    MoveAsideIfLocked(ExpandConstant('{app}\bopogram-settings.exe'));
    MoveAsideIfLocked(ExpandConstant('{app}\bopogram.exe'));
  end;
end;

{ Run a program as the signed-in user (not elevated), hidden; its exit code. }
function RunAsUser(Exe, Params, Status: String): Integer;
begin
  WizardForm.StatusLabel.Caption := Status;
  if not ExecAsOriginalUser(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, Result) then
    Result := -1;
end;

{ After the files: the configuration, then what is done as the signed-in user. }
procedure AfterFiles();
var
  Code: Integer;
  Engine: String;
begin
  Engine := ExpandConstant('{app}\bopogram.exe');
  SaveStringToFile(ExpandConstant('{app}\bopogram-host.json'),
    '{"engine": "rust", "config": "%LOCALAPPDATA%\\Bopogram\\data\\models\\engine.json"}', False);
  if OldUserdata <> '' then
    RunAsUser(Engine, 'pack adopt-userdata "' + OldUserdata + '"', '正在移轉個人資料…');
#if Pack != ""
  Code := RunAsUser(Engine, 'pack install "' + ExpandConstant('{tmp}\models.zip') + '"', '正在安裝模型（檢查、建立字典）…');
  if Code <> 0 then
    MsgBox('模型沒有安裝成功。請在「Bopogram 設定」的「更新模型包…」重新選擇模型包。', mbError, MB_OK);
#endif
  Code := RunAsUser('powershell.exe', '-NoProfile -ExecutionPolicy Bypass -File "' + ExpandConstant('{app}\keyboard.ps1') + '" add', '正在加入鍵盤…');
  if Code = 2 then
    MsgBox('語言清單中沒有「中文（台灣）」。請到「設定 > 時間與語言 > 語言與地區」加入後，再從它的鍵盤選項加入 Bopogram。', mbInformation, MB_OK);
  ExecAsOriginalUser(ExpandConstant('{app}\bopogram-host.exe'), '', '', SW_HIDE, ewNoWait, Code);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  if CurUninstallStep <> usPostUninstall then exit;
  Data := ExpandConstant('{localappdata}\Bopogram');
  if not DirExists(Data) then exit;
  if UninstallSilent then exit;
  if MsgBox('要一併刪除 Bopogram 的模型、設定與個人化資料嗎？' + #13#10 + #13#10 + Data + #13#10 + #13#10 +
      '選「否」會保留，之後重新安裝可以繼續使用。', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    DelTree(Data, True, True, True);
end;
