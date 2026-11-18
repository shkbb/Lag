# App Shell Specification (1:1 Port Reference)

Target: Rust rewrite (`lag-rs`: Slint UI, `libmpv` player, `windows-rs`, FFmpeg 7.1 DLLs) matching the C# / .NET 8 / Avalonia 11 implementation in `E:\project\InstantReplay\InstantReplay\` with identical visual styling and runtime behaviour.

---

## 1. Scope

Files covered in this specification (with line counts):

| File | Lines | Role |
|---|---:|---|
| `Program.cs` | 272 | Entry point, FileLog init, 1 ms timer resolution, `LAG_*` test/probe dispatch, crash handlers, Velopack shim, single-instance mutex, `DynamicRenderTimer` binding |
| `App.axaml` | 351 | Root Avalonia application resources, 17 vector flag `DrawingImage`s, converters, popup/slider sizing tokens, `ToggleSwitch` `ControlTheme`, `TrayIcon` + `NativeMenu` |
| `App.axaml.cs` | 425 | FFmpeg dir/DLL/PATH setup, DI container, `SetLanguage`, `SetTheme` (with bitmap cross-fade), `StartMinimized`, global hotkey wiring, tray menu localization & click handlers, `ExitWatchdog` |
| `Views/MainWindow.axaml` | 415 | Frameless main window (`1360×840`, min `1080×640`), top command bar (`58px`), icon rail (`78px`), `TransitioningContentControl`, capture-target hover flyout, shell styles |
| `Views/MainWindow.axaml.cs` | 168 | `RetuneRenderTimer`, tray-hide on `Closing` (stopping Player + Editor playback), active-nav / record-CTA / fullscreen state sync, drag/min/max/close chrome, `ESC` exits fullscreen |
| `ViewModels/MainViewModel.cs` | 744 | Root shell VM: child VMs, navigation commands, recording start/stop/pause/save orchestration, buffer fill ticker, capture target & toast triggers, `StatsHudWindow` lifecycle, startup diagnostics, sound playback |
| `Views/StatsHudWindow.cs` | 168 | Always-on-top click-through uncapturable (`WDA_EXCLUDEFROMCAPTURE`) live FPS/CPU/GPU/RAM HUD window |
| `Services/ToastService.cs` | 228 | Sequential queued non-activating topmost toast notifications in primary screen top-right working area (suppressed on exclusive fullscreen) |
| `Services/GlobalHotkeyManager.cs` | 219 | SharpHook (`TaskPoolGlobalHook`) system-wide keyboard/mouse hook: key rebinding capture, PTT press/release, `KeystrokeTracker` feed, save-hotkey press |
| `Services/GlobalHotkeyService.cs` | 373 | Win32 `RegisterHotKey` message-loop thread (`Win32HotkeyThread`) for Save Replay, Screenshot, Pause/Resume, and Start/Stop Record |
| `Services/AppUpdateService.cs` | 166 | Deployment detection (`DevOrPortable` / `Installed` / `LegacyVelopack`), GitHub Releases API check, silent `Lag-win-Setup.exe` download & handoff |
| `Services/FileLog.cs` | 150 | `Console.Out`/`Error` tee to `Documents\Lag\Logs\Lag-yyyy-MM-dd_HH-mm-ss.txt` (15-file retention) and `crash.log` (managed + native SEH crashes) |
| `Themes/Tokens.axaml` | 32 | Theme-invariant fonts, corner radii, brand logo SVG geometries |
| `Themes/Palette.axaml` | 287 | `Dark` and `Light` ("cream") `ThemeDictionaries` colour/brush tokens + Fluent override brushes |
| `Themes/Components.axaml` | 137 | Redesign `r`-prefixed reusable styles (`rcard`, `rdivider`, `rgroup`, `rrow`, `rsub`, `rmono`, `rkbd`, `rsegtrack`, `rseg`, `rpill`, `rchip`, `FlyoutPresenter`) |
| `Themes/DarkTheme.axaml` | 757 | Base/shared & legacy control styles (`TextBlock`, `Button`, `ListBox.segctl`, `CheckBox.app`, `Border.kbd`, `ComboBox`, `MenuItem`, `ToolTip`, `TextBox`, `Slider`, `ListBox.replays`, `ScrollBar`, `ProgressBar.buffer`, etc.) |
| `Core/BoolToStringConverter.cs` | 29 | `BoolToStringConverter` (`TrueText\|FalseText`) |
| `Core/DeferredDispose.cs` | 21 | `DeferredDispose.Later(IDisposable?, ms = 3000)` |
| `Core/FxAnimations.cs` | 47 | `FxAnimations.SlideFadeIn(Control, fromY = 16, ms = 240)` |
| `Core/LanguageToFlagConverter.cs` | 28 | `LanguageToFlagConverter` (`"en"` → `Flag_en` resource) |
| `Core/Localizer.cs` | 26 | `Localizer.Get(key)` and `Localizer.Format(key, args)` |
| `Core/UiConverters.cs` | 94 | `EqualsConverter`, `OneMinusIntConverter`, `IntensityToBrushConverter`, `StringToUpperConverter`, `LevelToWidthConverter` |
| `Core/ViewModelBase.cs` | 21 | `ViewModelBase` (`ObservableObject` + `Title`) |
| `Assets/Langs/*.axaml` (17 files) | 263–265 ea | Localized `ResourceDictionary` string tables (`en`, `uk`, `de`, `fr`, `be`, `lt`, `et`, `lv`, `fi`, `sv`, `no`, `da`, `nl`, `it`, `es`, `pt`, `ja`) |
| `Assets/save.wav`, `Assets/screenshot.wav`, `Assets/icon.ico` | — | Binary WAV feedback chimes (copied to output `Assets/`) and embedded app/tray/installer icon |
| `app.manifest` | 42 | Windows 10/11 compatibility GUID + `PerMonitorV2, PerMonitor` DPI awareness |
| `InstantReplay.csproj` | 105 | `net8.0-windows10.0.26100.0`, `WinExe`, assembly name `Lag`, asset copy rules, NuGet dependencies |
| `../installer/Lag.iss` | 149 | Inno Setup 64-bit per-user/elevatable installer, 18 setup languages, `/AUTOSTART` silent self-update, legacy Velopack migration |
| `../README.md` | 212 | Project documentation (English + Ukrainian) |

---

## 2. Features

1. **Single-Instance Guard**: Only one copy of Lag may run at a time (named mutex `LagAppSingleInstanceMutex`); a second launch exits silently, including when an existing instance is elevated (`Program.cs:157-173`).
2. **Legacy Velopack Update Shim**: Processes Velopack lifecycle CLI invocations (`install`/`update`/`uninstall`) before single-instance locking so legacy Velopack packages can migrate off (`Program.cs:148-155`).
3. **1 ms System Timer & Dynamic Display-Locked Render Timer**: Requests 1 ms Windows timer resolution (`timeBeginPeriod(1)`) at startup (`Program.cs:68`) and paces UI/compositor rendering via `DynamicRenderTimer`, which locks to the `HMONITOR` DXGI `WaitForVerticalBlank` of whichever display currently hosts `MainWindow`, falling back to 60 Hz clock pacing when hidden/minimized (`Program.cs:196-270`, `MainWindow.axaml.cs:102-127`, `DynamicRenderTimer.cs:20-165`).
4. **Process Background-Throttling Opt-Out (`PerformanceGuard`)**: Disables Windows EcoQoS/power throttling (`PROCESS_POWER_THROTTLING_EXECUTION_SPEED = 0`) and attempts to raise GPU scheduling priority (`D3DKMTSetProcessSchedulingPriorityClass`: Realtime → High → AboveNormal) so fullscreen games do not starve capture or hotkeys (`App.axaml.cs:216`, `PerformanceGuard.cs:47-85`).
5. **Persistent File Logging & Crash Forensics**: Tees `stdout`/`stderr` to timestamped logs in `Documents\Lag\Logs\Lag-yyyy-MM-dd_HH-mm-ss.txt` (retaining newest 15 files) and records managed unhandled exceptions + native Windows SEH crashes (with faulting DLL + offset) to `Documents\Lag\Logs\crash.log` (`Program.cs:63,140-146`, `FileLog.cs:29-149`).
6. **Headless Diagnostic/Test CLI Modes**: 9 environment-variable-gated headless test/probe modes (`LAG_VFR_TEST`, `LAG_MFPROBE`, `LAG_WGCPROBE`, `LAG_DUPPROBE`, `LAG_STALLTEST`, `LAG_GCTEST`, `LAG_CADENCETEST`, `LAG_CARRYTEST`, `LAG_PROCTEST`) exit early before UI startup (`Program.cs:71-135`).
7. **Bundled FFmpeg Bootstrap**: Configures working directory, `SetDllDirectory`, `AddDllDirectory`, `PATH`, `FFMpegCore`, and `FfmpegInterop` to `<exe_dir>\ffmpeg` before ViewModels probe encoders (`App.axaml.cs:38-50`).
8. **Frameless Custom-Chrome Main Window**: Custom title/command bar (`58px` height), draggable logo & spacer, custom Minimize / Maximize-Restore / Close buttons, and left icon rail (`78px` width) (`MainWindow.axaml:13-16,218-378`, `MainWindow.axaml.cs:137-149`).
9. **Top Command Bar Controls**:
   - **Record CTA** (`Start Recording` / `Stop Recording`, switches cyan ↔ red, animated blinking stop-square while recording, 1200 ms toggle debounce) (`MainWindow.axaml:246-258`, `MainViewModel.cs:598-607`).
   - **Save Replay** button (disabled when not recording) (`MainWindow.axaml:261-267`, `MainViewModel.cs:654-693`).
   - **Pause / Resume** button (visible only while recording; toggles buffer pause without tearing down capture) (`MainWindow.axaml:270-283`, `MainViewModel.cs:615-633`).
   - **Status Pill** with idle/pulsing-red dot, status text, smooth `0–100%` buffer fill progress bar, and `m:ss / m:ss` buffer elapsed/total text (`MainWindow.axaml:289-305`, `MainViewModel.cs:46-90`).
10. **4-View Navigation with Animated Transitions**: Icon rail switches between **Library**, **Player**, **Editor**, and **Settings** (bottom-pinned) via `TransitioningContentControl` (vertical `PageSlide` + `CrossFade`, 220 ms). Defaults to **Settings** at startup (`MainWindow.axaml:331-393`, `MainViewModel.cs:190,331-368`).
11. **Live Capture Target Indicator & Hover Flyout**: Bottom of icon rail shows a pulsing `Controller` or `Monitor` icon when `HasCaptureTarget` is true; hovering slides out a `CaptureFlyout` card showing the captured game name or localized `"Desktop"` (`MainWindow.axaml:340-355,395-411`, `MainWindow.axaml.cs:153-157`, `MainViewModel.cs:93-144`).
12. **Player Fullscreen Mode**: Hides top command bar and left icon rail (`IsVisible="{Binding !IsFullscreen}"`), sets `WindowState = FullScreen`, and restores on `ESC` key (`MainWindow.axaml:221,328`, `MainWindow.axaml.cs:17,130-133,160-167`, `MainViewModel.cs:374-375`).
13. **Start Minimized & Minimize-to-Tray on Close**:
    - If `Settings.StartMinimized` is true, starts minimized without taskbar icon, hides on first `Opened`, then restores `ShowInTaskbar = true` (`App.axaml.cs:230-242`).
    - Clicking window Close cancels closing, stops Player and Editor playback so audio never leaks in the tray, and hides the window (`MainWindow.axaml.cs:34-54`).
14. **System Tray Icon & Context Menu**: Clicking tray icon opens/restores/focuses `MainWindow`; context menu offers `Save Replay`, `Open Library`, separator, `Open`, `Exit` (`App.axaml:332-349`, `App.axaml.cs:322-424`). Exit flushes debounced settings, disposes DI container, and uses a `2500 ms` watchdog thread to force process termination (`App.axaml.cs:405-424`).
15. **Auto-Start Recording on Launch**: If `Settings.AutoStartRecording` is true, posts `StartRecordingCommand` at `Background` priority after startup (`MainViewModel.cs:292-297`).
16. **Dual Global Hotkey System**:
    - **Win32 `RegisterHotKey`** (`GlobalHotkeyService`) on a dedicated background message loop handles `Save Replay` (`Alt+F10` default), `Screenshot` (`Alt+F9` default), `Pause/Resume` (`Alt+F8` default), and `Start/Stop Record` (`Alt+F7` default), all with `MOD_NOREPEAT` (`GlobalHotkeyService.cs:13-363`, `App.axaml.cs:251-301`, `SettingsViewModel.cs:160-278`).
    - **SharpHook `TaskPoolGlobalHook`** (`GlobalHotkeyManager`) handles keybinding capture (`IsCapturing`, `ESC` unbinds), Push-to-Talk hold/release (`V` default), mouse/key feeding to `KeystrokeTracker`, and also fires `HotkeyPressed` for `Save Replay` (`GlobalHotkeyManager.cs:19-163`, `MainViewModel.cs:244-274`).
    - All hotkey actions are suppressed during capture and for `800 ms` afterwards via `Settings.AreHotkeysSuppressed` (`SettingsViewModel.cs:1596-1609`).
17. **Instant Screenshots + Audio Feedback**: Global screenshot hotkey captures configured display (or primary) on a worker thread into `LibraryPath\Screenshot_yyyy-MM-dd_HH-mm-ss.png`, shows toast, and plays `Assets/screenshot.wav`; saving a replay plays `Assets/save.wav` immediately on press (`MainViewModel.cs:414-429,645-650,713-742`, `ScreenshotService.cs:41-68`).
18. **Sequential Non-Activating Toasts**: Top-right primary-monitor floating cards (`2.4 s` visible + `220 ms` fade in / `260 ms` fade out), bounded queue of 6, `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`, suppressed (sound-only) when an exclusive fullscreen D3D app is detected (`ToastService.cs:22-218`).
19. **Live Performance Stats HUD**: Always-on-top, click-through (`WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`), capture-excluded (`WDA_EXCLUDEFROMCAPTURE`) floating window showing 3 detail levels (`FPS`, `+ CPU/GPU`, `+ RAM`), auto-hiding during exclusive fullscreen (`StatsHudWindow.cs:20-168`, `MainViewModel.cs:185-186,436-455`).
20. **Live Theme Switching (`Dark` / `Light` Cream / `System`) with Cross-Fade**: Snapshots window pixels to an `OverlayLayer` image, flips `RequestedThemeVariant`, and fades the snapshot out over `280 ms` (`CubicEaseOut`) (`App.axaml.cs:103-199`, `Themes/Palette.axaml:18-286`).
21. **Live Localization (17 Languages) + Vector Flags**: Swaps merged `ResourceDictionary` (`Assets/Langs/{code}.axaml`) live across 17 languages (`en`, `uk`, `de`, `fr`, `be`, `lt`, `et`, `lv`, `fi`, `sv`, `no`, `da`, `nl`, `it`, `es`, `pt`, `ja`), with `en` fallback and 17 vector `DrawingImage` flags (`App.axaml:23-214`, `App.axaml.cs:61-84`, `Core/Localizer.cs:10-26`).
22. **Silent & Manual Self-Updater**: Checks `https://api.github.com/repos/shkbb/Lag/releases/latest` `3 s` after launch (and manually from Settings), ignores `DevOrPortable` builds, migrates `LegacyVelopack` installs even at equal version, downloads `Lag-win-Setup.exe` to `%TEMP%\Lag-Setup-{ver}.exe`, runs `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /AUTOSTART`, and exits (`AppUpdateService.cs:36-166`, `MainViewModel.cs:236-240`, `SettingsViewModel.cs:1344-1428`).
23. **Inno Setup Windows Installer**: 64-bit wizard (`Lag-win-Setup.exe`) defaulting to per-user `{autopf}\Lag` (elevatable via dialog), 18 installer languages, desktop shortcut task, Restart Manager app close, `/AUTOSTART` relaunch, and one-time cleanup of `%LocalAppData%\Lag` Velopack installs (`installer/Lag.iss:1-149`).

