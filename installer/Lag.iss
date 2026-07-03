; ═══════════════════════════════════════════════════════════════════════════
; Lag — Inno Setup installer (replaces the Velopack one-click Setup).
;
; Build (see the release recipe):
;   dotnet publish InstantReplay/InstantReplay.csproj -c Release -r win-x64 ^
;       --self-contained -p:Version=<ver> -o release-publish
;   ISCC installer\Lag.iss /DAppVersion=<ver> /DPublishDir=..\release-publish
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

procedure CurStepChanged(CurStep: TSetupStep);
var
  R: Integer;
begin
  if CurStep = ssPostInstall then
  begin
    CleanupLegacyVelopack();

    { Silent self-update flow: the app hands off to "Setup /VERYSILENT /AUTOSTART" and
      exits; relaunch it once the new files are in place. }
    if CmdLineParamExists('/AUTOSTART') then
      Exec(ExpandConstant('{app}\{#MyAppExeName}'), '', '', SW_SHOWNORMAL, ewNoWait, R);
  end;
end;
