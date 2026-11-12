; Inno Setup script — per-user install, no admin prompt.
; Build:  iscc /DAppVersion=1.0.0 installer\setup.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "Claude Usage Widget"
#define AppExe "claude-usage-widget.exe"

[Setup]
AppId={{6E0C2D1B-4B7A-4F6E-9C1E-3A8D2B5F7C41}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=djluiso
AppPublisherURL=https://github.com/djluiso/claude-usage-widget
AppSupportURL=https://github.com/djluiso/claude-usage-widget/issues
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
OutputDir=..\dist
OutputBaseFilename=ClaudeUsageWidget-{#AppVersion}-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2
SolidCompression=yes

[Tasks]
Name: "startup"; Description: "Start with Windows"; GroupDescription: "Options:"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"

[Registry]
; Same value the app's "Start with Windows" menu item uses, so they stay in sync.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ClaudeUsageWidget"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "StopWidget"

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\ClaudeUsageWidget"

[Code]
// Stop a running copy before files are replaced (upgrades / reinstalls).
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE,
       ewWaitUntilTerminated, Code);
  Result := '';
end;

// The Run value may also have been added from the app's own menu.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'ClaudeUsageWidget');
end;