---

## 3. UI

### 3.1 MainWindow (`Views/MainWindow.axaml`)

- **Window properties** (`MainWindow.axaml:8-16`):
  - `Title`: `"Lag"`, `Icon`: `avares://Lag/Assets/icon.ico`
  - `Width`: `1360`, `Height`: `840`, `MinWidth`: `1080`, `MinHeight`: `640`
  - `Background`: `#0C0D11` (hardcoded fallback on Window; inner root Grid uses `{DynamicResource SurfaceApp}`)
  - `ExtendClientAreaToDecorationsHint="True"`, `ExtendClientAreaChromeHints="NoChrome"`, `ExtendClientAreaTitleBarHeightHint="-1"`, `WindowStartupLocation="CenterScreen"`
- **View DataTemplates** (`MainWindow.axaml:19-32`): Maps `SettingsViewModel` → `SettingsView`, `LibraryViewModel` → `LibraryView`, `PlayerViewModel` → `PlayerView`, `EditorViewModel` → `EditorView`.
- **Root Layout** (`MainWindow.axaml:216`): `Grid RowDefinitions="Auto,*"`, `Background="{DynamicResource SurfaceApp}"`.

#### Top Command Bar (`MainWindow.axaml:219-320`)
- Container: `Border` (`Grid.Row="0"`, `Height="58"`, `Background="{DynamicResource SurfaceApp}"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="0,0,0,1"`, `IsVisible="{Binding !IsFullscreen}"`).
- Inner `Grid`: `ColumnDefinitions="Auto,Auto,Auto,Auto,Auto,*,Auto,Auto"`, `Margin="18,0,0,0"`.
  - **Col 0 — Logo (drag zone)**: `StackPanel` (`Orientation="Horizontal"`, `Spacing="10"`, `VerticalAlignment="Center"`, `Background="Transparent"`, `PointerPressed="OnTitleBarPointerPressed"` → `BeginMoveDrag`):
    - `Border` (`28×28`, `Background="{DynamicResource Accent}"`, `CornerRadius="8"`): `Canvas` (`22×22`, centred) containing 3 `Path` elements (`LogoArcGeometry`, `LogoCornerGeometry`, `LogoHandsGeometry`) with `Stroke="{DynamicResource OnAccent}"`, `StrokeThickness="2"`, `StrokeLineCap="Round"`, `StrokeJoin="Round"`.
    - `TextBlock` (`Text="Lag"`, `FontFamily="{StaticResource FontDisplay}"`, `FontSize="18"`, `FontWeight="Bold"`, `Foreground="{DynamicResource TextHi}"`).
  - **Col 1 — Divider**: `Border` (`1×26`, `Margin="16,0"`, `Background="{DynamicResource BorderStrong}"`).
  - **Col 2 — Record CTA (`x:Name="RecordCta"`)**: `Button` (`Classes="tbar-accent"`, `.rec` added/removed in code-behind when `IsRecording` changes, `Command="{Binding ToggleRecordingCommand}"`):
    - `StackPanel` (`Horizontal`, `Spacing="9"`):
      - When `!IsRecording`: `Ellipse` (`9×9`, `Fill="{DynamicResource OnAccent}"`) + `TextBlock` (`{DynamicResource Btn_StartRecording}`).
      - When `IsRecording`: `Rectangle` (`10×10`, `RadiusX="2"`, `RadiusY="2"`, `Fill="White"`, blinks via keyframe animation) + `TextBlock` (`{DynamicResource Btn_StopRecording}`).
  - **Col 3 — Save Replay**: `Button` (`Classes="tbar"`, `Margin="10,0,0,0"`, `Command="{Binding SaveReplayCommand}"`, `IsEnabled="{Binding IsRecording}"`):
    - `StackPanel` (`Horizontal`, `Spacing="8"`): `MaterialIcon` (`Kind="TrayArrowDown"`, `15×15`) + `TextBlock` (`{DynamicResource Btn_SaveReplay}`).
  - **Col 4 — Pause / Resume**: `Button` (`Classes="tbar"`, `Margin="9,0,0,0"`, `Command="{Binding TogglePauseRecordingCommand}"`, `IsVisible="{Binding IsRecording}"`):
    - `StackPanel` (`Horizontal`, `Spacing="7"`): `MaterialIcon` (`Kind="Pause"` when `!IsPaused`, `Kind="Play"` when `IsPaused`, `13×13`) + `TextBlock` (`{DynamicResource Btn_PauseRecording}` when `!IsPaused`, `{DynamicResource Btn_ResumeRecording}` when `IsPaused`).
  - **Col 5 — Spacer (drag zone)**: `Panel` (`Background="Transparent"`, `PointerPressed="OnTitleBarPointerPressed"`).
  - **Col 6 — Status Pill**: `Border` (`Background="{DynamicResource SurfaceCard}"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="1"`, `CornerRadius="11"`, `Padding="14,7"`):
    - `StackPanel` (`Horizontal`, `Spacing="11"`):
      - Status dot: `Ellipse` (`8×8`): `{DynamicResource StatusIdle}` when `!IsRecording`; `Classes="recpulse"` `{DynamicResource RecDot}` when `IsRecording`.
      - `TextBlock` (`Text="{Binding StatusText}"`, `FontSize="12.5"`, `FontWeight="SemiBold"`, `Foreground="{DynamicResource TextKbd}"`, `MinWidth="62"`, `MaxWidth="160"`, `TextTrimming="CharacterEllipsis"`).
      - `ProgressBar` (`Classes="buffer"`, `Width="74"`, `Height="5"`, `Minimum="0"`, `Maximum="100"`, `Value="{Binding BufferFillPercent}"`).
      - `TextBlock` (`Text="{Binding BufferStatus}"`, `FontFamily="{StaticResource FontMono}"`, `FontSize="11"`, `Foreground="{DynamicResource TextMuted}"`).
  - **Col 7 — Window Controls**: `StackPanel` (`Horizontal`, `Margin="4,0,0,0"`):
    - Minimize button (`Classes="wctl"`, `Click="OnMinimizeClick"`): `MaterialIcon Kind="WindowMinimize"` (`14×14`).
    - Maximize/Restore button (`Classes="wctl"`, `Click="OnMaximizeClick"`): `MaterialIcon Kind="SquareRoundedOutline"` (`11×11`).
    - Close button (`Classes="wctl close"`, `Click="OnCloseClick"`): `MaterialIcon Kind="WindowClose"` (`14×14`).

#### Body (`MainWindow.axaml:323-412`)
`Grid` (`Grid.Row="1"`, `ColumnDefinitions="Auto,*"`):
- **Col 0 — Icon Rail (`78px`)**: `Border` (`Width="78"`, `Background="{DynamicResource SurfaceApp}"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="0,0,1,0"`, `IsVisible="{Binding !IsFullscreen}"`):
  - `DockPanel` (`Margin="0,14"`):
    - Bottom-docked **Settings** (`x:Name="RailSettings"`, `Classes="railitem"`, `Command="{Binding NavigateToSettingsCommand}"`): `MaterialIcon Kind="Tune"` (`20×20`) + `TextBlock {DynamicResource Nav_Settings}` (`Spacing="4"`).
    - Bottom-docked **Live Capture Indicator** (`x:Name="CaptureRailIcon"`, `44×44`, `Margin="0,0,0,12"`, `CornerRadius="12"`, `Cursor="Hand"`, `Background="Transparent"`, `IsVisible="{Binding HasCaptureTarget}"`, `PointerEntered="OnCaptureEntered"`, `PointerExited="OnCaptureExited"`): `MaterialIcon` (`22×22`, `Classes="capturepulse"`, `Foreground="{DynamicResource Accent}"`, `Kind="Controller"` if `CaptureTargetIsGame` else `Kind="Monitor"`).
    - Top-docked **Navigation Stack** (`Spacing="6"`, `HorizontalAlignment="Center"`):
      - `RailLibrary` (`Classes="railitem"`, `Command="{Binding NavigateToLibraryCommand}"`): `MaterialIcon Kind="ViewGridOutline"` (`20×20`) + `{DynamicResource Nav_Library}`.
      - `RailPlayer` (`Classes="railitem"`, `Command="{Binding NavigateToPlayerCommand}"`): `MaterialIcon Kind="Play"` (`20×20`) + `{DynamicResource Nav_Player}`.
      - `RailEditor` (`Classes="railitem"`, `Command="{Binding NavigateToEditorCommand}"`): `MaterialIcon Kind="PencilOutline"` (`20×20`) + `{DynamicResource Editor_Title}`.
- **Col 1 — Content Host**: `TransitioningContentControl` (`x:Name="ContentHost"`, `Background="{DynamicResource SurfaceApp}"`, `Content="{Binding CurrentView}"`):
  - `PageTransition`: `CompositePageTransition` combining `CrossFade` (`0:0:0.22`) and `PageSlide` (`0:0:0.22`, `Orientation="Vertical"`).
- **Col 1 Overlay — Capture Flyout** (`x:Name="CaptureFlyout"`, `Classes="captureflyout"`, `HorizontalAlignment="Left"`, `VerticalAlignment="Bottom"`, `Margin="10,0,0,82"`, `ZIndex="60"`, `IsHitTestVisible="False"`, `IsVisible="{Binding HasCaptureTarget}"`):
  - `Border`: `Background="{DynamicResource SurfaceCard}"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="1"`, `CornerRadius="12"`, `Padding="14,11"`, `BoxShadow="0 8 24 0 #66000000"`.
  - `StackPanel` (`Horizontal`, `Spacing="11"`): `MaterialIcon` (`20×20`, `Foreground="{DynamicResource Accent}"`, `Controller` or `Monitor`) + `TextBlock` (`Text="{Binding CaptureTargetName}"`, `FontSize="13.5"`, `FontWeight="SemiBold"`, `Foreground="{DynamicResource TextHi}"`, `MaxWidth="220"`, `TextTrimming="CharacterEllipsis"`).

#### MainWindow Shell Styles & Animations (`MainWindow.axaml:38-214`)
- `Button.tbar-accent`: `Background={Accent}`, `Foreground={OnAccent}`, `Height=38`, `CornerRadius=10`, `Padding=16,0`, `FontSize=13.5`, `FontWeight=SemiBold`, `BorderThickness=0`, `Cursor=Hand`; `ContentPresenter` `BrushTransition Background 0.15s`; `:pointerover` → `{AccentHoverBrush}`; `.rec` → `Background={DangerBrush}`, `Foreground=White`, `:pointerover` → `{DangerHoverBrush}`.
- `Button#RecordCta.rec Rectangle`: infinite keyframe animation `1.1s`, `SineEaseInOut`: `0% Opacity=1`, `50% Opacity=0.35`, `100% Opacity=1`.
- `Button.tbar`: `Background={SurfaceInput}`, `Foreground={TextBtn}`, `Height=38`, `CornerRadius=10`, `Padding=15,0`, `FontSize=13`, `FontWeight=SemiBold`, `BorderThickness=1`, `BorderBrush={BorderStrong}`, `Cursor=Hand`; `ContentPresenter` `BrushTransition Background 0.15s`; `:pointerover` → `{SurfaceHover}`; `:disabled` → `Opacity=0.45`.
- `Button.wctl`: `Background=Transparent`, `Foreground={TextFaint}`, `Width=42`, `Height=58`, `CornerRadius=0`, `BorderThickness=0`; `ContentPresenter` `BrushTransition Background 0.12s`; `:pointerover` → `{BorderFaint}`; `.close:pointerover` → `Background={DangerClose}` and `MaterialIcon Foreground=White`.
- `Button.railitem`: `Background=Transparent`, `Foreground={TextMid}`, `Width=62`, `Height=56`, `CornerRadius=11`, `BorderThickness=0`, `Cursor=Hand`, `FontSize=11`, `FontWeight=Medium`; `ContentPresenter` `BrushTransition Background 0.14s`; `:pointerover` → `{BorderFaint}`; `MaterialIcon` `{TextMid}`; `.active` → `Background={AccentDim}`, `Foreground={TextHi}`, `MaterialIcon` `{Accent}`.
  - `Button.railitem TextBlock`: `TextWrapping=Wrap`, `TextAlignment=Center`, `MaxLines=2`, `TextTrimming=CharacterEllipsis`, `MaxWidth=56`, `FontSize=9.5`, `LineHeight=11`.
  - `Button.railitem MaterialIcon`: `RenderTransform=scale(1)`, `TransformOperationsTransition 0.12s CubicEaseOut`; `:pressed` → `scale(0.82)`.
- `Ellipse.recpulse`: infinite keyframe animation `1.4s`: `0% Opacity=1`, `50% Opacity=0.3`, `100% Opacity=1`.
- `MaterialIcon.capturepulse`: infinite keyframe animation `1.8s`: `0% Opacity=1`, `50% Opacity=0.55`, `100% Opacity=1`.
- `Border.captureflyout`: `Opacity=0`, `RenderTransform=translateX(-12px)`, `DoubleTransition Opacity 0.18s CubicEaseOut`, `TransformOperationsTransition RenderTransform 0.18s CubicEaseOut`; `.shown` → `Opacity=1`, `translateX(0)`.

### 3.2 StatsHudWindow (`Views/StatsHudWindow.cs`)

- Frameless transparent topmost non-activating window (`StatsHudWindow.cs:35-43`):
  - `SystemDecorations=None`, `TransparencyLevelHint=[Transparent]`, `Background=null`, `Topmost=True`, `ShowInTaskbar=False`, `CanResize=False`, `ShowActivated=False`, `Focusable=False`, `SizeToContent=WidthAndHeight`.
  - Win32 extended styles applied on `Opened`: `WS_EX_TRANSPARENT (0x20) | WS_EX_LAYERED (0x80000) | WS_EX_NOACTIVATE (0x08000000) | WS_EX_TOOLWINDOW (0x80)`, plus `SetWindowDisplayAffinity(hwnd, 0x11)` (`WDA_EXCLUDEFROMCAPTURE`) (`StatsHudWindow.cs:133-146`).
- Visual tree (`StatsHudWindow.cs:45-60`):
  - `Border`: `Background=#D10E1116` (always dark translucent regardless of app theme), `BorderBrush=#33FFFFFF`, `BorderThickness=1`, `CornerRadius=8`, `Padding="12,5"`.
  - `TextBlock`: `Foreground=White`, `FontWeight=SemiBold`, `FontSize = 10 + clamp(scale, 0.03, 0.10) * 140` (default scale `0.045` → `16.3px`, range `14.2–24px`), `VerticalAlignment=Center`.
- Positioning (`StatsHudWindow.cs:121-130`):
  - Screen: `Screens.Primary ?? Screens.All[0]`.
  - `Position = (b.X + (int)(_xf * max(0, b.Width - size.Width)), b.Y + (int)(_yf * max(0, b.Height - size.Height)))` where `_xf`, `_yf` default to `0.5`, `0.02`.

### 3.3 Toast Overlay Window (`Services/ToastService.cs`)

- Frameless transparent topmost non-activating window (`ToastService.cs:123-140`):
  - `SystemDecorations=None`, `TransparencyLevelHint=[Transparent]`, `Background=Transparent`, `Topmost=True`, `ShowInTaskbar=False`, `ShowActivated=False`, `CanResize=False`, `Focusable=False`, `SizeToContent=WidthAndHeight`, `Opacity=0`, `DoubleTransition Opacity 220ms`.
  - On `Opened`: stamps `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` via `SetWindowLong(GWL_EXSTYLE)` (`ToastService.cs:148-152`).
  - Position: top-right of `Primary.WorkingArea` (`ScreenFromWindow` fallback) with `24px` physical-pixel inset: `(wa.X + wa.Width - ceil(bounds.Width * scaling) - 24, wa.Y + 24)` (`ToastService.cs:155-161`).
