; ═══════════════════════════════════════════════════════════════════════════
; Lag — Inno Setup installer (replaces the Velopack one-click Setup).
;
; Build (see the release recipe):
;   dotnet publish InstantReplay/InstantReplay.csproj -c Release -r win-x64 ^
;       --self-contained -p:Version=<ver> -o release-publish
;   ISCC installer\Lag.iss /DAppVersion=<ver> /DPublishDir=..\release-publish
;
; Rust build (lag-rs): lag-rs\tools\package.ps1 -Version <ver> -Installer assembles the folder
; (exe, FFmpeg/libmpv DLLs, ffmpeg\, rnnoise\std.rnnn, Assets\), refuses to continue when any
; runtime file is missing, then runs ISCC on this script.
;
; The app's updater (AppUpdateService) downloads this installer from GitHub Releases and
; runs it with /VERYSILENT /AUTOSTART — silent in-place update + relaunch. On its first
; run over a legacy Velopack install it also removes the old %LocalAppData%\Lag layout.
; ═══════════════════════════════════════════════════════════════════════════

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef PublishDir
  #define PublishDir "..\release-publish"
#endif

#define MyAppName "Lag"
#define MyAppExeName "Lag.exe"
#define MyAppPublisher "shkbb"
#define MyAppURL "https://github.com/shkbb/Lag"

