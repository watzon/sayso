; The Windows installer. Build it with scripts\bundle-windows.ps1 -Installer,
; which passes AppVersion, SourceDir (build\windows\Sayso), and OutputDir.
;
; It installs for the current user by default, into
; %LOCALAPPDATA%\Programs\Sayso, so it needs no administrator. The user can
; choose an install for all users in the first dialog.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\build\windows\Sayso"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
; Sayso holds this mutex while it runs (paths.InstanceMutex).
#define InstanceMutex "Local\dev.sayso.Sayso.instance"

[Setup]
; The id of the app for Windows. Never change it: updates find the
; installed app by it.
AppId={{6A0D2C3B-4E7F-4B8A-9C1D-5A3E8F2B7C61}
AppName=Sayso
AppVersion={#AppVersion}
AppVerName=Sayso {#AppVersion}
AppPublisher=Watzon Ventures LLC
AppPublisherURL=https://justsayso.app
AppSupportURL=https://github.com/watzon/sayso/issues
AppUpdatesURL=https://github.com/watzon/sayso/releases
DefaultDirName={autopf}\Sayso
DefaultGroupName=Sayso
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
LicenseFile={#SourceDir}\LICENSE.txt
OutputDir={#OutputDir}
OutputBaseFilename=Sayso-{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\app\AppIcon.ico
UninstallDisplayIcon={app}\Sayso.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Setup asks the user to quit Sayso first.
AppMutex={#InstanceMutex}
CloseApplications=yes
; Pick the language from Windows without a question.
ShowLanguageDialog=no

; The language of the installer: the first one that matches the language of
; Windows, else English. These files come with Inno Setup 6. Simplified
; Chinese has no official file yet, so it gets English.
[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; The speech engine before 0.12. sayso-engine.exe replaces it.
Type: files; Name: "{app}\SaysoEngine.exe"
; The engine builds for each instruction set of 0.13 to 0.16. One sayso-engine.exe replaces them.
Type: files; Name: "{app}\sayso-engine-avx.exe"
Type: files; Name: "{app}\sayso-engine-baseline.exe"
; The cleanup runtime of 0.16 and older, in the app folder. llama\sayso-cleanup.exe replaces it.
Type: files; Name: "{app}\sayso-cleanup.exe"
Type: files; Name: "{app}\sayso-cleanup-avx.exe"
Type: files; Name: "{app}\sayso-cleanup-baseline.exe"

[Icons]
Name: "{autoprograms}\Sayso"; Filename: "{app}\Sayso.exe"
Name: "{autodesktop}\Sayso"; Filename: "{app}\Sayso.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Run]
Filename: "{app}\Sayso.exe"; Description: "{cm:LaunchProgram,Sayso}"; Flags: nowait postinstall skipifsilent; Check: not IsUpdating
; An update that Sayso started (/UPDATE=1) starts the new version, also in
; silent mode.
Filename: "{app}\Sayso.exe"; Flags: nowait; Check: IsUpdating

[UninstallRun]
; Quit a running Sayso, so its files can be removed.
Filename: "{sys}\taskkill.exe"; Parameters: "/IM Sayso.exe /F"; Flags: runhidden; RunOnceId: "QuitSayso"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM sayso-engine.exe /F"; Flags: runhidden; RunOnceId: "QuitEngine"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM sayso-engine-avx.exe /F"; Flags: runhidden; RunOnceId: "QuitEngineAvx"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM sayso-engine-baseline.exe /F"; Flags: runhidden; RunOnceId: "QuitEngineBaseline"
; The cleanup runtime (llama\sayso-cleanup.exe) holds the llama.cpp DLLs open.
Filename: "{sys}\taskkill.exe"; Parameters: "/IM sayso-cleanup.exe /F"; Flags: runhidden; RunOnceId: "QuitCleanup"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM sayso-cleanup-avx.exe /F"; Flags: runhidden; RunOnceId: "QuitCleanupAvx"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM sayso-cleanup-baseline.exe /F"; Flags: runhidden; RunOnceId: "QuitCleanupBaseline"

[Registry]
; Settings › General › Launch at login writes this value. Remove it with the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Sayso"; Flags: uninsdeletevalue dontcreatekey

; History, models, and settings stay in %APPDATA%\Sayso and %LOCALAPPDATA%\Sayso
; after an uninstall, as on macOS. The README says how to remove them.

[Code]
// The helpers of Sayso, with the names of every version since 0.12.
const
  HelperNames = 'sayso-engine,sayso-engine-avx,sayso-engine-baseline,sayso-cleanup,sayso-cleanup-avx,sayso-cleanup-baseline';
  InstanceMutex = '{#InstanceMutex}';


// IsUpdating is true when Sayso started Setup to install an update
// (internal/update, SetupCommandLine).
function IsUpdating: Boolean;
begin
  Result := ExpandConstant('{param:UPDATE|0}') = '1';
end;

// Sayso quits right after it starts Setup for an update. Setup checks
// AppMutex after InitializeSetup, so wait here, up to 15 seconds, for Sayso
// to be gone; if it still runs then, AppMutex asks the user to close it.
function InitializeSetup: Boolean;
var
  Waited: Integer;
begin
  Result := True;
  if not IsUpdating then
    Exit;
  Waited := 0;
  while CheckForMutexes(InstanceMutex) and (Waited < 15000) do
  begin
    Sleep(250);
    Waited := Waited + 250;
  end;
end;


// AppMutex makes the user quit Sayso before the install. A helper that
// outlives its app (after a crash) still holds its exe open, and then the
// install cannot replace or delete it. So stop each helper that runs from the
// install folder, and no process of another app with the same name.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Dir, Command: String;
  ResultCode: Integer;
begin
  Result := '';
  Dir := AddBackslash(ExpandConstant('{app}'));
  StringChangeEx(Dir, '''', '''''', True);
  Command := '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' +
    'Get-Process -Name ' + HelperNames + ' -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.Path -and $_.Path.StartsWith(''' + Dir + ''', [StringComparison]::OrdinalIgnoreCase) } | ' +
    'Stop-Process -Force -ErrorAction SilentlyContinue"';
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'), Command, '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