- Visual tree (`ToastService.cs:77-121`):
  - Outer `Border`: `Background` = active theme `CardBrush` with alpha forced to `0xF0` (fallback `#F0181C23`), `BorderBrush` = `PopupBorderBrush` (fallback `#1FFFFFFF`), `BorderThickness=1`, `CornerRadius=12`, `Padding="16,11"`.
  - Horizontal `StackPanel` (`Spacing=10`, `VerticalAlignment=Center`):
    - `MaterialIcon` (`20×20`, `Kind=icon`, `Foreground` = resource `accentKey` fallback `#22D3EE`).
    - Vertical `StackPanel` (`Spacing=2`):
      - Title `TextBlock`: `FontSize=13`, `FontWeight=Medium`, `Foreground` = `FgBrush` (fallback `#E7EAEE`).
      - Optional Subtitle `TextBlock` (only when non-whitespace): `FontSize=11`, `Foreground` = `Fg2Brush` (fallback `#9AA3AD`), `MaxWidth=260`, `TextTrimming=CharacterEllipsis`.
- Animation timing (`ToastService.cs:163-170`): Fade in to `Opacity=1` (`220ms`), stay visible `2400ms`, fade out to `Opacity=0`, close + dequeue next after `260ms`.

### 3.4 System Tray Icon & Menu (`App.axaml:332-349`, `App.axaml.cs:322-424`)

- `TrayIcon`: `Icon="avares://Lag/Assets/icon.ico"`, `ToolTipText="Lag"`, `Clicked="TrayIcon_OpenClicked"`.
- `NativeMenu` items (localized at startup via `LocalizeTrayMenu`):
  1. `Tray_SaveReplay` ("Save Replay") → `TrayIcon_SaveReplayClicked`
  2. `Tray_OpenLibrary` ("Open Library") → `TrayIcon_OpenLibraryClicked`
  3. `NativeMenuItemSeparator`
  4. `Tray_Open` ("Open") → `TrayIcon_OpenClicked`
  5. `Tray_Exit` ("Exit") → `TrayIcon_ExitClicked`

### 3.5 Full Design Token Table (`Themes/Tokens.axaml`, `Themes/Palette.axaml`, `App.axaml`)

#### Theme-Invariant Tokens (`Themes/Tokens.axaml` & `App.axaml`)

| Token Key | Type | Value | Source |
|---|---|---|---|
| `FontUi` | `FontFamily` | `Inter` | `Tokens.axaml:13` |
| `FontDisplay` | `FontFamily` | `Inter` | `Tokens.axaml:14` |
| `FontMono` | `FontFamily` | `Cascadia Code, Consolas, Courier New, monospace` | `Tokens.axaml:15` |
| `RadCard` | `CornerRadius` | `15` | `Tokens.axaml:18` |
| `RadGeneral` | `CornerRadius` | `13` | `Tokens.axaml:19` |
| `RadPill` | `CornerRadius` | `11` | `Tokens.axaml:20` |
| `RadButton` | `CornerRadius` | `10` | `Tokens.axaml:21` |
| `RadInput` | `CornerRadius` | `9` | `Tokens.axaml:22` |
| `RadSeg` | `CornerRadius` | `7` | `Tokens.axaml:23` |
| `RadKbd` | `CornerRadius` | `6` | `Tokens.axaml:24` |
| `LogoArcGeometry` | `StreamGeometry` | `M2.75 11C2.75 12.6317 3.23385 14.2267 4.14038 15.5835C5.0469 16.9402 6.33537 17.9976 7.84286 18.622C9.35035 19.2464 11.0092 19.4098 12.6095 19.0915C14.2098 18.7732 15.6798 17.9874 16.8336 16.8336C17.9874 15.6798 18.7732 14.2098 19.0915 12.6095C19.4098 11.0092 19.2464 9.35035 18.622 7.84286C17.9976 6.33537 16.9402 5.0469 15.5835 4.14038C14.2267 3.23385 12.6317 2.75 11 2.75C8.69362 2.75868 6.47989 3.65862 4.82167 5.26167L2.75 7.33333` | `Tokens.axaml:28` |
| `LogoCornerGeometry` | `StreamGeometry` | `M2.75 2.75V7.33333H7.33333` | `Tokens.axaml:29` |
| `LogoHandsGeometry` | `StreamGeometry` | `M11 6.41667V11L14.6667 12.8333` | `Tokens.axaml:30` |
| `OverlayCornerRadius` | `CornerRadius` | `10` | `App.axaml:226` |
| `ComboBoxDropdownBorderPadding` | `Thickness` | `4` | `App.axaml:227` |
| `MenuFlyoutPresenterThemePadding` | `Thickness` | `4` | `App.axaml:228` |
| `SliderHorizontalHeight` | `Double` | `20` | `App.axaml:232` |
| `SliderHorizontalThumbWidth` | `Double` | `12` | `App.axaml:233` |
| `SliderHorizontalThumbHeight` | `Double` | `12` | `App.axaml:234` |
| `SliderTrackThemeHeight` | `Double` | `4` | `App.axaml:235` |
| `SliderPreContentMargin` | `GridLength` | `4` | `App.axaml:236` |
| `SliderPostContentMargin` | `GridLength` | `4` | `App.axaml:237` |
| `SliderThumbCornerRadius` | `CornerRadius` | `6` | `App.axaml:238` |
| `SliderThumbBackground` | `SolidColorBrush` | `White` | `App.axaml:239` |
| `SliderThumbBackgroundPointerOver` | `SolidColorBrush` | `White` | `App.axaml:240` |
| `SliderThumbBackgroundPressed` | `SolidColorBrush` | `White` | `App.axaml:241` |
| `SliderThumbBackgroundDisabled` | `SolidColorBrush` | `White` | `App.axaml:242` |
| `SliderContainerBackground` | `SolidColorBrush` | `Transparent` | `App.axaml:243` |
| `SliderContainerBackgroundPointerOver` | `SolidColorBrush` | `Transparent` | `App.axaml:244` |
| `SliderContainerBackgroundPressed` | `SolidColorBrush` | `Transparent` | `App.axaml:245` |
| `SliderContainerBackgroundDisabled` | `SolidColorBrush` | `Transparent` | `App.axaml:246` |
| `SliderTrackValueFillDisabled` | `SolidColorBrush` | `#22D3EE` | `App.axaml:247` |
| `SliderTrackFillDisabled` | `SolidColorBrush` | `#24FFFFFF` | `App.axaml:248` |

#### Language Vector Flags (`App.axaml:23-214`, `30×20` geometry space)

- `Flag_en`: `#012169` rect, `White` (`4.5`) + `#C8102E` (`1.8`) diagonals, `White` (`6`) + `#C8102E` (`3.4`) cross.
- `Flag_uk`: `#0057B7` top half, `#FFD700` bottom half.
- `Flag_de`: horizontal thirds `#000000`, `#DD0000`, `#FFCE00`.
- `Flag_fr`: vertical thirds `#002395`, `#FFFFFF`, `#ED2939`.
- `Flag_be`: horizontal thirds `#FFFFFF`, `#E10000`, `#FFFFFF`.
- `Flag_lt`: horizontal thirds `#FDB913`, `#006A44`, `#C1272D`.
- `Flag_et`: horizontal thirds `#0072CE`, `#000000`, `#FFFFFF`.
- `Flag_lv`: horizontal `2:1:2` (`y=0..8, 8..12, 12..20`) `#9E3039`, `#FFFFFF`, `#9E3039`.
- `Flag_fi`: `#FFFFFF` field, `#002F6C` cross (`x=11, y=10`, thickness `5`).
- `Flag_sv`: `#006AA7` field, `#FECC00` cross (`x=11, y=10`, thickness `4`).
- `Flag_no`: `#BA0C2F` field, `#FFFFFF` cross (`5.5`) + `#00205B` cross (`2.6`) at `x=11, y=10`.
- `Flag_da`: `#C8102E` field, `#FFFFFF` cross (`x=11, y=10`, thickness `4`).
- `Flag_nl`: horizontal thirds `#AE1C28`, `#FFFFFF`, `#21468B`.
- `Flag_it`: vertical thirds `#008C45`, `#FFFFFF`, `#CD212A`.
- `Flag_es`: horizontal `1:2:1` (`y=0..5, 5..15, 15..20`) `#AA151B`, `#F1BF00`, `#AA151B`.
- `Flag_pt`: vertical `2:3` (`x=0..12, 12..30`) `#006600`, `#FF0000` + `#FFE900` circle (`center=12,10 r=3.6`).
- `Flag_ja`: `#FFFFFF` field + `#BC002D` circle (`center=15,10 r=6`).

#### Themed Colour & Brush Palette (`Themes/Palette.axaml:21-285`)

All values are ARGB `#AARRGGBB` (or `#RRGGBB` opaque).

| Key | Dark (`Palette.axaml:21-152`) | Light (`Palette.axaml:155-284`) | Type |
|---|---|---|---|
| `AccentColor` | `#FF22D3EE` | `#FFA9722E` | `Color` |
| `Accent` | `#FF22D3EE` | `#FFA9722E` | `SolidColorBrush` |
| `AccentDim` | `#1F22D3EE` | `#29A9722E` | `SolidColorBrush` |
| `AccentRing` | `#5222D3EE` | `#5CA9722E` | `SolidColorBrush` |
| `AccentHover` | `#8022D3EE` | `#FF946228` | `SolidColorBrush` |
| `AccentText` | `#FF7BE7F5` | `#FF7C5320` | `SolidColorBrush` |
| `OnAccent` | `#FF06222A` | `#FFFBF4E8` | `SolidColorBrush` |
| `SurfaceApp` | `#FF0C0D11` | `#FFEAE3D1` | `SolidColorBrush` |
| `SurfaceCard` | `#FF14161C` | `#FFF8F4EA` | `SolidColorBrush` |
| `SurfaceInput` | `#FF1A1D25` | `#FFE2D6BA` | `SolidColorBrush` |
| `SurfaceHover` | `#FF1E222B` | `#FFE2DAC4` | `SolidColorBrush` |
| `SurfaceHover2` | `#FF22262F` | `#FFDCD3BB` | `SolidColorBrush` |
| `BorderFaint` | `#0EFFFFFF` | `#0F2A2415` | `SolidColorBrush` |
| `BorderCard` | `#12FFFFFF` | `#182A2415` | `SolidColorBrush` |
| `BorderInput` | `#17FFFFFF` | `#242A2415` | `SolidColorBrush` |
| `BorderStrong` | `#1AFFFFFF` | `#302A2415` | `SolidColorBrush` |
| `TextHi` | `#FFECEDEF` | `#FF2A2820` | `SolidColorBrush` |
| `TextRow` | `#FFDDE0E4` | `#FF36332A` | `SolidColorBrush` |
| `TextBtn` | `#FFD7DADE` | `#FF3D3A30` | `SolidColorBrush` |
| `TextKbd` | `#FFC4C8CE` | `#FF4A4638` | `SolidColorBrush` |
| `TextMid` | `#FF9AA0AA` | `#FF6E6A5A` | `SolidColorBrush` |
| `TextMuted` | `#FF8B919C` | `#FF807B69` | `SolidColorBrush` |
| `TextFaint` | `#FF7C828D` | `#FF928D78` | `SolidColorBrush` |
| `TextFaintest` | `#FF6A707B` | `#FFA39D86` | `SolidColorBrush` |
| `DotDivider` | `#FF4A505B` | `#FFB7AF98` | `SolidColorBrush` |
| `DangerClose` | `#FFE5484D` | `#FFD23A40` | `SolidColorBrush` |
| `DangerStop` | `#FFEF4D62` | `#FFD93B50` | `SolidColorBrush` |
| `RecDot` | `#FFFF5C5C` | `#FFE5484D` | `SolidColorBrush` |
| `StatusIdle` | `#FF5D646C` | `#FFA99F88` | `SolidColorBrush` |
| `SidebarBrush` | `#FF0C0E11` | `#FFEAE3D1` | `SolidColorBrush` |
| `MainBrush` | `#FF12151A` | `#FFF1EBDC` | `SolidColorBrush` |
| `SurfaceBrush` | `#FF20252D` | `#FFEBE4D2` | `SolidColorBrush` |
| `SurfaceHoverBrush` | `#FF272D36` | `#FFE2DAC4` | `SolidColorBrush` |
| `CardBrush` | `#FF181C23` | `#FFF8F4EA` | `SolidColorBrush` |
| `CardHoverBrush` | `#FF1C2129` | `#FFFCF8EF` | `SolidColorBrush` |
| `SegSelBrush` | `#FF2A313B` | `#FFF8F4EA` | `SolidColorBrush` |
| `VideoBrush` | `#FF07090C` | `#FF16140E` | `SolidColorBrush` |
| `AccentBrush` | `#FF22D3EE` | `#FFA9722E` | `SolidColorBrush` |
| `AccentHoverBrush` | `#FF4ADDF2` | `#FF946228` | `SolidColorBrush` |
| `OnAccentBrush` | `#FF0A0C0F` | `#FFFBF4E8` | `SolidColorBrush` |
| `DangerBrush` | `#FFF0464C` | `#FFD83E44` | `SolidColorBrush` |
| `DangerHoverBrush` | `#FFF35F64` | `#FFC5353B` | `SolidColorBrush` |
| `FgBrush` | `#FFE7EAEE` | `#FF2A2820` | `SolidColorBrush` |
| `Fg2Brush` | `#FF9AA3AD` | `#FF6E6A5A` | `SolidColorBrush` |
| `Fg3Brush` | `#FF6A7380` | `#FFA39D86` | `SolidColorBrush` |
| `EmptyIconBrush` | `#FF3E4754` | `#FFC9C2AC` | `SolidColorBrush` |
| `Divider6Brush` | `#0FFFFFFF` | `#142A2415` | `SolidColorBrush` |
| `Divider5Brush` | `#0DFFFFFF` | `#112A2415` | `SolidColorBrush` |
| `Overlay85Brush` | `#D90A0C0F` | `#D90A0C0F` | `SolidColorBrush` |
| `Overlay88Brush` | `#E00A0C0F` | `#E00A0C0F` | `SolidColorBrush` |
| `PlayerBarBrush` | `#D10A0C0F` | `#EBF8F4EA` | `SolidColorBrush` |
| `HoverVeilBrush` | `#0AFFFFFF` | `#0C2A2415` | `SolidColorBrush` |
| `Track14Brush` | `#24FFFFFF` | `#262A2415` | `SolidColorBrush` |
| `VolumeFillBrush` | `#FFAEB6C0` | `#FF8F8770` | `SolidColorBrush` |
| `StatusReadyBrush` | `#FF22D3EE` | `#FFA9722E` | `SolidColorBrush` |
| `StatusRecordingBrush` | `#FFF0464C` | `#FFD83E44` | `SolidColorBrush` |
| `TextPrimaryBrush` | `#FFE7EAEE` | `#FF2A2820` | `SolidColorBrush` |
| `TextSecondaryBrush` | `#FF9AA3AD` | `#FF6E6A5A` | `SolidColorBrush` |
| `TextDimBrush` | `#FF6A7380` | `#FFA39D86` | `SolidColorBrush` |
| `BgDarkBrush` | `#FF12151A` | `#FFF1EBDC` | `SolidColorBrush` |
| `AccentCyanBrush` | `#FF22D3EE` | `#FFA9722E` | `SolidColorBrush` |
| `NavHoverBrush` | `#FF14181E` | `#FFE2DAC4` | `SolidColorBrush` |
| `SegHoverBrush` | `#FF20262E` | `#FFE2DAC4` | `SolidColorBrush` |
| `PopupBorderBrush` | `#1FFFFFFF` | `#242A2415` | `SolidColorBrush` |
| `TextSelectionBrush` | `#5922D3EE` | `#5CA9722E` | `SolidColorBrush` |
| `TransportHoverBrush` | `#1AFFFFFF` | `#1A2A2415` | `SolidColorBrush` |
| `ScrollThumbBrush` | `#59FFFFFF` | `#552A2415` | `SolidColorBrush` |
| `ScrollThumbPressedBrush` | `#8CFFFFFF` | `#882A2415` | `SolidColorBrush` |
| `ToggleSwitchFillOn` | `#22D3EE` | `#A9722E` | `SolidColorBrush` |
| `ToggleSwitchFillOnPointerOver` | `#4ADDF2` | `#B5803A` | `SolidColorBrush` |
| `ToggleSwitchFillOnPressed` | `#22D3EE` | `#A9722E` | `SolidColorBrush` |
| `ToggleSwitchStrokeOn` | `#22D3EE` | `#A9722E` | `SolidColorBrush` |
| `ToggleSwitchStrokeOnPointerOver` | `#4ADDF2` | `#B5803A` | `SolidColorBrush` |
| `ToggleSwitchStrokeOnPressed` | `#22D3EE` | `#A9722E` | `SolidColorBrush` |
| `ToggleSwitchKnobFillOn` | `White` | `White` | `SolidColorBrush` |
| `ToggleSwitchKnobFillOnPointerOver` | `White` | `White` | `SolidColorBrush` |
| `ToggleSwitchKnobFillOnPressed` | `White` | `White` | `SolidColorBrush` |
| `ToggleSwitchFillOff` | `#20252D` | `#DCD2B8` | `SolidColorBrush` |
| `ToggleSwitchFillOffPointerOver` | `#272D36` | `#D3C8AB` | `SolidColorBrush` |
| `ToggleSwitchFillOffPressed` | `#272D36` | `#D3C8AB` | `SolidColorBrush` |
| `ToggleSwitchStrokeOff` | `#20252D` | `#C9BFA2` | `SolidColorBrush` |
| `ToggleSwitchStrokeOffPointerOver` | `#272D36` | `#BFB495` | `SolidColorBrush` |
| `ToggleSwitchStrokeOffPressed` | `#272D36` | `#BFB495` | `SolidColorBrush` |
| `ToggleSwitchKnobFillOff` | `#9AA3AD` | `#8A8068` | `SolidColorBrush` |
| `ToggleSwitchKnobFillOffPointerOver` | `#B6BEC7` | `#756B54` | `SolidColorBrush` |
| `ToggleSwitchKnobFillOffPressed` | `#B6BEC7` | `#756B54` | `SolidColorBrush` |
| `ComboBoxDropDownBackground` | `#181C23` | `#F8F4EA` | `SolidColorBrush` |
| `ComboBoxDropDownBorderBrush` | `#1FFFFFFF` | `#242A2415` | `SolidColorBrush` |
| `ComboBoxDropDownGlyphForeground` | `#9AA3AD` | `#6E6A5A` | `SolidColorBrush` |
| `MenuFlyoutPresenterBackground` | `#181C23` | `#F8F4EA` | `SolidColorBrush` |
| `MenuFlyoutPresenterBorderBrush` | `#1FFFFFFF` | `#242A2415` | `SolidColorBrush` |
| `MenuFlyoutItemBackground` | `Transparent` | `Transparent` | `SolidColorBrush` |
| `MenuFlyoutItemBackgroundPointerOver` | `#272D36` | `#E2DAC4` | `SolidColorBrush` |
| `MenuFlyoutItemBackgroundPressed` | `#2A313B` | `#DCD3BB` | `SolidColorBrush` |
| `MenuFlyoutItemForeground` | `#E7EAEE` | `#2A2820` | `SolidColorBrush` |
| `MenuFlyoutItemForegroundPointerOver` | `#E7EAEE` | `#2A2820` | `SolidColorBrush` |
| `MenuFlyoutItemForegroundPressed` | `#E7EAEE` | `#2A2820` | `SolidColorBrush` |
| `MenuFlyoutSeparatorBackground` | `#0FFFFFFF` | `#142A2415` | `SolidColorBrush` |
| `ToolTipBackground` | `#20252D` | `#EBE4D2` | `SolidColorBrush` |
| `ToolTipForeground` | `#E7EAEE` | `#2A2820` | `SolidColorBrush` |
| `ToolTipBorderBrush` | `#1FFFFFFF` | `#242A2415` | `SolidColorBrush` |
| `SystemAccentColor` | `#FF22D3EE` | `#FFA9722E` | `Color` |
| `SystemAccentColorLight1` | `#FF4DDDF2` | `#FFBC8A45` | `Color` |
| `SystemAccentColorLight2` | `#FF7BE7F5` | `#FFCDA268` | `Color` |
| `SystemAccentColorLight3` | `#FFA7F0F9` | `#FFDDBC92` | `Color` |
| `SystemAccentColorDark1` | `#FF18B8D6` | `#FF946228` | `Color` |
| `SystemAccentColorDark2` | `#FF129BB8` | `#FF7C5320` | `Color` |
| `SystemAccentColorDark3` | `#FF0C7E96` | `#FF614019` | `Color` |

