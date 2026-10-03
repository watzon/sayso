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
; Sayso holds this mutex while it runs (sayso-platform-windows, window.rs).
; Setup asks the user to quit Sayso first.
AppMutex=Local\dev.sayso.Sayso.instance
CloseApplications=yes

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Sayso"; Filename: "{app}\Sayso.exe"
Name: "{autodesktop}\Sayso"; Filename: "{app}\Sayso.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Run]
Filename: "{app}\Sayso.exe"; Description: "{cm:LaunchProgram,Sayso}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Quit a running Sayso, so its files can be removed.
Filename: "{sys}\taskkill.exe"; Parameters: "/IM Sayso.exe /F"; Flags: runhidden; RunOnceId: "QuitSayso"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM SaysoEngine.exe /F"; Flags: runhidden; RunOnceId: "QuitEngine"

[Registry]
; Settings › General › Launch at login writes this value. Remove it with the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Sayso"; Flags: uninsdeletevalue dontcreatekey

; History, models, and settings stay in %APPDATA%\Sayso and %LOCALAPPDATA%\Sayso
; after an uninstall, as on macOS. The README says how to remove them.
