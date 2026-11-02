; The Herdr app's Windows installer: a setup.exe people double-click, for the
; same layout install.ps1 makes (so either one updates the other's install).
;
; Built by scripts/setup-exe, which compiles dist/herdr-app/ from
; scripts/package and passes the version, the app folder, the .ico and the
; output folder in.
;
; Writes what install.ps1 writes and nothing else:
;   %LOCALAPPDATA%\Programs\herdr-app   the app, replacing the old one
;   HKCU\Environment\Path               the folder, once, keeping the type
;   the Start menu's Herdr.lnk          the app, not a console
; An install.ps1 install is recognised by that folder: same place, same
; shortcut, so a person can install with one and update with the other.
;
; The core is not bundled: it is upstream's program with its own release, and
; the app installs it on first start (as install.ps1 does, for a missing one).

#define AppExe "herdr-app.exe"
#define AppName "Herdr"
#define AppPublisher "Herdr"
#define AppUrl "https://herdr.dev"

#ifndef Version
  #error Version is not defined (scripts/setup-exe passes it)
#endif
#ifndef SourceDir
  #error SourceDir is not defined (scripts/setup-exe passes dist/herdr-app)
#endif
#ifndef IconFile
  #define IconFile "..\..\assets\icons\herdr-app.ico"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
; Keep this AppId: it is what makes an update an update, and what the
; uninstaller in HKCU is found by. Changing it orphans installed copies.
AppId={{8E4B1A6C-3F2D-4C1B-9E77-5A0C2B7D4F31}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}
AppUpdatesURL={#AppUrl}
VersionInfoVersion={#Version}
VersionInfoDescription={#AppName} {#Version} setup
VersionInfoProductName={#AppName}
VersionInfoProductVersion={#Version}
; Per-user, like install.ps1: no admin prompt, and the app's own updates
; (which replace files in this folder) keep working without elevation.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\herdr-app
; The same folder install.ps1 uses, so the two update each other.
DisableDirPage=yes
DisableProgramGroupPage=yes
OutputDir={#OutputDir}
OutputBaseFilename=herdr-app-windows-x86_64-setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Windows 10 1803: what the app itself needs (its updater uses tar.exe).
MinVersion=10.0.17134
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; A running Herdr holds its own exe: let the Restart Manager close it and open
; it again, so an update does not ask the person to quit first.
CloseApplications=yes
RestartApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
; Ticked: someone installing Herdr probably wants `herdr-app` in a terminal.
Name: "addtopath"; Description: "Add Herdr to your PATH (the herdr-app command in new terminals)"; GroupDescription: "Other:"

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\herdr-app.svg"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-NOTICES"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Herdr"; Filename: "{app}\{#AppExe}"; WorkingDir: "{%USERPROFILE}"; Comment: "Terminals and coding agents that keep running"

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The app's own updater works beside the app folder, not in it: a staged
; build, its receipt, the download scratch and the folder it moved aside.
Type: filesandordirs; Name: "{localappdata}\Programs\herdr-app.old"
Type: filesandordirs; Name: "{localappdata}\Programs\herdr-app.new"
Type: filesandordirs; Name: "{localappdata}\Programs\herdr-app.download"
Type: files; Name: "{localappdata}\Programs\herdr-app.new.json"

[Code]
const
  EnvironmentKey = 'Environment';
  PathValue = 'Path';

// The user's PATH as it is stored. Inno 6 reads a REG_EXPAND_SZ value without
// expanding it (so other programs' %VARIABLES% stay text) and writes it back
// with the type it had, which is what install.ps1 goes out of its way to do.
function ReadUserPath(out Value: String): Boolean;
begin
  Result := RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, PathValue, Value);
  if not Result then
    Value := '';
end;

function WriteUserPath(const Value: String): Boolean;
begin
  Result := RegWriteStringValue(HKEY_CURRENT_USER, EnvironmentKey, PathValue, Value);
end;

// Trailing separators are not part of a folder: C:\x\ is C:\x.
function TrimSeparators(const Value: String): String;
begin
  Result := Value;
  while (Result <> '') and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

function PathListed(const Value, Folder: String): Boolean;
var
  Rest, Entry: String;
begin
  Result := False;
  Rest := Value + ';';
  while Rest <> '' do begin
    Entry := Copy(Rest, 1, Pos(';', Rest) - 1);
    Delete(Rest, 1, Pos(';', Rest));
    if CompareText(Trim(TrimSeparators(Entry)), Folder) = 0 then begin
      Result := True;
      exit;
    end;
  end;
end;

function WithoutFolder(const Value, Folder: String): String;
var
  Rest, Entry: String;
begin
  Result := '';
  Rest := Value + ';';
  while Rest <> '' do begin
    Entry := Copy(Rest, 1, Pos(';', Rest) - 1);
    Delete(Rest, 1, Pos(';', Rest));
    if (Entry <> '') and (CompareText(Trim(TrimSeparators(Entry)), Folder) <> 0) then begin
      if Result <> '' then
        Result := Result + ';';
      Result := Result + Entry;
    end;
  end;
end;

function HerdrFolder(): String;
begin
  Result := TrimSeparators(ExpandConstant('{app}'));
end;

// New terminals read the environment; already-running ones do not. Setting any
// user variable tells Windows the environment changed (install.ps1 does this).
procedure TellProgramsEnvironmentChanged();
begin
  RegWriteStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'HERDR_INSTALLING', '1');
  RegDeleteValue(HKEY_CURRENT_USER, EnvironmentKey, 'HERDR_INSTALLING');
end;

procedure AddToPath();
var
  Value: String;
begin
  ReadUserPath(Value);
  if PathListed(Value, HerdrFolder()) then
    exit;
  if (Value <> '') and (Value[Length(Value)] <> ';') then
    Value := Value + ';';
  Value := Value + HerdrFolder();
  if not WriteUserPath(Value) then
    MsgBox('Herdr could not add itself to your PATH. Add ' + HerdrFolder() + ' to it yourself, or start Herdr from that folder.', mbError, MB_OK)
  else
    TellProgramsEnvironmentChanged();
end;

procedure RemoveFromPathIfOurs();
var
  Value: String;
begin
  if not ReadUserPath(Value) then
    exit;
  if not PathListed(Value, HerdrFolder()) then
    exit;
  if WriteUserPath(WithoutFolder(Value, HerdrFolder())) then
    TellProgramsEnvironmentChanged();
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath();
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  // Only the folder this install put there; one added by hand behaves the same.
  if CurUninstallStep = usPostUninstall then
    RemoveFromPathIfOurs();
end;