Hardcoded converter colours (`Core/UiConverters.cs:44-45`):
- `IntensityToBrushConverter.Caution`: `#E5B84C` (amber, tier 1)
- `IntensityToBrushConverter.Extreme`: `#E0564C` (red, tier >= 2)

#### Reusable Control Styles

##### Custom ToggleSwitch (`App.axaml:256-320`)
- Track `Border`: `40×22`, `CornerRadius=11`, `BorderThickness=1`, `Background={ToggleSwitchFillOff}`, `BorderBrush={ToggleSwitchStrokeOff}`, `BrushTransition Background/BorderBrush 0.16s`.
- Knob `Ellipse`: `16×16`, `Margin="2,0,0,0"`, `Fill={ToggleSwitchKnobFillOff}`, `RenderTransform=translateX(0px)`, `TransformOperationsTransition RenderTransform 0.16s CubicEaseOut`, `BrushTransition Fill 0.16s`.
- `:pointerover`: Track uses `ToggleSwitchFillOffPointerOver` / `ToggleSwitchStrokeOffPointerOver`; Knob uses `ToggleSwitchKnobFillOffPointerOver`.
- `:checked`: Track uses `ToggleSwitchFillOn` / `ToggleSwitchStrokeOn`; Knob translates `translateX(18px)` and uses `ToggleSwitchKnobFillOn`.
- `:checked:pointerover`: Track uses `ToggleSwitchFillOnPointerOver` / `ToggleSwitchStrokeOnPointerOver`; Knob uses `ToggleSwitchKnobFillOnPointerOver`.
- `:disabled`: `Opacity=0.45`.

##### Redesign Component Styles (`Themes/Components.axaml:12-135`)
- `Border.rcard`: `Background={SurfaceCard}`, `BorderBrush={BorderCard}`, `BorderThickness=1`, `CornerRadius={RadCard}` (`15`), `ClipToBounds=True`.
- `Border.rdivider`: `Height=1`, `Background={BorderFaint}`.
- `TextBlock.rgroup`: `FontFamily={FontMono}`, `FontSize=11`, `Foreground={TextFaintest}`, `Margin="2,0,0,11"`.
- `TextBlock.rrow`: `FontSize=14`, `FontWeight=Medium`, `Foreground={TextRow}`.
- `TextBlock.rsub`: `FontSize=12.5`, `Foreground={TextFaint}`, `TextWrapping=Wrap`.
- `TextBlock.rmono`: `FontFamily={FontMono}`, `Foreground={TextFaint}`.
- `Border.rkbd`: `Background={SurfaceInput}`, `BorderBrush={BorderStrong}`, `BorderThickness=1`, `CornerRadius={RadKbd}` (`6`), `Padding="9,4"`; child `TextBlock`: `FontFamily={FontMono}`, `FontSize=11.5`, `Foreground={TextKbd}`.
- `Border.rsegtrack`: `Background={SurfaceInput}`, `CornerRadius={RadInput}` (`9`), `Padding=3`.
- `Button.rseg`: `Background=Transparent`, `Foreground={TextMid}`, `CornerRadius={RadSeg}` (`7`), `Padding="12,6"`, `FontSize=12.5`, `FontWeight=Medium`, `BorderThickness=1`, `BorderBrush=Transparent`, `Cursor=Hand`; `ContentPresenter` `BrushTransition Background 0.12s`; `:pointerover` → `Foreground={TextHi}`; `.sel` → `Foreground={AccentText}`, `BorderBrush={AccentRing}`, `ContentPresenter Background={AccentDim}`.
- `Border.rpill`: `Background={SurfaceInput}`, `BorderBrush={BorderInput}`, `BorderThickness=1`, `CornerRadius={RadInput}` (`9`), `Padding="12,8"`.
- `Button.rchip`: `Background={SurfaceInput}`, `Foreground={TextHi}`, `BorderThickness=1`, `BorderBrush={BorderStrong}`, `CornerRadius={RadInput}` (`9`), `Padding="15,8"`, `FontSize=12.5`, `FontWeight=SemiBold`, `Cursor=Hand`; `ContentPresenter` `BrushTransition Background 0.14s`; `:pointerover` → `{SurfaceHover2}`.
- `FlyoutPresenter`: `Background={SurfaceCard}`, `BorderBrush={BorderStrong}`, `BorderThickness=1`, `CornerRadius={RadGeneral}` (`13`), `Padding=8`.

##### Shared & Legacy Styles (`Themes/DarkTheme.axaml:16-755`)
- `TextBlock` (global): `Foreground={FgBrush}`, `FontFamily="Inter, Segoe UI, sans-serif"`.
- `Button` (global): `RenderTransform=scale(1)`, `BrushTransition Background 0.15s`, `TransformOperationsTransition RenderTransform 0.08s`; `:pressed` → `scale(0.97)`.
- `Button.cta` (used in `EditorView`): `Background={AccentBrush}`, `Foreground={OnAccentBrush}`, `Height=36`, `CornerRadius=8`, `FontSize=13`, `FontWeight=Medium`, `BorderThickness=0`, `Cursor=Hand`; `:pointerover` → `{AccentHoverBrush}`; `.rec` → `{DangerBrush}` + `White`, `:pointerover` → `{DangerHoverBrush}`.
- `Button.chip` (used in `EditorView`): `Background={SurfaceBrush}`, `Foreground={FgBrush}`, `CornerRadius=8`, `Padding="12,7"`, `FontSize=13`, `BorderThickness=0`, `Cursor=Hand`; `:pointerover` → `{SurfaceHoverBrush}`; `.danger` → `{DangerBrush}` + `White`, `:pointerover` → `{DangerHoverBrush}`.
- `ListBox.segctl`: `Background={SurfaceInput}`, `CornerRadius=8`, `Padding=3`, `BorderThickness=0`, horizontal `StackPanel` (`Spacing=2`). `ListBoxItem`: `Padding="10,4"`, `CornerRadius=6`, `MinHeight=0`, `FontSize=12`, `Foreground={Fg2Brush}`, `Cursor=Hand`, `RenderTransform=scale(1)`, transitions `Foreground 0.15s`, `RenderTransform 0.08s`; `:pressed` → `scale(0.95)`; `ContentPresenter`: `Transparent`, `CornerRadius=6`, `BrushTransition Background 0.15s`, `:pointerover` → `{SegHoverBrush}`, `:selected` → `Transparent` (pill is drawn by `SegmentedListBox`), `Foreground={FgBrush}`, `FontWeight=Medium`.
- `CheckBox.app`: `Cursor=Hand`; `StackPanel` (`Horizontal`, `Spacing=10`) with `16×16` `Border` (`CornerRadius=4`, `Background={SurfaceBrush}`, `BrushTransition Background 0.15s`) + check `Path` (`M2 5.5 L4.2 8 L8.5 2.5`, `Stroke={OnAccentBrush}`, `1.8px`, `10×10`, `Opacity=0`, `scale(0.4)`, transitions `Opacity 0.12s`, `RenderTransform 0.15s CubicEaseOut`). `:checked`: box `{AccentBrush}`, check `Opacity=1`, `scale(1)`. `CheckBox.app TextBlock`: `FontSize=13`, `Foreground={Fg2Brush}`, `BrushTransition Foreground 0.15s`; `:checked` → `{FgBrush}`.
- `Border.kbd`: `Background={SurfaceBrush}`, `CornerRadius=6`, `Padding="8,4"`; `TextBlock`: `FontSize=12`, `FontWeight=Medium`, `Foreground={FgBrush}`. `.listening`: infinite `1s` `SineEaseInOut` opacity pulse `1 → 0.45 → 1`.
- `Button.bare:disabled`: `ContentPresenter Background=Transparent`, `Opacity=0.45`.
- `ComboBox`: `Background={SurfaceBrush}`, `Foreground={FgBrush}`, `CornerRadius=8`, `MinHeight=30`, `Padding="12,5,8,5"`, `FontSize=13`, `BorderThickness=0`, `PlaceholderForeground={Fg2Brush}`; `Border#Background` `BrushTransition Background 0.15s`, `:pointerover` → `{SurfaceHoverBrush}`; `PathIcon#DropDownGlyph` `Foreground={Fg2Brush}`, rotates `0deg → 180deg` on `:dropdownopen` (`0.18s CubicEaseOut`).
- `ComboBoxItem`: `FontSize=13`, `Foreground={Fg2Brush}`, `Padding="10,6"`, `Margin="0,1"`, `CornerRadius=6`, `Cursor=Hand`; `ContentPresenter` `BrushTransition Background 0.12s`, `:pointerover` → `{SurfaceBrush}`, `:selected` → `Background={SegSelBrush}`, `TextBlock.Foreground={FgBrush}`.
- `ContextMenu` / `MenuFlyoutPresenter`: `Background={CardBrush}`, `BorderBrush={PopupBorderBrush}`, `BorderThickness=1`, `CornerRadius=10`, `Padding=4`.
- `MenuItem`: `Background=Transparent`, `Foreground={FgBrush}`, `FontSize=13`, `Padding="10,7"`, `CornerRadius=6`, `Margin="0,1"`, `Cursor=Hand`; `PART_LayoutRoot` `BrushTransition Background 0.12s`; `MaterialIcon` `{Fg2Brush}` (`BrushTransition 0.12s`), `:selected` → `{AccentBrush}`.
- `Separator`: `Background={Divider6Brush}`, `Height=1`, `Margin="8,4"`.
- `ToolTip`: `Background={SurfaceBrush}`, `Foreground={FgBrush}`, `BorderBrush={PopupBorderBrush}`, `BorderThickness=1`, `CornerRadius=8`, `Padding="10,6"`, `FontSize=12`.
- `TextBox`: `Background={SurfaceBrush}`, `Foreground={FgBrush}`, `CornerRadius=8`, `MinHeight=30`, `Padding="9,4"`, `FontSize=13`, `BorderThickness=1`, `BorderBrush=Transparent`, `CaretBrush={AccentBrush}`, `SelectionBrush={TextSelectionBrush}`; `PART_BorderElement` `BrushTransition Background/BorderBrush 0.15s`; `:pointerover` → `Background={SurfaceHoverBrush}`, `BorderBrush=Transparent`; `:focus` → `Background={SurfaceBrush}`, `BorderBrush={AccentBrush}`.
- `Slider`: `Foreground={AccentBrush}`, `Background={Track14Brush}`, `Cursor=Hand`, `:horizontal` `Height=20`, `MinHeight=20`; `:pointerover/:pressed` pin `PART_DecreaseButton` to `{AccentBrush}` and `PART_IncreaseButton` to `{Track14Brush}`; `Thumb` scales `1 → 1.25` on `:pointerover` (`0.12s CubicEaseOut`); `:disabled` `Opacity=0.45`. `Slider.volume`: uses `{VolumeFillBrush}` instead of `{AccentBrush}`.
- `ListBox.replays`: `Background=Transparent`, `BorderThickness=0`, `Padding=0`; `ListBoxItem`: `Padding=6`, `CornerRadius=8`, `Margin="0,0,0,8"`, `Background=Transparent`, `Cursor=Hand`; `ContentPresenter` `CornerRadius=8`, `BrushTransition Background 0.15s`, `:pointerover` → `{NavHoverBrush}`, `:selected` → `{SurfaceBrush}`.
- `Button.transport` (used in `EditorView`): `Background=Transparent`, `Foreground={FgBrush}`, `Padding=4`, `CornerRadius=6`, `BorderThickness=0`, `Cursor=Hand`, `Focusable=False`; `:pointerover` → `{TransportHoverBrush}` (`0.12s`).
- `ScrollViewer`: `AllowAutoHide=True`. `ScrollBar:vertical`: `Width=10`, `Background=Transparent`, custom `Thumb` (`Width=6`, `MinHeight=28`, `CornerRadius=3`, `Background=Transparent` at rest, `BrushTransition Background 0.15s`, `:pointerover` → `{ScrollThumbBrush}`, `:pressed` → `{ScrollThumbPressedBrush}`).
- `ProgressBar.buffer`: `Height=4`, `MinHeight=4`, `CornerRadius=2`, `Background={SurfaceBrush}`, `Foreground={AccentBrush}`, `DoubleTransition Value 0.45s CubicEaseOut`.
- **Dead / legacy styles in `DarkTheme.axaml`** (unreferenced in current `Views/*.axaml`): `TextBlock.section` (`DarkTheme.axaml:21-26`), `Border.divider` (`29-33`), `Button.surface` (`80-100`), `Button.nav` / `Button.nav-active` (`131-185`), `Button.winctl` (`188-215`), `Button.tab` (`218-252`), `Button.clipcard` (`629-675`; `LibraryView` uses `clipcardr`).