[Setup]
; NEVER change AppId — it keys the uninstall entry and lets updates find the existing
; install dir. AppUpdateService.DetectInstall matches the "Lag_is1" uninstall key.
AppId=Lag
AppName={#MyAppName}
AppVersion={#AppVersion}
AppVerName={#MyAppName} {#AppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}/releases
VersionInfoVersion={#AppVersion}
WizardStyle=modern
SetupIconFile=..\InstantReplay\Assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
LicenseFile=..\LICENSE

; Per-user by default (no UAC, like the old installer), but the user may elevate from a
; dialog and install for all users (then {autopf} = Program Files).
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\{#MyAppName}
; The whole point of the swap: a real wizard with a folder-picker page.
DisableDirPage=no
DisableProgramGroupPage=yes
DisableWelcomePage=no

; Windows 10 1903+ (WGC requirement of the capture engine).
MinVersion=10.0.18362
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

; Updates install over a possibly running app: close it via Restart Manager, do not
; auto-restart it (we relaunch explicitly through /AUTOSTART).
; One Setup at a time. Lag 2.0.0 can start two silent updates at once (it checked for updates twice at
; launch); the second one finds this mutex and exits.
SetupMutex=LagSetupMutex,Global\LagSetupMutex
CloseApplications=yes
RestartApplications=no

Compression=lzma2/max
SolidCompression=yes
OutputBaseFilename=Lag-win-Setup
OutputDir=Output

[Languages]
Name: "english";    MessagesFile: "compiler:Default.isl"
Name: "ukrainian";  MessagesFile: "compiler:Languages\Ukrainian.isl"
Name: "czech";      MessagesFile: "compiler:Languages\Czech.isl"
Name: "danish";     MessagesFile: "compiler:Languages\Danish.isl"
Name: "dutch";      MessagesFile: "compiler:Languages\Dutch.isl"
Name: "finnish";    MessagesFile: "compiler:Languages\Finnish.isl"
Name: "french";     MessagesFile: "compiler:Languages\French.isl"
Name: "german";     MessagesFile: "compiler:Languages\German.isl"
Name: "hungarian";  MessagesFile: "compiler:Languages\Hungarian.isl"
Name: "italian";    MessagesFile: "compiler:Languages\Italian.isl"
Name: "japanese";   MessagesFile: "compiler:Languages\Japanese.isl"
Name: "korean";     MessagesFile: "compiler:Languages\Korean.isl"
Name: "norwegian";  MessagesFile: "compiler:Languages\Norwegian.isl"
Name: "polish";     MessagesFile: "compiler:Languages\Polish.isl"
Name: "portuguese"; MessagesFile: "compiler:Languages\Portuguese.isl"
Name: "russian";    MessagesFile: "compiler:Languages\Russian.isl"
Name: "spanish";    MessagesFile: "compiler:Languages\Spanish.isl"
Name: "turkish";    MessagesFile: "compiler:Languages\Turkish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#PublishDir}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
; Finish-page checkbox for interactive installs; silent updates relaunch via /AUTOSTART instead.
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[Code]
function CmdLineParamExists(const Value: string): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Value) = 0 then
    begin
      Result := True;
      Exit;
    end;
end;

{ One-time migration off the legacy Velopack install. Only a REAL Velopack layout is
  touched (its Update.exe stub is the marker), and never when the user chose to install
  INTO that folder. Removes the app tree, its shortcuts and its uninstall entry; user data
  (settings in %AppData%\Lag, clips, logs) lives elsewhere and is not touched. }
procedure CleanupLegacyVelopack();
var
  OldRoot: string;
begin
  OldRoot := ExpandConstant('{localappdata}\Lag');
  if not FileExists(OldRoot + '\Update.exe') then Exit;
  if Pos(LowerCase(OldRoot), LowerCase(ExpandConstant('{app}'))) = 1 then Exit;

  Log('Migrating: removing the legacy Velopack install at ' + OldRoot);
  DelTree(OldRoot, True, True, True);
  DeleteFile(ExpandConstant('{userdesktop}\Lag.lnk'));
  DeleteFile(ExpandConstant('{userprograms}\Lag.lnk'));
  RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER,
    'Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag');
end;

{ One-time migration off the .NET build (1.x, Inno-installed): its self-contained runtime
  (~1100 files, ~480 MB) would otherwise stay next to the Rust build, since [Files] only
  adds and overwrites. Its own uninstaller removes exactly the files it installed (nothing
  else in a folder the user picked); settings in %AppData%\Lag, clips and logs live
  elsewhere and are not touched. Marker: the .NET host's Lag.runtimeconfig.json. }
procedure UninstallDotNetBuild();
var
  Cmd: string;
  R, I: Integer;
begin
  if not FileExists(ExpandConstant('{app}\Lag.runtimeconfig.json')) then Exit;
  if not RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag_is1',
       'UninstallString', Cmd) then
    if not RegQueryStringValue(HKLM, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag_is1',
         'UninstallString', Cmd) then Exit;
  Cmd := RemoveQuotes(Cmd);
  { only an uninstaller that sits in the folder we are installing into }
  if CompareText(ExtractFilePath(Cmd), AddBackslash(ExpandConstant('{app}'))) <> 0 then Exit;
  if not FileExists(Cmd) then Exit;
  Log('Migrating: uninstalling the .NET build in ' + ExpandConstant('{app}'));
  if not Exec(Cmd, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, R) then
  begin
    Log('Migrating: the .NET build uninstaller did not start');
    Exit;
  end;
  { The uninstaller finishes in a second copy of itself that deletes unins000.exe last; this
    process can return before that. Wait until it is gone, so it cannot remove files this setup
    is about to write (Lag.exe, Assets\, ffmpeg\ and rnnoise\ have the same names in both builds).
    Seen in the LagTest run of 2026-11-18: the new uninstaller became unins001 because unins000
    still existed. }
  for I := 1 to 600 do
  begin
    if not FileExists(Cmd) then Break;
    Sleep(100);
  end;
  if FileExists(Cmd) then
    Log('Migrating: the old uninstaller was still there after 60 s, continuing')
  else
    Log('Migrating: .NET build removed');
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  R: Integer;
begin
  if CurStep = ssInstall then
    UninstallDotNetBuild();

  if CurStep = ssPostInstall then
  begin
    CleanupLegacyVelopack();

    { Silent self-update flow: the app hands off to "Setup /VERYSILENT /AUTOSTART" and
      exits; relaunch it once the new files are in place. }
    if CmdLineParamExists('/AUTOSTART') then
      Exec(ExpandConstant('{app}\{#MyAppExeName}'), '', '', SW_SHOWNORMAL, ewNoWait, R);
  end;
end;
