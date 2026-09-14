; Per-user installer for codex-switch.exe (no elevation).
; Build: cargo build --release, then
;   ISCC.exe installer\codex-switcher.iss
; Output: target\installer\codex-switcher-setup-<version>.exe

; Override the exe location with /DExeDir=... (e.g. a separate --target-dir while a tray
; started from target\release keeps that exe locked).
#ifndef ExeDir
  #define ExeDir "..\target\release"
#endif

#define AppName "Codex Account Switcher"
#define AppVersion "0.1.0"
#define AppExe "codex-switch.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"
#define RunValue "CodexAccountSwitcher"

[Setup]
AppId={{6E2A9C1D-4B7F-4E8A-9D3C-2F1B5A7C8E90}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=tsuyoshi-otake
DefaultDirName={localappdata}\Programs\CodexAccountSwitcher
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=codex-switcher-setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
UninstallDisplayIcon={app}\{#AppExe}
; The tray holds the exe open; let setup close it instead of failing on a locked file.
CloseApplications=yes
RestartApplications=no

[Files]
Source: "{#ExeDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"

[UninstallRun]
; Only our own tray image; Codex / ChatGPT processes are never touched.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "StopTray"

[UninstallDelete]
; Profiles, journal and settings in %LOCALAPPDATA%\CodexAccountSwitcher are kept on purpose:
; they hold the user's encrypted accounts. Delete that folder manually to remove them.
Type: dirifempty; Name: "{app}"

[Code]
// Autostart stays owned by the app (first tray run registers it; an opt-out sticks).
// Setup only repoints an existing Run value at the installed exe and removes it on uninstall.
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and RegValueExists(HKCU, '{#RunKey}', '{#RunValue}') then
    RegWriteStringValue(HKCU, '{#RunKey}', '{#RunValue}', '"' + ExpandConstant('{app}\{#AppExe}') + '"');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKCU, '{#RunKey}', '{#RunValue}');
end;