---

## 4. State & Logic

### 4.1 Startup Sequence (`Program.cs`, `App.axaml.cs`)

1. `Program.Main(string[] args)` (`Program.cs:59-186`):
   - `FileLog.Initialize()` creates `Documents\Lag\Logs\`, prunes `Lag-*.txt` keeping 15, opens `Lag-{yyyy-MM-dd_HH-mm-ss}.txt`, writes header, tees `Console.Out` and `Console.Error` (`FileLog.cs:41-61`).
   - `timeBeginPeriod(1)` sets 1 ms Windows timer resolution (`Program.cs:68`).
   - Checks 9 `LAG_*` diagnostic env vars in order; if any matches, runs test/probe and returns (`Program.cs:71-135`).
   - Registers `AppDomain.CurrentDomain.UnhandledException`, `TaskScheduler.UnobservedTaskException`, and native Win32 SEH filter `SetUnhandledExceptionFilter` (`Program.cs:140-146`).
   - Runs `VelopackApp.Build().Run()` (`Program.cs:155`).
   - Acquires named mutex `"LagAppSingleInstanceMutex"` (`Program.cs:159-175`); exits immediately if `createdNew == false` or `UnauthorizedAccessException`.
   - Builds Avalonia app (`UsePlatformDetect`, `WithInterFont`, `LogToTrace`, `AfterSetup → RaiseRenderTimerToDisplayRate` which replaces `IRenderTimer` with `DynamicRenderTimer` at `clamp(maxMonitorHz, 60, 240)` and logs tick rate verification at t=10s) and enters desktop lifetime (`Program.cs:179-270`).
2. `App.Initialize()` (`App.axaml.cs:32-57`):
   - Resolves `ffmpegDir = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "ffmpeg"))`.
   - Sets `Environment.CurrentDirectory = ffmpegDir`, `SetDllDirectory(ffmpegDir)`, `AddDllDirectory(ffmpegDir)`, prepends `ffmpegDir + ";"` to `PATH`, configures `GlobalFFOptions` `BinaryFolder = ffmpegDir`, calls `FfmpegInterop.Initialize(ffmpegDir)`.
   - Loads `App.axaml` (initial `RequestedThemeVariant="Dark"`), calls `SetLanguage("en")`.
3. `App.OnFrameworkInitializationCompleted()` (`App.axaml.cs:205-316`):
   - Registers singletons: `GlobalHotkeyManager`, `GlobalHotkeyService`, `HardwareDetector`, `VfrRecorderAdapter`, `IReplayRecorder` (forwarding to `VfrRecorderAdapter`), `SettingsViewModel`, `LibraryViewModel`, `PlayerViewModel`, `EditorViewModel`, `MainViewModel`.
   - Calls `PerformanceGuard.Apply()`.
   - Constructs `MainViewModel` and `MainWindow`.
   - If `mainViewModel.Settings.StartMinimized`: sets `WindowState = Minimized`, `ShowInTaskbar = false`, hooks one-shot `Opened` handler to `Hide()` and restore `ShowInTaskbar = true`.
   - Starts `GlobalHotkeyManager.StartAsync()` (SharpHook) and `GlobalHotkeyService.Start(0x0001, 0x79)` (Win32 Alt+F10, immediately updated by `SettingsViewModel` or wait—`SettingsViewModel` already ran during `GetRequiredService<MainViewModel>()`, see Section 9!).
   - Subscribes Win32 hotkey events (`HotkeyPressed`, `ScreenshotPressed`, `PausePressed`, `RecordPressed`) marshaling to UI thread and checking `mainViewModel.Settings.AreHotkeysSuppressed`.
   - Hooks `ShutdownRequested` → `FlushSettings()` + `_serviceProvider.Dispose()`.
   - Calls `LocalizeTrayMenu()`.

### 4.2 `MainViewModel` State & Commands (`ViewModels/MainViewModel.cs`)

- **Observable Properties**:
  - `CurrentView: ViewModelBase` (defaults to `Settings`, `MainViewModel.cs:190`)
  - `IsRecording: bool` (`MainViewModel.cs:38`)
  - `IsPaused: bool` (`MainViewModel.cs:43`)
  - `StatusText: string` (default `Localizer.Get("Status_Ready")`, `MainViewModel.cs:47`)
  - `BufferStatus: string` (default `"0:00"`, `MainViewModel.cs:51`)
  - `BufferFillPercent: double` (`0..100`, `MainViewModel.cs:55`)
  - `IsSaving: bool` (`MainViewModel.cs:148`)
  - `IsFullscreen: bool` (`MainViewModel.cs:156`)
  - `HasCaptureTarget: bool` (`MainViewModel.cs:160`)
  - `CaptureTargetIsGame: bool` (`MainViewModel.cs:164`)
  - `CaptureTargetName: string` (default `""`, `MainViewModel.cs:168`)
  - `EditableClips: ObservableCollection<ReplayClip>` (videos only, synced with `Library.Clips.CollectionChanged`, `MainViewModel.cs:30,287,394-399`)
- **Startup Side Effects in `MainViewModel` constructor** (`MainViewModel.cs:185-298`):
  - Subscribes `Settings.StatsHudChanged` → `UpdateStatsHud()`, posts initial `UpdateStatsHud()`.
  - Calls `LogStartupDiagnostics()` (logs version, GPUs, config string, `HardwareProfiler.Capture`, `PresetResolver.LogRecommendations`).
  - Subscribes `_engine.ReplaySaved`: sets `IsSaving = false`, `StatusText = Localizer.Format("Status_ReplaySaved", fileName)`, writes sidecar metadata via `ClipMetadataStore.Write` (`Game = carried?.Name ?? (target?.IsGame ? target.Name : null)`, `Exe = carried?.Exe ?? target?.Exe`), fires `Library.RefreshCommand`, shows silent toast `Toast_ReplaySaved` with `fileName`.
  - Subscribes `_engine.CaptureTargetChanged` → marshals `UpdateCaptureTarget()`.
  - Triggers initial `Library.RefreshCommand.ExecuteAsync(null)`.
  - Schedules `Task.Delay(3s)` → `Settings.AutoUpdateOnStartupAsync()`.
  - Subscribes `_hotkeyManager.HotkeyPressed` / `PttPressed` / `PttReleased`.
  - Subscribes `Library.PlayClipRequested` → `Player.LoadAndPlay(clip); CurrentView = Player;`, and `Library.EditClipRequested` → `OpenEditor(clip)`.
  - If `Settings.AutoStartRecording`, posts `StartRecordingCommand` at `Background` priority.
- **Commands**:
  - `NavigateToSettings`: if `CurrentView == Player`, calls `Player.StopPlayback()`; `CurrentView = Settings`.
  - `NavigateToLibrary`: if `CurrentView == Player`, calls `Player.StopPlayback()`; `CurrentView = Library`; fire-and-forget `Library.RefreshCommand`.
  - `NavigateToPlayer`: `CurrentView = Player`; fire-and-forget `Library.RefreshCommand`.
  - `NavigateToEditor`: if `CurrentView == Player`, calls `Player.StopPlayback()`; `CurrentView = Editor`; fire-and-forget `Library.RefreshCommand`.
  - `ToggleFullscreen`: `IsFullscreen = !IsFullscreen`.
  - `OpenEditorFromPlayer`: if `Player.CurrentClip != null`, calls `OpenEditor(clip)` (no-op if `clip.IsImage`).
  - `OpenEditorForClip(ReplayClip?)`: if non-null and `!clip.IsImage`, `Editor.OpenClip(clip); CurrentView = Editor`.
  - `TakeScreenshot`: runs `ScreenshotService.Capture(Settings.SelectedMonitor?.DeviceName, Settings.LibraryPath)` on `Task.Run`; if non-null path returned, shows toast `Toast_ScreenshotSaved` with filename and plays `Assets/screenshot.wav`.
  - `StartRecordingAsync`: if `IsRecording`, returns; awaits `_teardownTask` if still running; builds `RecorderOptions` snapshot (computing even `OutputWidth`/`OutputHeight` when `SelectedResolution.TargetHeight > 0 && < height`), runs `_engine.Initialize(options)` + `_engine.StartBuffer()` on `Task.Run`, sets `IsRecording = true`, `StatusText = Status_Recording`, starts 1 s buffer ticker, resets `_lastCaptureToast = null`, shows `Toast_RecordingStarted` (`RecordCircle`, `"RecDot"`). On error sets `StatusText = Localizer.Format("Status_RecordError", ex.Message)`.
  - `StopRecording`: if `!IsRecording`, returns; immediately sets `IsRecording = false`, `IsPaused = false`, `StatusText = Status_Stopping`, `_lastCaptureToast = null`, shows `Toast_RecordingStopped` (`StopCircle`, `"Fg2Brush"`), stops buffer ticker, clears `Settings.HasPendingChanges = false`, and runs `_engine.Teardown()` in `_teardownTask = Task.Run(...)`, posting `StatusText = Status_Stopped` on completion.
  - `ToggleRecordingAsync`: 1200 ms UTC debounce (`_lastToggleUtc`), calls `StopRecording()` or `StartRecordingAsync()`.
  - `TogglePauseRecording`: if `!IsRecording`, returns; toggles `IsPaused`, calls `_engine.SetPaused(pause)`, stops/resumes `_bufferTimer`, sets `StatusText` to `Status_Paused` / `Status_Recording`.
  - `SaveReplay`: if `!IsRecording` → sets `StatusText = Status_NotRecording` and returns; if `IsSaving` → returns; sets `IsSaving = true`, `StatusText = Status_Saving`, calls `BeginSaveFeedback()` (toast `Status_Saving` with `ContentSave` + `Assets/save.wav`), starts `120 s` save watchdog (`Status_SaveError("timeout")` if still saving), invokes `_engine.SaveReplay()` on `Task.Run`.

### 4.3 Buffer Ticker & Capture Target Logic (`MainViewModel.cs:57-144`)

- `_bufferTimer`: `1 s` `DispatcherTimer`, increments `_bufferElapsedSec` up to `_bufferTotalSec`, updates `BufferFillPercent = 100.0 * elapsed / total` and `BufferStatus = "{m}:{ss:00} / {M}:{SS:00}"`, stops at 100%.
- `UpdateCaptureTarget()`:
  - Reconciles `IsPaused` with `_engine.IsPaused` (auto target switch starts un-paused session).
  - If `_engine.CurrentTarget` is null: `HasCaptureTarget = false`.
  - Otherwise sets `CaptureTargetIsGame`, `CaptureTargetName` (`Name` if `IsGame` and non-blank, else localized `DesktopLabels[lang]`), `HasCaptureTarget = true`.
  - Fires `ToastService.Show(Localizer.Get("Toast_NowRecording"), CaptureTargetName, Controller/Monitor, "Accent")` only when `IsRecording && _lastCaptureToast != null && _lastCaptureToast != (IsGame, CaptureTargetName)`.

### 4.4 Theme Cross-Fade (`App.axaml.cs:89-199`)

- Maps `"light"` → `ThemeVariant.Light`, `"dark"` → `ThemeVariant.Dark`, anything else (`"system"`) → `ThemeVariant.Default`.
- If `MainWindow` is visible and `OverlayLayer` exists: renders current window pixels at `ClientSize * RenderScaling` into a `RenderTargetBitmap`, adds an `Image` (`IsHitTestVisible = false`) to `OverlayLayer`, sets `RequestedThemeVariant`. If `ActualThemeVariant` did not change, immediately removes overlay; otherwise posts `overlay.Opacity = 0` (`280 ms` `CubicEaseOut`) at `Background` priority and removes/disposes the overlay after `600 ms`.

### 4.5 Global Hotkeys (`GlobalHotkeyManager.cs`, `GlobalHotkeyService.cs`)

- **Win32 `GlobalHotkeyService`** (`GlobalHotkeyService.cs:13-373`):
  - Dedicated background thread `"Win32HotkeyThread"` storing its native `GetCurrentThreadId()`.
  - Registers up to 4 IDs (`HOTKEY_ID = 9000`, `SCREENSHOT_ID = 9001`, `PAUSE_ID = 9002`, `RECORD_ID = 9003`) with `RegisterHotKey(IntPtr.Zero, id, modifiers | MOD_NOREPEAT (0x4000), vk)`. Any combo with `vk == 0` is skipped (unbound).
  - If none of the 4 register, the thread exits immediately.
  - Standard Win32 `GetMessage` loop dispatches `WM_HOTKEY (0x0312)` events to C# events; `StopMessageLoop()` posts `WM_QUIT (0x0012)` and `Join(1000)` before re-registering on combo changes or `Dispose()`.
  - `VirtualKeyFromKeyCode` maps `VcA–VcZ` (`0x41–0x5A`), `Vc0–Vc9` (`0x30–0x39`), `VcF1–VcF24` (`0x70–0x87`), and named keys (`Space=0x20`, `Enter=0x0D`, `Escape=0x1B`, `Tab=0x09`, `Backspace=0x08`, `Delete=0x2E`, `Insert=0x2D`, `Home=0x24`, `End=0x23`, `PageUp=0x21`, `PageDown=0x22`, `Up=0x26`, `Down=0x28`, `Left=0x25`, `Right=0x27`, `PrintScreen=0x2C`); returns `0` if unmapped.
  - `ModifiersToWin32` maps Left/Right Ctrl (`0x0002`), Shift (`0x0004`), Alt (`0x0001`), Meta/Win (`0x0008`).
- **SharpHook `GlobalHotkeyManager`** (`GlobalHotkeyManager.cs:13-199`):
  - Uses `TaskPoolGlobalHook` (never blocks the low-level Win32 hook thread).
  - Feeds `KeystrokeTracker.Instance.OnKey` and `OnMouseButton` (mouse buttons only, never mouse move/drag).
  - When `IsCapturing == true`: ignores standalone modifiers (`Left/Right Shift/Ctrl/Alt/Meta`); on any other key sets `IsCapturing = false` and raises `HotkeyCaptured(key, modifiers, cleared: key == VcEscape)`.
  - When `PttEnabled && key == PttKey`: raises `PttPressed` on press (repeats allowed), `PttReleased` on release.
  - Checks `(modifiers & RequiredModifiers) == RequiredModifiers && key == RequiredKey` and raises `HotkeyPressed`.

### 4.6 Toast Queue (`Services/ToastService.cs`)

- Thread-safe via `Dispatcher.UIThread.Post`. Queue capped at 6 pending items (`if (_queue.Count >= 6) _queue.Dequeue()`).
- Plays sequentially (`_active` flag).
- Before creating window, checks `SHQueryUserNotificationState(out state) == 0 && state == 3` (`QUNS_RUNNING_D3D_FULL_SCREEN`). If true, skips visual window, logs to console, and calls `Done()`.

### 4.7 Stats HUD (`Views/StatsHudWindow.cs`, `SystemStatsSampler.cs`)

- `1 s` `DispatcherTimer` checks `SHQueryUserNotificationState == 3`: hides window if exclusive fullscreen, shows otherwise, updates text and repositions.
- `SystemStatsSampler`: internal `1 s` `System.Threading.Timer` samples:
  - CPU%: delta of `GetSystemTimes(idle, kernel, user)` → `clamp(100 * (dKernel + dUser - dIdle) / (dKernel + dUser), 0, 100)`.
  - GPU%: PDH counter `\GPU Engine(*engtype_3D)\Utilization Percentage` (`PDH_FMT_DOUBLE`), summed across all instances with `CStatus == 0`, clamped `0..100`.
  - RAM: `GlobalMemoryStatusEx` → `TotalGb = ullTotalPhys / 1073741824.0`, `UsedGb = (ullTotalPhys - ullAvailPhys) / 1073741824.0`.
- FPS: derived in `StatsHudWindow` from `(_engine.FramesCaptured - _lastFrames) / max(0.2, elapsedSec)`, rounded to integer, displayed as `"--"` on first tick, counter reset, or `0`.
- Formats (`InvariantCulture`):
  - `_detail == 0`: `"{fps} FPS"`
  - `_detail == 1`: `"{fps} FPS   CPU {cpu}%   GPU {gpu}%"`
  - `_detail >= 2`: `"{fps} FPS   CPU {cpu}%   GPU {gpu}%   RAM {used:0.0}/{total:0.0} GB"`

### 4.8 Core Utilities (`Core/*.cs`)

- `BoolToStringConverter`: splits `ConverterParameter` on `'|'` (expects 2 parts: `TrueText|FalseText`).
- `DeferredDispose.Later(IDisposable? d, int ms = 3000)`: marshals to UI thread if needed and disposes `d` after `ms` milliseconds via `DispatcherTimer.RunOnce` (swallows exceptions).
- `FxAnimations.SlideFadeIn(Control c, double fromY = 16, int ms = 240)`: detaches transitions, snaps `Opacity=0` and `translateY(fromY)`, then at `DispatcherPriority.Background` attaches `DoubleTransition(Opacity)` + `TransformOperationsTransition(RenderTransform)` (`CubicEaseOut`, `ms`) and sets `Opacity=1`, `translateY(0px)`.
- `LanguageToFlagConverter`: resolves `"Flag_{code}"` from `Application.Current` resources.
- `Localizer`: `Get(key)` looks up string in `Application.Current` resources (returns `key` if missing); `Format(key, args)` calls `string.Format(Get(key), args)`.
- `EqualsConverter`: `string.Equals(value?.ToString(), parameter?.ToString(), Ordinal)`.
- `OneMinusIntConverter`: two-way `1 - i` for integer index reversal (`[Mono, Stereo]` UI vs `0=stereo, 1=mono`).
- `IntensityToBrushConverter`: `>= 2` → `#E0564C`, `1` → `#E5B84C`, else `AvaloniaProperty.UnsetValue`.
- `StringToUpperConverter`: `value?.ToString()?.ToUpperInvariant()`.
- `LevelToWidthConverter`: `max(0, level * scale)` (`scale` parsed with `InvariantCulture`).
- `ViewModelBase`: `ObservableObject` with `Title` property.

---

## 5. Persistence & Formats

### 5.1 Log Files (`Services/FileLog.cs`)

- **Directory**: `%UserProfile%\Documents\Lag\Logs` (`Environment.SpecialFolder.MyDocuments` + `Lag\Logs`) (`FileLog.cs:30`).
- **Session log filename**: `Lag-yyyy-MM-dd_HH-mm-ss.txt` (`FileLog.cs:48`).
- **Retention**: On startup, enumerates `Lag-*.txt` ordered by `LastWriteTimeUtc` descending, skips 15, deletes older files (`FileLog.cs:46,140-149`).
- **Header format** (`FileLog.cs:52-54`):
  ```text
  ===== Lag log — yyyy-MM-dd HH:mm:ss =====
  Lag {InformationalVersion|AssemblyVersion|?} | OS {OSVersion} | 64-bit {Is64BitProcess} | CLR {Environment.Version} | machine {MachineName}
  ------------------------------------------------------------
  ```
- **Line format** (`FileLog.cs:89`): `[HH:mm:ss.fff] {line}` (UTF-8, `AutoFlush = true`).
- **Crash log**: `%UserProfile%\Documents\Lag\Logs\crash.log` (appended across sessions, `FileLog.cs:34,110-137`):
  - Managed (`source` = `"AppDomain"`, `"UnobservedTask"`, `"Main"`):
    ```text
    ───── yyyy-MM-dd HH:mm:ss [{source}] Lag {version} ─────
    {ex.ToString()}

    ```
  - Native SEH (`FileLog.cs:121-137`):
    ```text
    ───── yyyy-MM-dd HH:mm:ss [NATIVE] Lag {version} ─────
    NATIVE CRASH {ACCESS_VIOLATION|STACK_OVERFLOW|STACK_BUFFER_OVERRUN|INT_DIVIDE_BY_ZERO|ILLEGAL_INSTRUCTION|BREAKPOINT|EXCEPTION} (0x{code:X8}) at 0x{addr:X16} in {ModuleName+0xOffset | 0x... (unknown module)}

    ```
    Mapped SEH codes: `0xC0000005`, `0xC00000FD`, `0xC0000409`, `0xC0000094`, `0xC000001D`, `0x80000003`.

### 5.2 Auto-Update Detection & Installer Download (`Services/AppUpdateService.cs`)

- **Deployment Detection (`DetectInstall`)** (`AppUpdateService.cs:44-75`):
  1. `LegacyVelopack`: `AppContext.BaseDirectory` ends with `"current"` (case-insensitive) and `..\Update.exe` exists.
  2. `Installed`: `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag_is1` or `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag_is1` has string value `InstallLocation` matching `AppContext.BaseDirectory` (case-insensitive, trailing `\` trimmed).
  3. `DevOrPortable`: anything else (never auto-updates).
- **GitHub Releases API** (`AppUpdateService.cs:38-39,96-120,155-161`):
  - `GET https://api.github.com/repos/shkbb/Lag/releases/latest` with `User-Agent: Lag-Updater` and 5-minute timeout.
  - Parses JSON `tag_name` (strips leading `v`/`V`, parses `Version`, zero-pads missing parts to 4 components) and `assets[]` looking for `name == "Lag-win-Setup.exe"` (case-insensitive) with non-empty `browser_download_url`.
- **Installer Download & Launch** (`AppUpdateService.cs:128-153`):
  - Downloads to `%TEMP%\Lag-Setup-{Version}.exe`.
  - Starts with `UseShellExecute = true` and arguments: `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /AUTOSTART`, then caller calls `Environment.Exit(0)` (`SettingsViewModel.cs:1426`).

### 5.3 Localization Dictionaries (`Assets/Langs/*.axaml`)

- **Format**: Avalonia XAML `ResourceDictionary` (`xmlns="https://github.com/avaloniaui"`, `xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"`, `xmlns:sys="using:System"`) containing `<sys:String x:Key="...">...</sys:String>` entries.
- **Supported codes (17)** (`App.axaml.cs:64-65`): `en`, `uk`, `de`, `fr`, `be`, `lt`, `et`, `lv`, `fi`, `sv`, `no`, `da`, `nl`, `it`, `es`, `pt`, `ja`.
- **All 235 keys in `Assets/Langs/en.axaml` (with exact English values)**:

| Key | English Value |
|---|---|
| `App_Tagline` | `Background screen recording` |
| `Nav_Header` | `NAVIGATION` |
| `Nav_Settings` | `Settings` |
| `Nav_Library` | `Library` |
| `Nav_Player` | `Player` |
| `Btn_StartRecording` | `Start Recording` |
| `Btn_StopRecording` | `Stop Recording` |
| `Btn_SaveReplay` | `Save Replay` |
| `Settings_Title` | `Settings` |
| `Settings_PendingChanges` | `Changes will take effect after restarting the recording.` |
| `Settings_Group_Language` | `LANGUAGE` |
| `Settings_Language` | `Language` |
| `Settings_Theme` | `Theme` |
| `Settings_Theme_Desc` | `Light, dark, or follow your system` |
| `Theme_Light` | `Light` |
| `Theme_Dark` | `Dark` |
| `Theme_System` | `System` |
| `Settings_Group_Recording` | `RECORDING` |
| `Settings_BufferLength` | `Replay buffer length` |
| `Settings_Display` | `Display` |
| `Settings_Microphone` | `Microphone` |
| `Settings_FrameRate` | `Frame rate (FPS)` |
| `Settings_MicVolume` | `Mic volume` |
| `Settings_OutputResolution` | `Output resolution` |
| `Settings_Codec` | `Video codec` |
| `Settings_AutoCleanup` | `Auto-clean library` |
| `Settings_AutoCleanup_Desc` | `Automatically delete the oldest clips when the library exceeds the limit` |
| `Settings_StorageLimit` | `Library size limit` |
| `Tab_Video` | `Video` |
| `Tab_Audio` | `Audio` |
| `Tab_General` | `General` |
| `Settings_Bitrate` | `Bitrate` |
| `Settings_Bitrate_CustomSuffix` | `Mbps` |
| `Settings_Gpu` | `Recording GPU` |
| `Settings_Format` | `File format` |
| `Settings_Group_Mic` | `MICROPHONE` |
| `Settings_Group_SystemAudio` | `SYSTEM SOUND` |
| `Settings_AudioMode` | `Recording mode` |
| `AudioMode_All` | `All PC audio` |
| `AudioMode_Apps` | `Specific apps` |
| `Settings_AppsHint` | `Apps will appear here when they start playing audio.` |
| `Settings_MicChannels` | `Channels` |
| `Channels_Stereo` | `Stereo` |
| `Channels_Mono` | `Mono` |
| `Settings_Ptt` | `Push-to-talk` |
| `Settings_Ptt_Desc` | `Microphone is live only while the key is held` |
| `Settings_PttKey` | `Push-to-talk key` |
| `Settings_SeparateTracks` | `Separate audio tracks` |
| `Settings_SeparateTracks_Desc` | `System sound and microphone are saved as separate tracks in the file` |
| `Settings_Group_Automation` | `AUTOMATION` |
| `Settings_StartWithWindows` | `Start with Windows` |
| `Settings_StartWithWindows_Desc` | `Adds the app to system startup` |
| `Settings_StartMinimized` | `Start minimized` |
| `Settings_StartMinimized_Desc` | `Launches hidden in the system tray` |
| `Settings_DisableGameMode` | `Disable Windows Game Mode` |
| `Settings_DisableGameMode_Desc` | `Windows Game Mode throttles background apps: in-game recordings can drop to 1-2 FPS and saving a replay may not trigger until you alt-tab. Lag disables it automatically while this is on. Strongly recommended to keep enabled.` |
| `Settings_GameCapture` | `Game capture (hook)` |
| `Settings_GameCapture_Desc` | `Needed for games in EXCLUSIVE fullscreen. CS2 also requires the "-allow_third_party_software" Steam launch option, otherwise the hook is rejected and retries waste CPU. Leave off if you record in windowed/borderless modes.` |
| `Editor_Title` | `Editor` |
| `Editor_Edit` | `Edit` |
| `Editor_Back` | `Back` |
| `Editor_PickClip` | `Choose a clip to edit` |
| `Editor_NoClips` | `No clips to edit yet.` |
| `Editor_Clips` | `CLIPS` |
| `Editor_Export` | `Export` |
| `Editor_Exporting` | `Exporting… {0}%` |
| `Editor_ExportDone` | `Saved: {0}` |
| `Editor_ExportError` | `Export failed: {0}` |
| `Editor_Speed` | `Speed` |
| `Editor_Format` | `Format` |
| `Editor_Crop` | `Crop` |
| `Editor_Deselect` | `Deselect` |
| `Editor_ExportResolution` | `Resolution` |
| `Editor_ExportGif` | `Export as GIF` |
| `Editor_ExportOptions` | `Export options` |
| `Editor_CancelExport` | `Cancel` |
| `Editor_Color` | `Color` |
| `Editor_Brightness` | `Brightness` |
| `Editor_Contrast` | `Contrast` |
| `Editor_Saturation` | `Saturation` |
| `Editor_Cut` | `Cut` |
| `Editor_CutEnd` | `End here` |
| `Editor_TabText` | `Text` |
| `Editor_AddText` | `Add text` |
| `Editor_TextSize` | `Size` |
| `Editor_DeleteText` | `Delete` |
| `Editor_TextDefault` | `Text` |
| `Editor_TextHint` | `Add text, then drag it on the video` |
| `Editor_TabClip` | `Clip` |
| `Editor_TabFilters` | `Filters` |
| `Editor_Tracks` | `AUDIO TRACKS` |
| `Editor_TrackSystem` | `System sound` |
| `Editor_TrackMic` | `Microphone` |
| `Editor_TrackAudio` | `Audio` |
| `Editor_SetStart` | `Set clip start to the current moment` |
| `Editor_SetEnd` | `Set clip end to the current moment` |
| `Editor_ResetTrim` | `Reset trim` |
| `Settings_Hotkey_Screenshot` | `Screenshot` |
| `Toast_ReplaySaved` | `Replay saved` |
| `Toast_ScreenshotSaved` | `Screenshot saved` |
| `Toast_RecordingStarted` | `Recording started` |
| `Toast_RecordingStopped` | `Recording stopped` |
| `Toast_NowRecording` | `Now recording` |
| `Settings_AutoStart` | `Start recording automatically` |
| `Settings_AutoStart_Desc` | `The replay buffer starts right after launch` |
| `Settings_Group_Hotkey` | `HOTKEY` |
| `Settings_Hotkey_SaveReplay` | `Save replay` |
| `Settings_Hotkey_Desc` | `Click the button, then press the desired key combination` |
| `Settings_Hotkey_Change` | `Change` |
| `Settings_Hotkey_Listening` | `Listening...` |
| `Settings_Group_Paths` | `PATHS` |
| `Settings_LibraryFolder` | `Library folder` |
| `Settings_Browse` | `Browse` |
| `About_Group` | `ABOUT` |
| `About_Version` | `Version` |
| `About_CheckUpdates` | `Check for Updates` |
| `Update_Checking` | `Checking for updates...` |
| `Update_UpToDate` | `You have the latest version.` |
| `Update_Downloading` | `Downloading update...` |
| `Update_DevMode` | `Updates are unavailable in dev mode.` |
| `Update_Failed` | `Update failed: {0}` |
| `Library_Title` | `Library` |
| `Library_Subtitle` | `Your saved moments` |
| `Library_Empty` | `No replays yet. Start recording and press the hotkey to save a replay!` |
| `Library_Delete` | `Delete` |
| `Library_OpenFolder` | `Open folder` |
| `Library_ShowInFolder` | `Show in folder` |
| `Library_FreeSpace` | `Free space` |
| `Library_Search` | `Search clips` |
| `Library_Favorites` | `Favorites` |
| `Library_Desktop` | `Desktop` |
| `Library_Today` | `Today` |
| `Library_Yesterday` | `Yesterday` |
| `Library_NoResults` | `No clips match your search.` |
| `Library_ClipCount` | `{0} CLIPS` |
| `Library_GroupCount` | `{0} clips` |
| `Library_ClearSelection` | `Clear selection` |
| `Player_Replays` | `REPLAYS` |
| `Player_Clips` | `Clips` |
| `Player_Duration` | `Duration` |
| `Player_Placeholder` | `Video player` |
| `Player_NowPlaying` | `Now playing: ` |
| `Player_ReplayBadge` | `REPLAY` |
| `Tooltip_Stop` | `Stop` |
| `Tooltip_Rewind` | `Back 10s` |
| `Tooltip_PlayPause` | `Play / Pause` |
| `Tooltip_Forward` | `Forward 10s` |
| `Tooltip_Next` | `Next replay` |
| `Tooltip_Fullscreen` | `Fullscreen (ESC to exit)` |
| `Tooltip_Volume` | `Volume` |
| `Status_Ready` | `Ready` |
| `Status_Starting` | `Starting recording...` |
| `Status_Recording` | `Recording...` |
| `Status_NoMonitor` | `No display selected for recording.` |
| `Status_RecordError` | `Recording error: {0}` |
| `Status_Stopping` | `Stopping...` |
| `Status_Stopped` | `Stopped` |
| `Status_StopError` | `Stop error: {0}` |
| `Status_SaveError` | `Save error: {0}` |
| `Status_ReplaySaved` | `Replay saved: {0}` |
| `Status_Saving` | `Saving replay...` |
| `Status_NotRecording` | `Recording is not active — press 'Start Recording' first.` |
| `Hotkey_PressCombo` | `Press a key combination...` |
| `Browse_Title` | `Select a folder for saving replays` |
| `Tray_SaveReplay` | `Save Replay` |
| `Tray_OpenLibrary` | `Open Library` |
| `Tray_Open` | `Open` |
| `Tray_Exit` | `Exit` |
| `Time_SecShort` | `{0} sec` |
| `Time_MinShort` | `{0} min` |
| `Time_HourMinShort` | `{0} h {1} min` |
| `Option_Native` | `Native` |
| `Option_Auto` | `Auto` |
| `Option_Custom` | `Custom` |
| `Sidebar_Buffer` | `Buffer` |
| `Library_Stats` | `{0} clips · {1} free` |
| `Player_Used` | `Used` |
| `Library_EmptyTitle` | `No replays yet` |
| `Settings_Hotkey_Pause` | `Pause recording` |
| `Settings_Hotkey_Record` | `Start/stop recording` |
| `Btn_PauseRecording` | `Pause` |
| `Btn_ResumeRecording` | `Resume` |
| `Status_Paused` | `Paused` |
| `Settings_OutputDevice` | `Output device` |
| `Option_DefaultDevice` | `Default (auto)` |
| `Settings_AudioModeByTarget` | `Match audio mode to capture` |
| `Settings_AudioModeByTarget_Desc` | `Recording a game captures the selected apps; the desktop captures all PC sound.` |
| `Settings_Subtitle` | `Configure capture, audio and app behavior.` |
| `Settings_Group_Encoding` | `ENCODING` |
| `Settings_Group_Devices` | `DEVICES` |
| `Settings_Group_Advanced` | `ADVANCED` |
| `Settings_Group_Startup` | `STARTUP & PERFORMANCE` |
| `Settings_Group_Storage` | `STORAGE & ABOUT` |
| `Settings_Logs` | `Logs` |
| `Settings_OpenLogs` | `Open folder` |
| `Settings_SaveDiagnostics` | `Export .zip` |
| `Preset_SectionTitle` | `PRESETS` |
| `Preset_Subtitle` | `Tuned to your hardware — pick a goal` |
| `Preset_Performance` | `Performance` |
| `Preset_Balanced` | `Balanced` |
| `Preset_Quality` | `Quality` |
| `Preset_PerformanceDesc` | `Smoothest gameplay` |
| `Preset_BalancedDesc` | `Solid fps and quality` |
| `Preset_QualityDesc` | `Sharpest clips` |
| `Preset_RecommendedBadge` | `Recommended` |
| `Preset_Rescan` | `Re-scan hardware` |
| `Preset_Advanced` | `ADVANCED` |
| `Preset_CleanupHint` | `Only {0} GB free — consider enabling auto-cleanup (~{1} GB).` |
| `Settings_OutputLevel` | `Output level` |
| `Settings_MicLevel` | `Mic level` |
| `Settings_HearYourself` | `Hear yourself` |
| `Settings_HearYourselfHint` | `Plays your mic to the selected output — use headphones to avoid echo.` |
| `Settings_InputSensitivity` | `Input sensitivity` |
| `Settings_InputSensitivityHint` | `Mutes the mic below the threshold — auto adapts to your room.` |
| `Settings_NoiseSuppression` | `Noise suppression` |
| `Settings_NoiseSuppressionHint` | `Removes steady background noise — fans, hum, keyboard — from your mic.` |
| `Settings_Auto` | `Auto` |
| `Settings_Threshold` | `Threshold` |
| `Settings_Listen` | `Listen` |
| `Settings_Stop` | `Stop` |
| `Settings_Group_Overlay` | `Overlay` |
| `Settings_WebcamOverlay` | `Webcam in recordings` |
| `Settings_WebcamOverlay_Desc` | `Show your camera in a corner of saved clips` |
| `Settings_WebcamDevice` | `Camera` |
| `Settings_WebcamPosition` | `Position` |
| `Settings_WebcamSize` | `Size` |
| `Settings_KeysOverlay` | `Show pressed keys` |
| `Settings_KeysOverlay_Desc` | `Overlay pressed keyboard and mouse buttons on clips` |
| `Overlay_Enable` | `Video overlays` |
| `Overlay_Enable_Desc` | `Bake the webcam and keyboard/mouse into recordings — pick a corner for each below` |
| `Overlay_None` | `None` |
| `Overlay_Camera` | `Camera` |
| `Overlay_Keys` | `Keyboard + mouse` |
| `Overlay_Stats` | `Resource monitor` |
| `Overlay_WheelHint` | `Drag to move · drag an edge or scroll the mouse wheel to resize` |

### 5.4 Assets, Manifest, Project, Installer & README

- **Sound assets (`Assets/*.wav`)** (`InstantReplay.csproj:52-53`, `MainViewModel.cs:713-742`):
  - `Assets/save.wav` and `Assets/screenshot.wav` are both embedded as Avalonia resources AND copied to `<exe_dir>\Assets\` (`CopyToOutputDirectory=PreserveNewest`) so `System.Media.SoundPlayer` plays them asynchronously on `Task.Run` via `PlaySync()`.
- **App Manifest (`app.manifest`)**:
  - Windows 10/11 compatibility GUID `{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}`.
  - DPI awareness: `true/pm` (`dpiAware`) and `PerMonitorV2, PerMonitor` (`dpiAwareness`). Required so WGC physical pixels match window/monitor coordinates.
- **Project File (`InstantReplay.csproj`)**:
  - `OutputType=WinExe`, `TargetFramework=net8.0-windows10.0.26100.0`, `SupportedOSPlatformVersion=10.0.19041.0`, `AssemblyName=Lag`, `ApplicationIcon=Assets\icon.ico`.
  - Copies `ffmpeg\**\*` and `rnnoise\**\*` to output directory (`PreserveNewest`).
  - Packages: `Avalonia* 11.2.3`, `CommunityToolkit.Mvvm 8.4.0`, `FFMpegCore 5.4.0`, `Material.Icons.Avalonia 2.1.10`, `Microsoft.Extensions.DependencyInjection 8.0.1`, `SharpHook 5.3.9`, `LibVLCSharp* 3.9.2`, `Velopack 1.1.1`, `NAudio.* 2.3.0`, `SharpDX.* 4.2.0`, `VideoLAN.LibVLC.Windows 3.0.21`, `FFmpeg.AutoGen 7.0.0`, `System.Windows.Extensions 8.0.0`, `System.Drawing.Common 8.0.10`.
- **Installer (`../installer/Lag.iss`)**:
  - `AppId=Lag`, `AppName=Lag`, `AppPublisher=shkbb`, `AppURL=https://github.com/shkbb/Lag`, `OutputBaseFilename=Lag-win-Setup`, `Compression=lzma2/max`, `SolidCompression=yes`.
  - `PrivilegesRequired=lowest`, `PrivilegesRequiredOverridesAllowed=dialog`, `DefaultDirName={autopf}\Lag`, `DisableDirPage=no`, `DisableProgramGroupPage=yes`, `DisableWelcomePage=no`.
  - `MinVersion=10.0.18362` (Win10 1903), `ArchitecturesAllowed=x64compatible`, `ArchitecturesInstallIn64BitMode=x64compatible`.
  - `CloseApplications=yes`, `RestartApplications=no`.
  - 18 setup languages (`english`, `ukrainian`, `czech`, `danish`, `dutch`, `finnish`, `french`, `german`, `hungarian`, `italian`, `japanese`, `korean`, `norwegian`, `polish`, `portuguese`, `russian`, `spanish`, `turkish`).
  - `[Tasks]`: `desktopicon` (`{autodesktop}\Lag.lnk`). `[Icons]`: `{autoprograms}\Lag.lnk`.
  - `[Run]`: interactive finish-page launch checkbox (`skipifsilent`).
  - `[Code]`: `ssPostInstall` calls `CleanupLegacyVelopack()` (if `%LocalAppData%\Lag\Update.exe` exists and `{app}` is not inside it, deletes `%LocalAppData%\Lag`, desktop/start-menu shortcuts, and `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag`), then if `/AUTOSTART` CLI param is present, launches `{app}\Lag.exe` (`SW_SHOWNORMAL`, `ewNoWait`).
- **`../README.md`**: Bilingual (English + Ukrainian) documentation covering features, WGC/FFmpeg VFR architecture, `Lag-win-Setup.exe` installation, SmartScreen note, build instructions, and GPL v3 license.

---

## 6. Native / External Dependencies

| C# Dependency / API | Usage in Shell | Suggested Rust Equivalent |
|---|---|---|
| `winmm.dll!timeBeginPeriod(1)` (`Program.cs:18,68`) | 1 ms OS timer resolution for render & media timers | `windows::Win32::Media::timeBeginPeriod(1)` |
| `kernel32.dll!SetUnhandledExceptionFilter` (`Program.cs:22,39`) | Top-level native SEH crash handler → `crash.log` | `windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter` |
| `System.Threading.Mutex` `"LagAppSingleInstanceMutex"` (`Program.cs:162`) | Single-instance guard across elevation levels | `windows::Win32::System::Threading::CreateMutexW` + `GetLastError() == ERROR_ALREADY_EXISTS` / `ERROR_ACCESS_DENIED` |
| `kernel32.dll!SetDllDirectory` / `AddDllDirectory` (`App.axaml.cs:25-30`) | Resolve bundled `ffmpeg\` DLLs (`avcodec-61`, etc.) | `windows::Win32::System::LibraryLoader::{SetDllDirectoryW, AddDllDirectory}` |
| `kernel32.dll!SetProcessInformation(ProcessPowerThrottling)` (`PerformanceGuard.cs:32,58`) | Opt out of Windows EcoQoS background throttling | `windows::Win32::System::Threading::SetProcessInformation` |
| `gdi32.dll!D3DKMTSetProcessSchedulingPriorityClass` (`PerformanceGuard.cs:44,73`) | Raise GPU scheduling priority (`5`→`4`→`3`) | Raw `gdi32.dll` FFI `D3DKMTSetProcessSchedulingPriorityClass` |
| `SharpDX.DXGI.Factory1` + `Output.WaitForVerticalBlank()` (`DynamicRenderTimer.cs:108,144`) | Phase-lock render loop to window's monitor vblank | `windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIOutput::WaitForVerticalBlank}` (if Slint doesn't already vsync per-monitor) |
| `user32.dll!RegisterHotKey` / `UnregisterHotKey` / `GetMessage` / `PostThreadMessage` (`GlobalHotkeyService.cs:28-50`) | System-wide `WM_HOTKEY` thread with `MOD_NOREPEAT` | `windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey}` + `GetMessageW` / `PostThreadMessageW` |
| `SharpHook` (`libuiohook` `TaskPoolGlobalHook`) (`GlobalHotkeyManager.cs:13-88`) | Low-level keyboard & mouse-button hook (keybind capture, PTT, `KeystrokeTracker`) | `WH_KEYBOARD_LL` + `WH_MOUSE_LL` on dedicated message thread pushing to lock-free channel (`crossbeam`/`flume`) so hook callback never blocks cursor! |
| `user32.dll!SetWindowLongPtrW` / `SetWindowDisplayAffinity(..., 0x11)` (`StatsHudWindow.cs:141-143`, `ToastService.cs:150-151`) | Non-activating, click-through, toolwindow, topmost, `WDA_EXCLUDEFROMCAPTURE` | `windows::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, SetWindowDisplayAffinity}` |
| `shell32.dll!SHQueryUserNotificationState` (`StatsHudWindow.cs:160`, `ToastService.cs:227`) | Detect exclusive fullscreen D3D (`QUNS_RUNNING_D3D_FULL_SCREEN == 3`) | `windows::Win32::UI::Shell::SHQueryUserNotificationState` |
| `pdh.dll` (`PdhOpenQueryW`, `PdhAddEnglishCounterW`, `PdhCollectQueryData`, `PdhGetFormattedCounterArrayW`) (`SystemStatsSampler.cs:128-137`) | Total 3D GPU utilization % | `windows::Win32::System::Performance::{PdhOpenQueryW, PdhAddEnglishCounterW, ...}` |
| `kernel32.dll!GetSystemTimes`, `GlobalMemoryStatusEx` (`SystemStatsSampler.cs:140,157`) | System-wide CPU % and physical RAM GB | `windows::Win32::System::Threading::GetSystemTimes`, `SystemInformation::GlobalMemoryStatusEx` |
| `System.Media.SoundPlayer` (`MainViewModel.cs:734`) | Play `Assets/save.wav` & `Assets/screenshot.wav` | `windows::Win32::Media::Audio::PlaySoundW` (`SND_FILENAME | SND_ASYNC`) or `rodio` |
| `System.Net.Http.HttpClient` + `System.Text.Json` + `Microsoft.Win32.Registry` (`AppUpdateService.cs:1-166`) | Check Inno uninstall registry key & download `Lag-win-Setup.exe` from GitHub API | `winreg` (or `windows-rs` registry), `reqwest` + `serde_json` |
| `Velopack` (`Program.cs:155`) | Legacy migration shim only | Not needed for fresh Rust app unless migrating legacy C# Velopack installs via CLI flags |

---

## 7. Cross-Area Interfaces

- **Shell → Recorder (`IReplayRecorder` / `VfrRecorderAdapter`)**:
  - `MainViewModel` calls `Initialize(RecorderOptions)`, `StartBuffer()`, `SaveReplay()`, `Teardown()`, `SetMicMuted(bool)`, `SetPaused(bool)` (`MainViewModel.cs:267,273,545-546,585,620,680`).
  - `MainViewModel` reads `CurrentTarget`, `LastSavedGameTag`, `IsPaused`, and `VfrRecorderAdapter.FramesCaptured` (for `StatsHudWindow`) (`MainViewModel.cs:98,105,206-207,444`).
  - `MainViewModel` subscribes to `ReplaySaved` and `CaptureTargetChanged` (`MainViewModel.cs:196,226`).
  - `GlobalHotkeyManager` feeds `KeystrokeTracker.Instance.OnKey(KeyCode, down)` and `OnMouseButton(button, down)` (`GlobalHotkeyManager.cs:99,105,116,157`).
  - `App.Initialize` calls `FfmpegInterop.Initialize(ffmpegDir)` (`App.axaml.cs:50`).
- **Shell → Settings (`SettingsViewModel`)**:
  - `App` reads `Settings.StartMinimized`, `Settings.AreHotkeysSuppressed`, calls `Settings.FlushPendingSave()` (`App.axaml.cs:230,261,275,286,297,402`).
  - `MainViewModel` reads all capture/overlay/hotkey/startup/version settings to build `RecorderOptions`, log diagnostics, run `AutoStartRecording`, run `AutoUpdateOnStartupAsync()`, and update `StatsHudWindow` via `Settings.StatsHudChanged` (`MainViewModel.cs:185,239,292,308-318,416-417,440,452,472-537,582`).
  - `SettingsViewModel` calls back into Shell: `App.SetLanguage(code)`, `App.SetTheme(mode)`, `GlobalHotkeyService` (`Update*Hotkey`, `Clear*Hotkey`), `GlobalHotkeyManager` (`IsCapturing`, `HotkeyCaptured`, `RequiredKey/Modifiers`, `PttEnabled/PttKey`), `AppUpdateService` (`DetectInstall`, `CurrentVersion`, `GetLatestAsync`, `DownloadAndInstallAsync`), `FileLog.LogDir` (`SettingsViewModel.cs:189-272,1211,1233,1346-1454,1569-1675`).
- **Shell → Library (`LibraryViewModel`)**:
  - `MainViewModel` calls `Library.RefreshCommand.ExecuteAsync(null)` at startup, on `ReplaySaved`, and on `Library`/`Player`/`Editor` navigation (`MainViewModel.cs:215,231,347,356,367`).
  - `MainViewModel` subscribes to `Library.PlayClipRequested`, `Library.EditClipRequested`, `Library.Clips.CollectionChanged`, and calls `ClipMetadataStore.Write` (`MainViewModel.cs:208,277,284,287`).
- **Shell → Player (`PlayerViewModel`) & Editor (`EditorViewModel`)**:
  - `MainViewModel` calls `Player.LoadAndPlay(clip)`, `Player.StopPlayback()`, `Editor.OpenClip(clip)`, reads `Player.CurrentClip` (`MainViewModel.cs:279,334,341,364,381,389`).
  - `MainWindow.Closing` calls `Player.StopPlayback()` and `Editor.StopPreview()` (`MainWindow.axaml.cs:48-49`).
  - `PlayerView` & `EditorView` bind to `MainViewModel.IsFullscreen`, `ToggleFullscreenCommand`, `OpenEditorFromPlayerCommand`, `EditableClips`, `OpenEditorForClipCommand`.
- **Shell → Hardware & Diagnostics**:
  - `Program.cs` & `MainWindow.cs` call `HardwareDetector.GetAvailableMonitors()`, `GetWindowRefreshRate(hwnd)`, `GetWindowMonitor(hwnd)` (`Program.cs:209`, `MainWindow.axaml.cs:124-126`).
  - `MainViewModel.LogStartupDiagnostics` calls `HardwareProfiler.Capture(Settings.LibraryPath)` and `PresetResolver.LogRecommendations(profile)` (`MainViewModel.cs:322-324`).
  - `MainViewModel.TakeScreenshot` calls `ScreenshotService.Capture(deviceName, outputDir)` (`MainViewModel.cs:422`).

---

## 8. Self-Tests / Diagnostics

All triggered via environment variables checked in `Program.Main` (`Program.cs:71-135`, exiting before mutex/UI) or runtime capture:

| Env Var | Entry Point | Purpose / Optional Args |
|---|---|---|
| `LAG_VFR_TEST=1` | `VfrEngineTest.Run()` (`Program.cs:71-75`) | Headless E2E VFR capture/encode test. Optional: `LAG_VFR_OUT`, `LAG_VFR_PID`, `LAG_VFR_DESKTOP=1`, `LAG_VFR_CODEC`, `LAG_VFR_BITRATE`, `LAG_VFR_FORMAT`, `LAG_VFR_HEIGHT`, `LAG_VFR_MAXFPS`, `LAG_VFR_PRESET`, `LAG_VFR_DUPONLY=1`, `LAG_VFR_SEPAUDIO=1`, `LAG_VFR_NOAUDIO=1`, `LAG_VFR_APPS`, `LAG_VFR_WEBCAM=1`, `LAG_VFR_KEYS=1`, `LAG_VFR_STATS=1`, `LAG_VFR_SECONDS`, `LAG_VFR_CARRY=1`. |
| `LAG_MFPROBE=<paths>` | `PlaybackTrackProbe.Run()` (`Program.cs:79-83`) | Semicolon-separated clip paths; probes which audio track Windows Media Foundation selects. |
| `LAG_WGCPROBE=1` | `WgcTimestampProbe.Run()` (`Program.cs:87-91`) | Foreground window WGC timestamp vs arrival probe. Optional: `LAG_WGCPROBE_SECONDS` (default 10), `LAG_WGCPROBE_FPS` (default 165). |
| `LAG_DUPPROBE=1` | `DuplicationProbe.Run()` (`Program.cs:95-99`) | Counts Desktop Duplication frames reaching monitor. Optional: `LAG_DUPPROBE_OUTPUT` (default 0), `LAG_DUPPROBE_SECONDS` (default 8). |
| `LAG_STALLTEST=1` | `StallMonitorSelfTest.Run()` (`Program.cs:102-106`) | Headless stall attribution check (no GPU/FFmpeg). |
| `LAG_GCTEST=1` | `GcSelfTest.Run()` (`Program.cs:110-114`) | Headless GC stress test of replay ring buffer. Optional: `LAG_GCTEST_PACKETS=<csv>`, `LAG_GCTEST_MODE=interactive`. |
| `LAG_CADENCETEST=1` | `CadenceSelfTest.Run()` (`Program.cs:117-121`) | Headless cadence stabiliser check. |
| `LAG_CARRYTEST=1` | `CarryOverSelfTest.Run()` (`Program.cs:124-128`) | Headless carry-over stitching logic test. |
| `LAG_PROCTEST=<pid>` | `ProcessLoopbackCapture.SelfTest(pid)` (`Program.cs:131-135`) | Headless 5 s per-app WASAPI loopback capture validation. |
| `LAG_VFR_DIAG=1` | `Bgra2Nv12Converter.Diag` (`Bgra2Nv12Converter.cs:46`) | Checked during normal/test runs for green-flash D3D diagnostics. |

---

## 9. Known Bugs / Quirks

1. **Tray menu language does not update live (`App.axaml.cs:312,322`, `SettingsViewModel.cs:1211`)**: `LocalizeTrayMenu()` is private and only called once at startup. Switching language in Settings updates XAML `{DynamicResource}` strings live, but tray menu items stay in the startup language until restart. *Fix in Rust port* (re-localize tray items on `SetLanguage`).
2. **Duplicate Save-Replay hotkey trigger path (`App.axaml.cs:254-268` & `MainViewModel.cs:244-259`)**: Both `GlobalHotkeyService` (Win32 `RegisterHotKey`) and `GlobalHotkeyManager` (SharpHook) listen for the Save Replay combo! In C# they race on `IsSaving` (`MainViewModel.cs:250` sets `IsSaving = true` on a thread-pool thread without locking, while `SaveReplayCommand` checks `IsSaving` on the UI thread). Also at startup `App.axaml.cs:253` calls `globalHotkeyService.Start(0x0001, 0x79)` (`Alt+F10`), which runs AFTER `SettingsViewModel` already ran `ApplyHotkeyToGlobalService()` during `GetRequiredService<MainViewModel>()` (`App.axaml.cs:218`) — wait: actually `SettingsViewModel` calls `UpdateHotkey` (which calls `StartLoop`), so `Start()` at line 253 sees `_messageLoopThread != null` and returns early (unless Save hotkey was unbound `VcUndefined`, in which case `_messageLoopThread == null` and `Start(0x0001, 0x79)` **re-enables Alt+F10 even though the user unbound it**!). *Fix in Rust port*: have a single authoritative hotkey registration path (`RegisterHotKey` for trigger hotkeys, low-level hook only for capture/PTT/keystroke overlay) and respect unbound state at startup.
3. **Save watchdog comment vs code (`MainViewModel.cs:697,710`)**: XML doc says "Reset it after 20s", code uses `TimeSpan.FromSeconds(120)` (intentionally increased for CPU fallback stitched saves). *Preserve `120 s`*.
4. **`MainWindow.axaml:12` hardcoded `#0C0D11` background**: Window background is hardcoded dark while inner Grid uses `{DynamicResource SurfaceApp}`. Should follow theme surface in Rust port so resize/startup doesn't flash dark in Light theme.
5. **`MainWindow.Closing` dead `OnExplicitShutdown` check (`MainWindow.axaml.cs:36-41`)**: `ShutdownMode` is never set to `OnExplicitShutdown`; exit goes through `TrayIcon_ExitClicked` (`Environment.Exit(0)`). *Preserve hide-to-tray on window close and explicit exit from tray*.
6. **Fullscreen exit always restores `WindowState.Normal` (`MainWindow.axaml.cs:132`)**: Exiting fullscreen from a previously maximized window drops back to `Normal` instead of `Maximized`. *May preserve or improve by remembering prior state*.
7. **`DesktopLabels` hardcoded in `MainViewModel.cs:137-144`**: Duplicates localization instead of reading `Library_Desktop` from language dictionary. *Can read from localizer in Rust port (same English/translated strings)*.
8. **`SystemStatsSampler.CountFrame()` unused by `StatsHudWindow` (`StatsHudWindow.cs:101-108`)**: `StatsHudWindow` computes FPS from `VfrRecorderAdapter.FramesCaptured`, ignoring `_sampler.Current.Fps`. *Preserve `FramesCaptured` delta FPS source*.
9. **`Program.cs:61` stale comment & `README.md:49,147` stale Velopack mention**: Comments mention `%AppData%\Lag\logs` (actual path is `Documents\Lag\Logs`) and Velopack (actual updater is GitHub Releases + `Lag-win-Setup.exe`). *Follow actual code paths*.
10. **`App.axaml:247-248` static disabled slider brushes**: `SliderTrackValueFillDisabled=#22D3EE` and `SliderTrackFillDisabled=#24FFFFFF` are static rather than themed in `Palette.axaml`. *Preserve or theme cleanly*.

---

## 10. Parity Checklist

- [ ] Request 1 ms system timer period (`timeBeginPeriod(1)`) for process lifetime.
- [ ] Enforce single instance via named mutex `LagAppSingleInstanceMutex` (including across elevation levels).
- [ ] Initialize file logging before UI: `Documents\Lag\Logs\Lag-yyyy-MM-dd_HH-mm-ss.txt`, prune keeping 15 newest, `[HH:mm:ss.fff]` line prefix, plus `crash.log` for panics and native SEH exceptions (`ACCESS_VIOLATION`, etc. with module+offset).
- [ ] Configure bundled `ffmpeg/` directory on process DLL search path / `PATH` / working dir before encoder probing.
- [ ] Opt process out of Windows EcoQoS (`SetProcessInformation` `ProcessPowerThrottling`) and raise GPU scheduling priority (`D3DKMTSetProcessSchedulingPriorityClass`: `5` → `4` → `3`).
- [ ] Enable `PerMonitorV2, PerMonitor` DPI awareness and Windows 10/11 compatibility in application manifest.
- [ ] Support all 9 `LAG_*` headless diagnostic/test CLI env vars before UI startup (`LAG_VFR_TEST`, `LAG_MFPROBE`, `LAG_WGCPROBE`, `LAG_DUPPROBE`, `LAG_STALLTEST`, `LAG_GCTEST`, `LAG_CADENCETEST`, `LAG_CARRYTEST`, `LAG_PROCTEST`).
- [ ] Create frameless main window (`1360×840`, min `1080×640`, centred, `Lag` icon, custom drag bar, min/max/close buttons).
- [ ] Render `58px` top command bar with logo (`28×28` rounded-8 accent badge + 3-path rewind-clock + `"Lag"`), `1×26` divider, Record CTA (cyan↔red `.rec`, animated blinking stop square, 1200 ms toggle debounce), Save Replay button (disabled when idle), Pause/Resume button (visible only while recording), and Status Pill (idle/pulsing-red dot, status text, `74×5` animated buffer progress bar, `m:ss / m:ss` text).
- [ ] Render `78px` left icon rail with top items (`Library`, `Player`, `Editor`) and bottom items (`CaptureTarget` pulsing icon + hover `CaptureFlyout`, `Settings`), highlighting active view with `AccentDim` background + `Accent` icon.
- [ ] Animate view transitions (`220ms` vertical slide + cross-fade) and start on `Settings` view.
- [ ] Support `IsFullscreen` mode: hide top bar + icon rail, enter OS fullscreen, exit on `ESC` key even when video surface has focus.
- [ ] Support `StartMinimized` (launch hidden in tray without flash) and `AutoStartRecording` (start buffer after UI init).
- [ ] Intercept main window close to stop Player & Editor playback and hide to system tray instead of exiting.
- [ ] Provide system tray icon (`icon.ico`, tooltip `"Lag"`, left-click restores/activates window) and localized context menu (`Save Replay`, `Open Library`, separator, `Open`, `Exit`), with 2500 ms exit watchdog + settings flush on `Exit`.
- [ ] Implement full `Dark` and `Light` (warm cream `#EAE3D1` / `#A9722E`) design token palettes, typography (`Inter`, monospace fallback chain), corner radii (`15/13/11/10/9/7/6`), and all `rcard`/`rchip`/`rseg`/`ToggleSwitch`/`Slider`/`ComboBox`/`ScrollBar` control styles and transition timings.
- [ ] Implement live theme switching (`light` / `dark` / `system`) with `280ms` snapshot cross-fade overlay.
- [ ] Implement live localization across all 17 languages (`en`, `uk`, `de`, `fr`, `be`, `lt`, `et`, `lv`, `fi`, `sv`, `no`, `da`, `nl`, `it`, `es`, `pt`, `ja`) with all 235 string keys, `en` fallback, and all 17 vector `30×20` flags (updating tray menu labels too).
- [ ] Implement Win32 `RegisterHotKey` (`MOD_NOREPEAT`) global hotkeys on dedicated thread for Save Replay (`Alt+F10`), Screenshot (`Alt+F9`), Pause/Resume (`Alt+F8`), Record Toggle (`Alt+F7`), supporting unbound (`VcUndefined`) state and `800ms` suppression during/after rebinding.
- [ ] Implement low-level global keyboard/mouse-button hook (non-blocking) for keybinding capture (`ESC` clears/unbinds, standalone modifiers ignored), Push-to-Talk hold/release (`V` default), and `KeystrokeTracker` feed (buttons only, never mouse-move).
- [ ] Implement Save Replay orchestration: immediate `Status_Saving` toast + `Assets/save.wav` chime, `120s` timeout watchdog, `ClipMetadataStore` game/exe sidecar tagging, `Library` refresh, silent `Toast_ReplaySaved` confirmation on completion.
- [ ] Implement global Screenshot capture (`Screenshot_yyyy-MM-dd_HH-mm-ss.png` of selected/primary monitor) + `Toast_ScreenshotSaved` + `Assets/screenshot.wav` chime.
- [ ] Implement sequential queued non-activating topmost toasts (`max 6`, `2400ms` visible, `220ms`/`260ms` fade, primary working area top-right `24px` inset, suppressed when `SHQueryUserNotificationState == 3`).
- [ ] Implement live `StatsHudWindow` (`Topmost`, `WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`, `WDA_EXCLUDEFROMCAPTURE`, `1s` sampler for engine-delta FPS, `GetSystemTimes` CPU%, PDH 3D GPU%, `GlobalMemoryStatusEx` RAM, 3 detail levels, scale `0.03–0.10`, fractional positioning, hidden during exclusive fullscreen).
- [ ] Implement auto-updater (`3s` post-launch silent check + manual check in Settings): detect `DevOrPortable` vs `Installed` (`Lag_is1` registry) vs `LegacyVelopack` (`..\Update.exe`), query GitHub Releases latest `Lag-win-Setup.exe`, download to `%TEMP%\Lag-Setup-{ver}.exe`, run `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /AUTOSTART`, and exit.
- [ ] Maintain Inno Setup installer (`Lag.iss`) parity: `AppId=Lag`, per-user default with UAC elevation option, 18 languages, `/AUTOSTART` relaunch, and legacy `%LocalAppData%\Lag` Velopack cleanup.
