# Settings Screen & Backing Services — 1:1 Port Specification

## 1. Scope

Primary files covered by this specification:

| File | Lines | Role |
|---|---:|---|
| `ViewModels/SettingsViewModel.cs` | 2319 | Core settings state, option lists, `SettingsData` JSON schema, load/save/debounce, hotkeys, registry automation, updates, diagnostics export, record/option types, `AppAudioItem` |
| `ViewModels/SettingsViewModel.Presets.cs` | 134 | `PresetCard`, hardware-tailored preset initialization, matching, application, disk-cleanup hint |
| `ViewModels/SettingsViewModel.AudioMeter.cs` | 213 | Live audio level meter, `AudioDeviceWatcher` hot-plug debouncing, "Hear yourself" monitoring, per-app audio scanner lifecycle |
| `Views/SettingsView.axaml` | 824 | Settings view layout, tabs (Video / Audio / Overlay / General), styles, bindings, intensive-quality warning modal |
| `Views/SettingsView.axaml.cs` | 285 | Tab scroll reset & slide-fade animation, attach/detach/window-focus lifecycle, interactive overlay preview drag/resize/wheel, manual noise-gate bar drag |
| `Services/PresetResolver.cs` | 239 | `RecordingIntent`, `GpuTier`, `RecommendedSettings`, GPU tiering and RAM/disk/display-aware preset resolution |
| `Services/HardwareDetector.cs` | 306 | DXGI GPU adapter enumeration, Win32 monitor + PnP `StableId` + refresh rate enumeration, WASAPI capture/render audio endpoint enumeration |
| `Services/HardwareProfiler.cs` | 206 | `GpuProfile`, `MachineProfile`, static machine snapshot (DXGI VRAM, `EncoderSelector`, registry CPU name, `GlobalMemoryStatusEx` RAM, `DriveInfo` disk, primary monitor) |
| `Services/AudioMonitor.cs` | 255 | Settings-only WASAPI mic capture + WASAPI loopback output peak meter, live `NoiseGate` + `MicDenoiser` (RNNoise) preview, `WasapiOut` "Hear yourself" loopback |
| `Controls/SegmentedListBox.cs` | 140 | Custom `ListBox` with a single sliding `Border` selection pill (`220ms CubicEaseOut`) |

Supporting files inspected for exact styling, converters, localization, persistence, and cross-area calls:
`Themes/Components.axaml` (137), `Themes/DarkTheme.axaml` (757), `Themes/Palette.axaml` (287), `Themes/Tokens.axaml` (32), `Core/UiConverters.cs` (94), `Core/FxAnimations.cs` (47), `Core/Localizer.cs` (26), `Core/LanguageToFlagConverter.cs` (28), `Services/AudioDeviceWatcher.cs` (41), `Services/AudioSessionService.cs` (124), `Services/VfrCapture/NoiseGate.cs` (77), `Services/VfrCapture/MicDenoiser.cs` (211), `Services/VfrCapture/EncoderSelector.cs` (185), `Services/VfrCapture/WebcamCaptureSource.cs` (168), `Services/VfrCapture/GameDetector.cs` (278), `Services/AppUpdateService.cs` (166), `Services/FileLog.cs` (150), `Services/GlobalHotkeyManager.cs` (219), `Services/GlobalHotkeyService.cs` (373), `Services/RecorderOptions.cs` (114), `ViewModels/MainViewModel.cs` (744), `Views/StatsHudWindow.cs` (168), `App.axaml` (351), `App.axaml.cs` (425), `Assets/Langs/en.axaml` (265).

---

## 2. Features

1. **Four-tab Settings Navigation**: Video (`0`), Audio (`1`), Overlay (`2`), General (`3`) via a sliding-pill `SegmentedListBox`. Switching tabs resets scroll to top (`0,0`) and plays a `240ms` slide-fade-in animation (`SettingsView.axaml:150-156`, `SettingsView.axaml.cs:71-88`).
2. **Pending-Changes Banner**: When recording is active (`_engine.IsRecording == true`) and any capture-pipeline setting changes, a yellow-amber notice (`Settings_PendingChanges`) appears at the top right (`SettingsViewModel.cs:1727-1744`, `SettingsView.axaml:143-146`).
3. **Hardware-Tailored Presets (Video tab)**: Three clickable cards (`Performance`, `Balanced` [Recommended badge], `Quality`) computed live from `MachineProfile` via `PresetResolver`. Highlights the card matching current settings; clicking applies resolution, FPS, codec, bitrate, and buffer duration (`SettingsViewModel.Presets.cs:13-132`, `PresetResolver.cs:48-130`).
4. **Low-Disk Auto-Cleanup Recommendation Banner**: Shown under presets when free disk space on the library drive is `> 0` and `< 40 GiB` and `AutoCleanupEnabled` is false (`SettingsViewModel.Presets.cs:78-82`).
5. **Recording & Encoding Controls (Video tab)**: Buffer duration (`30s`–`15m`), display monitor (keyed by PnP `StableId`), FPS segmented strip (capped by monitor refresh rate + `Custom` numeric input), output downscale resolution (capped by monitor native height, tier-colored), video codec segmented strip (only codec families probed usable on this machine + `Auto`), bitrate dropdown + conditional `3–100 Mbps` custom slider, recording GPU selector, and container format (`mp4`, `mov`, `mkv`) (`SettingsView.axaml:217-310`).
6. **Intensive-Quality Warning Modal**: One-time-per-session confirmation modal when manual settings exceed the `Quality` preset ceiling for this machine (`SettingsViewModel.cs:394-543`, `SettingsView.axaml:808-821`).
7. **System Audio & Per-App Mixing (Audio tab)**: Toggle between `All PC audio` and `Specific apps`, plus `Match audio mode to capture` (auto-switches at record time based on game vs. desktop target). Independent enable checkboxes and volume sliders for System Sound (`0–100%`), Microphone (`0–200%` with amber boost indicator `> 100%`), Game Sound (`0–100%`), and live-discovered active audio apps (`0–100%`) excluding detected games (`SettingsView.axaml:406-477`, `SettingsViewModel.cs:757-895`).
8. **Live Audio Devices, Meters, Noise Gate, RNNoise & Monitoring (Audio tab)**: Output device selector + live loopback meter; Microphone selector; Input Sensitivity (`Auto` adaptive floor vs. interactive `480px` draggable threshold bar with dB label `-100 dB`..`0 dB`); RNNoise noise suppression toggle; `"Hear yourself"` live mic-to-output preview toggle (auto-disables when leaving Audio tab, navigating away, or losing window focus); Mic Mono/Stereo segmented strip (`SettingsView.axaml:479-584`, `SettingsViewModel.AudioMeter.cs:15-212`).
9. **Audio Advanced (Audio tab)**: Push-to-talk key capture + enable toggle; Separate audio tracks toggle (`SettingsView.axaml:586-618`).
10. **Interactive Overlay Designer (Overlay tab)**: Master `Video overlays` toggle, live `736×414` 16:9 drag/edge-resize/scroll-wheel stage for Webcam PiP, Keyboard+Mouse block, and Resource Monitor HUD, plus per-overlay toggles, camera selector (async WinRT enumeration), and Resource Monitor detail level (`FPS`, `FPS + CPU/GPU`, `FPS + CPU/GPU + RAM`) (`SettingsView.axaml:315-400`, `SettingsView.axaml.cs:92-255`).
11. **Appearance & Language (General tab)**: Theme (`Light`, `Dark`, `System`) with whole-window `280ms` cross-fade; 17 UI languages with crisp vector SVG-style flags, immediate live dictionary swap, and localized option rebuild (`SettingsView.axaml:625-654`, `SettingsViewModel.cs:1183-1327`, `App.axaml.cs:71-190`).
12. **Global Hotkey Rebinding (General tab)**: Four global Win32 hotkeys (`Save replay`, `Screenshot`, `Pause recording`, `Start/stop recording`) captured via SharpHook global hook; supports 1–3 key combos and `Escape` to unbind (`—`) (`SettingsView.axaml:656-717`, `SettingsViewModel.cs:141-278, 1565-1700`).
13. **Startup & Performance (General tab)**: `Start with Windows` (HKCU `Run` key, self-heals path on launch), `Start minimized` (tray-only launch), `Start recording automatically`, `Disable Windows Game Mode` (HKCU `GameBar` keys, default ON, re-asserted on launch) (`SettingsView.axaml:719-750`, `SettingsViewModel.cs:1083-1178, 1535-1542`).
14. **Storage, Folder, Updates & Diagnostics (General tab)**: Library auto-cleanup toggle + storage limit (`10–500 GB`), Library folder picker, version display + GitHub Releases Inno-Setup auto-updater (manual button + silent 3s startup check), `Open folder` (logs), and `Export .zip` (all logs bundled into `Documents\Lag\Lag-diagnostics-<timestamp>.zip` and selected in Explorer) (`SettingsView.axaml:752-803`, `SettingsViewModel.cs:1331-1472, 1702-1723`).
15. **Hot-Plug Audio Device Watcher**: `IMMNotificationClient` debounced at `300ms` refreshes mic/output dropdowns and restarts the live audio meter if running (`SettingsViewModel.AudioMeter.cs:27-68`, `AudioDeviceWatcher.cs:14-40`).
16. **Debounced Persistence**: All edits coalesce into a `350ms` save timer writing `%AppData%\Lag\settings.json`, flushed synchronously on app shutdown/tray exit (`SettingsViewModel.cs:1757-1875`, `App.axaml.cs:304-308, 400-423`).

---

## 3. UI

### 3.1 Theme Tokens & Shared Control Styles

All colors resolve dynamically by theme (`Dark` vs `Light` in `Themes/Palette.axaml:21-284`). Theme-invariant radii/fonts are in `Themes/Tokens.axaml:13-24`:
- Fonts: `FontUi` = `Inter`, `FontDisplay` = `Inter`, `FontMono` = `Cascadia Code, Consolas, Courier New, monospace` (`Tokens.axaml:13-15`). Global `TextBlock` fallback: `Inter, Segoe UI, sans-serif`, `Foreground = FgBrush` (`DarkTheme.axaml:16-19`).
- Radii: `RadCard = 15`, `RadGeneral = 13`, `RadPill = 11`, `RadButton = 10`, `RadInput = 9`, `RadSeg = 7`, `RadKbd = 6` (`Tokens.axaml:18-24`).

| Token | Dark Hex | Light ("cream") Hex | Source |
|---|---|---|---|
| `SurfaceApp` | `#FF0C0D11` | `#FFEAE3D1` | `Palette.axaml:33,167` |
| `SurfaceCard` | `#FF14161C` | `#FFF8F4EA` | `Palette.axaml:34,168` |
| `SurfaceInput` | `#FF1A1D25` | `#FFE2D6BA` | `Palette.axaml:35,169` |
| `SurfaceHover` | `#FF1E222B` | `#FFE2DAC4` | `Palette.axaml:36,170` |
| `SurfaceHover2` | `#FF22262F` | `#FFDCD3BB` | `Palette.axaml:37,171` |
| `BorderFaint` | `#0EFFFFFF` | `#0F2A2415` | `Palette.axaml:40,174` |
| `BorderCard` | `#12FFFFFF` | `#182A2415` | `Palette.axaml:41,175` |
| `BorderInput` | `#17FFFFFF` | `#242A2415` | `Palette.axaml:42,176` |
| `BorderStrong` | `#1AFFFFFF` | `#302A2415` | `Palette.axaml:43,177` |
| `Accent` | `#FF22D3EE` | `#FFA9722E` | `Palette.axaml:25,159` |
| `AccentDim` | `#1F22D3EE` | `#29A9722E` | `Palette.axaml:26,160` |
| `SegSelBrush` | `#FF2A313B` | `#FFF8F4EA` | `Palette.axaml:69,203` |
| `SegHoverBrush` | `#FF20262E` | `#FFE2DAC4` | `Palette.axaml:99,232` |
| `VideoBrush` | `#FF07090C` | `#FF16140E` | `Palette.axaml:70,204` |
| `SurfaceBrush` | `#FF20252D` | `#FFEBE4D2` | `Palette.axaml:65,199` |
| `SurfaceHoverBrush` | `#FF272D36` | `#FFE2DAC4` | `Palette.axaml:66,200` |
| `CardBrush` | `#FF181C23` | `#FFF8F4EA` | `Palette.axaml:67,201` |
| `Track14Brush` | `#24FFFFFF` | `#262A2415` | `Palette.axaml:86,220` |
| `TextHi` | `#FFECEDEF` | `#FF2A2820` | `Palette.axaml:46,180` |
| `TextRow` | `#FFDDE0E4` | `#FF36332A` | `Palette.axaml:47,181` |
| `TextMid` | `#FF9AA0AA` | `#FF6E6A5A` | `Palette.axaml:50,184` |
| `TextMuted` | `#FF8B919C` | `#FF807B69` | `Palette.axaml:51,185` |
| `TextFaint` | `#FF7C828D` | `#FF928D78` | `Palette.axaml:52,186` |
| `TextFaintest` | `#FF6A707B` | `#FFA39D86` | `Palette.axaml:53,187` |
| `FgBrush` | `#FFE7EAEE` | `#FF2A2820` | `Palette.axaml:76,210` |
| `Fg2Brush` | `#FF9AA3AD` | `#FF6E6A5A` | `Palette.axaml:77,211` |

Shared reusable classes used inside `SettingsView.axaml`:
- `Border.rcard`: `Background=SurfaceCard`, `BorderBrush=BorderCard`, `BorderThickness=1`, `CornerRadius=15`, `ClipToBounds=True` (`Components.axaml:12-18`).
- `Border.rdivider`: `Height=1`, `Background=BorderFaint` (`Components.axaml:21-24`).
- `Grid.row`: `Margin=18,15` (`SettingsView.axaml:11-13`).
- `TextBlock.rgroup`: `FontFamily=FontMono`, `FontSize=11`, `Foreground=TextFaintest`, `Margin=2,0,0,11` (`Components.axaml:27-32`).
- `TextBlock.rrow`: `FontSize=14`, `FontWeight=Medium`, `Foreground=TextRow` (`Components.axaml:35-39`).
- `TextBlock.rsub`: `FontSize=12.5`, `Foreground=TextFaint`, `TextWrapping=Wrap` (`Components.axaml:42-46`).
- `TextBlock.rmono`: `FontFamily=FontMono`, `Foreground=TextFaint` (`Components.axaml:49-52`).
- `Button.rchip`: `Background=SurfaceInput`, `Foreground=TextHi`, `BorderThickness=1`, `BorderBrush=BorderStrong`, `CornerRadius=9`, `Padding=15,8`, `FontSize=12.5`, `FontWeight=SemiBold`, `Cursor=Hand`, hover `Background=SurfaceHover2` (`140ms`), press `scale(0.97)` (`80ms`) (`Components.axaml:109-125`, `DarkTheme.axaml:36-47`). Variant `.active`: `Foreground=White`, `BorderBrush=Accent`, `Background=Accent` (also on hover) (`SettingsView.axaml:101-110`).
- `Border.kbd`: `Background=SurfaceBrush`, `CornerRadius=6`, `Padding=8,4`; child `TextBlock`: `FontSize=12`, `FontWeight=Medium`, `Foreground=FgBrush` (`DarkTheme.axaml:322-331`). Variant `.listening`: infinite `1s` `SineEaseInOut` opacity pulse `1 -> 0.45 -> 1` (`DarkTheme.axaml:342-350`). Host `Button.bare` sets `Padding=0`, `Background=Transparent`, `BorderThickness=0`, `Cursor=Hand`; `:disabled` sets `Opacity=0.45` and transparent content background (`DarkTheme.axaml:334-339`).
- `ComboBox`: `Background=SurfaceBrush`, `Foreground=FgBrush`, `CornerRadius=8`, `MinHeight=30`, `Padding=12,5,8,5`, `FontSize=13`, `BorderThickness=0`; hover `SurfaceHoverBrush` (`150ms`); chevron `Fg2Brush` rotates `0deg -> 180deg` (`180ms CubicEaseOut`) on `:dropdownopen`. Popup corner radius `10`, padding `4`, bg `ComboBoxDropDownBackground`. `ComboBoxItem`: `FontSize=13`, `Fg2Brush`, `Padding=10,6`, `Margin=0,1`, `CornerRadius=6`, hover `SurfaceBrush`, selected `SegSelBrush` + `FgBrush` (`DarkTheme.axaml:353-407`, `App.axaml:226-227`).
- `ToggleSwitch`: Custom template (`40×22`, `CornerRadius=11`, `BorderThickness=1`, `Cursor=Hand`). Track colors from `ToggleSwitchFillOff`/`On`, `ToggleSwitchStrokeOff`/`On` (`160ms` transition). Knob: `16×16` `Ellipse`, `Margin=2,0,0,0`, translates `0px -> 18px` (`160ms CubicEaseOut`) when checked. `:disabled` sets `Opacity=0.45` (`App.axaml:256-320`).
- `Slider`: `Height=20`, `Foreground=AccentBrush`, `Background=Track14Brush`, track height `4px`, white circular thumb `12×12` (`CornerRadius=6`) scaling `1 -> 1.25` on hover (`120ms CubicEaseOut`). `:disabled` sets `Opacity=0.45` (`DarkTheme.axaml:553-589`, `App.axaml:232-248`).
- `TextBox`: `Background=SurfaceBrush`, `Foreground=FgBrush`, `CornerRadius=8`, `MinHeight=30`, `Padding=9,4`, `FontSize=13`, `BorderThickness=1`, `BorderBrush=Transparent`, `CaretBrush=AccentBrush`, `SelectionBrush=TextSelectionBrush`; hover `SurfaceHoverBrush`; focus `BorderBrush=AccentBrush` (`DarkTheme.axaml:475-503`).
- `ScrollViewer`: `VerticalScrollBarVisibility=Auto`, `AllowAutoHide=True`, custom slim `10px` vertical scrollbar with `6px` wide (`MinHeight=28`, `CornerRadius=3`) thumb that is transparent at rest and fades to `ScrollThumbBrush` on hover (`150ms`) and `ScrollThumbPressedBrush` when pressed (`DarkTheme.axaml:703-742`).
- `SegmentedListBox` (`ListBox.segctl`, `Controls/SegmentedListBox.cs:22-140`, `DarkTheme.axaml:259-307`):
  - Track: `Background=SurfaceInput`, `CornerRadius=8`, `Padding=3`, `BorderThickness=0`, horizontal `StackPanel` (`Spacing=2`). No `ScrollViewer`.
  - Selection pill: single `Border` (`CornerRadius=6`, `Background=SegSelBrush`, `HorizontalAlignment=Left`, `VerticalAlignment=Top`) in an underlay `Panel` behind `PART_ItemsPresenter`.
  - First show snaps into place (`Transitions = null`), then arms `220ms CubicEaseOut` transitions on `RenderTransform` (`translateX/Y`), `Width`, and `Height`. Repositioned on `SelectionChanged` and `LayoutUpdated` (threshold `>= 0.5px` delta). Hidden when `SelectedIndex < 0` or container not arranged.
  - Items: `Padding=10,4`, `CornerRadius=6`, `MinHeight=0`, `FontSize=12`, `Foreground=Fg2Brush`, `Cursor=Hand`, `:pressed` scales `0.95` (`80ms`), `:pointerover` sets item bg `SegHoverBrush` (`150ms`), `:selected` sets item bg `Transparent`, `Foreground=FgBrush`, `FontWeight=Medium`.
- `IntensityToBrushConverter` (`StaticResource Intensity`, `UiConverters.cs:42-60`): Tier `0` → inherit default color (`UnsetValue`), Tier `1` → `#E5B84C` (amber), Tier `>= 2` → `#E0564C` (red).

### 3.2 Outer Shell & Header (`SettingsView.axaml:128-167`)

- Root `Grid` (`Background=SurfaceApp`) containing:
  1. `ScrollViewer` (`x:Name="SettingsScroll"`, `VerticalScrollBarVisibility="Auto"`):
     - Centered `StackPanel` (`Width=800`, `HorizontalAlignment=Center`, `Margin="0,34,0,64"`).
     - **Header** (`Grid ColumnDefinitions="*,Auto"`):
       - Col 0 (`StackPanel Spacing="4"`): Title (`{DynamicResource Settings_Title}`, `FontDisplay`, `FontSize=32`, `Bold`, `TextHi`) + Subtitle (`{DynamicResource Settings_Subtitle}`, `FontSize=13.5`, `TextMuted`).
       - Col 1: Pending changes warning (`{DynamicResource Settings_PendingChanges}`, `FontSize=11`, `#F0B45A`, `VerticalAlignment=Bottom`, `TextWrapping=Wrap`, `MaxWidth=280`, `TextAlignment=Right`, `IsVisible="{Binding HasPendingChanges}"`).
     - **Tab strip** (`SegmentedListBox.segctl`, `HorizontalAlignment=Left`, `Margin="0,26,0,0"`, `SelectedIndex="{Binding SelectedSettingsTab, Mode=TwoWay}"`, `SelectionChanged="OnTabsChanged"`):
       - Index `0`: `{DynamicResource Tab_Video}`
       - Index `1`: `{DynamicResource Tab_Audio}`
       - Index `2`: `{DynamicResource Settings_Group_Overlay}`
       - Index `3`: `{DynamicResource Tab_General}`
     - *(Dead markup at `SettingsView.axaml:157-166`: `<Border IsVisible="False">` containing legacy 3-tab `Button.rseg` buttons.)*
  2. **Intensive-Quality Modal Overlay** (`Border ZIndex="100"`, `Background="#B0000000"`, `IsVisible="{Binding ShowIntensiveWarning}"`, `SettingsView.axaml:809-821`):
     - Centered card: `Width=560`, `MaxWidth=560`, `Padding=28`, `CornerRadius=14`, `Background=SurfaceCard`, `BorderBrush=BorderStrong`, `BorderThickness=1`, `BoxShadow="0 12 40 0 #80000000"`.
     - `StackPanel Spacing="14"`:
       1. `TextBlock` (`Text="{Binding IntensiveWarnTitle}"`, `FontDisplay`, `FontSize=18`, `Bold`, `TextHi`, `Wrap`)
       2. `TextBlock` (`Text="{Binding IntensiveWarnSummary}"`, `FontSize=13`, `Medium`, `#E5B84C`, `Wrap`)
       3. `TextBlock` (`Text="{Binding IntensiveWarnBody}"`, `FontSize=13`, `TextMid`, `Wrap`, `LineHeight=20`)
       4. `Button.rchip` (`Content="{Binding IntensiveWarnOk}"`, `HorizontalAlignment=Right`, `Click="OnIntensiveUnderstood"`)

### 3.3 Tab 0 — Video (`TabPanel0`, `Margin="0,28,0,0"`, `Spacing="26"`, `SettingsView.axaml:168-312`)

Controls in exact top-to-bottom order:

1. **PRESETS Section** (`StackPanel Spacing="12"`):
   - Heading: `TextBlock.rgroup` (`Margin=0`, `{DynamicResource Preset_SectionTitle}`) + Subtitle: `TextBlock.rsub` (`Margin=0`, `{DynamicResource Preset_Subtitle}`).
   - 3-column `UniformGrid` (`ItemsSource="{Binding Presets}"`):
     - Each `PresetCard` rendered as `Border.presetcard` (`Classes.active="{Binding IsActive}"`, `Margin=4`, `Tapped="OnPresetClick"`):
       - Styling: `SurfaceCard`, `BorderCard`, `BorderThickness=1.5`, `CornerRadius=12`, `Padding="15,13"`, `Cursor=Hand`, `120ms` brush transitions; hover `SurfaceHover` + `BorderStrong`; `.active` sets `BorderBrush=Accent`.
       - Row 1 (`Grid *,Auto`): `Title` (`FontSize=15.5`, `SemiBold`, `TextHi`) + `"Recommended"` badge (`Border.badge`: `Accent`, `CornerRadius=6`, `Padding="6,2"`, `IsVisible="{Binding IsRecommended}"`, text `{DynamicResource Preset_RecommendedBadge}` `10px SemiBold White`).
       - Row 2: `Description` (`12px`, `TextMuted`).
       - Row 3: `Subtitle` (`11.5px`, `Accent`, `Wrap`, `Margin="0,2,0,0"`).
   - **Disk-low hint banner** (`Border.hint`: `SurfaceInput`, `CornerRadius=9`, `Padding="14,11"`, `IsVisible="{Binding ShowCleanupHint}"`):
     - Horizontal (`Spacing=9`): `"💡"` (`14px`) + `CleanupHintText` (`12.5px`, `TextHi`, `Wrap`).

2. **ADVANCED Heading**: `TextBlock.rgroup` (`{DynamicResource Preset_Advanced}`).

3. **RECORDING Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Recording}` + `Border.rcard`):
   - Row 1: `{DynamicResource Settings_BufferLength}` | `ComboBox` (`BufferOptions` → `SelectedBuffer`).
   - Divider (`Border.rdivider`).
   - Row 2: `{DynamicResource Settings_Display}` | `ComboBox` (`Monitors` → `SelectedMonitor`).
   - Divider.
   - Row 3: `{DynamicResource Settings_FrameRate}` | Horizontal `StackPanel` (`Spacing=8`):
     - `TextBox` (`Width=56`, `Text="{Binding CustomFps}"`, `IsVisible="{Binding IsCustomFps}"`).
     - `SegmentedListBox.segctl` (`FpsOptions` → `SelectedFps`), item template `TextBlock` displaying `Display` with `Foreground="{Binding Tier, Converter={StaticResource Intensity}}"`.
   - Divider.
   - Row 4: `{DynamicResource Settings_OutputResolution}` | `ComboBox` (`ResolutionOptions` → `SelectedResolution`), item template `TextBlock` displaying `Display` with `Intensity` foreground.

4. **ENCODING Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Encoding}` + `Border.rcard`):
   - Row 1: `{DynamicResource Settings_Codec}` | `SegmentedListBox.segctl` (`CodecOptions` → `SelectedCodec`).
   - Divider.
   - Row 2: `{DynamicResource Settings_Bitrate}` | Horizontal `StackPanel` (`Spacing=12`):
     - `ComboBox` (`MinWidth=120`, `BitrateOptions` → `SelectedBitrate`), item template `TextBlock` displaying `Display` with `Intensity` foreground.
     - `Slider` (`Width=120`, `Minimum=3`, `Maximum=100`, `TickFrequency=1`, `IsSnapToTickEnabled=True`, `Value="{Binding BitrateSliderValue}"`, `IsVisible="{Binding IsCustomBitrate}"`).
     - Custom readout (`Spacing=4`, `IsVisible="{Binding IsCustomBitrate}"`): `{Binding BitrateDisplayMbps}` + `{DynamicResource Settings_Bitrate_CustomSuffix}` (`13px`, `TextHi`).
   - Divider.
   - Row 3: `{DynamicResource Settings_Gpu}` | `ComboBox` (`GpuOptions` → `SelectedGpu`).
   - Divider.
   - Row 4: `{DynamicResource Settings_Format}` | `ComboBox` (`FormatOptions` → `SelectedFormat`), item template uppercased via `Upper` converter (`"MP4"`, `"MOV"`, `"MKV"`).

### 3.4 Tab 1 — Audio (`TabPanel1`, `Margin="0,28,0,0"`, `Spacing="26"`, `SettingsView.axaml:402-619`)

*(Note: in XAML, `TabPanel2` appears before `TabPanel1` in source order, but Tab 1 is Audio and Tab 2 is Overlay.)*

1. **SYSTEM SOUND Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_SystemAudio}` + `Border.rcard`):
   - Row 1: `{DynamicResource Settings_AudioMode}` | `SegmentedListBox.segctl` (`SelectedIndex="{Binding AudioModeIndex, Mode=TwoWay}"`): `[AudioMode_All, AudioMode_Apps]`.
   - Divider.
   - Row 2: `{DynamicResource Settings_AudioModeByTarget}` + desc `{DynamicResource Settings_AudioModeByTarget_Desc}` | `ToggleSwitch` (`IsChecked="{Binding AudioModeByTarget}"`).
   - Divider.
   - Row 3 (System audio source, `Grid ColumnDefinitions="Auto,Auto,*,120,42"`, `Margin="18,11"`, `IsVisible="{Binding ShowSystemAudioRow}"`):
     - `CheckBox` (`SystemAudioEnabled`) | `MaterialIcon` (`VolumeHigh`, `20×20`, `Margin="6,0,10,0"`, `TextFaint`) | Hardcoded `"Звук системи"` (`14px`, `TextRow`) | `Slider` (`0..100`, `TickFrequency=1`, `IsSnapToTickEnabled=True`, `Value="{Binding SystemAudioVolume}"`, `IsEnabled="{Binding SystemAudioEnabled}"`) | `TextBlock` (`"{0}%"`, `FontMono`, `11px`, `TextFaint`, right-aligned).
   - Row 4 (Microphone source, `Grid Auto,Auto,*,120,42`, `Margin="18,11"`):
     - `CheckBox` (`MicEnabled`) | `MaterialIcon` (`Microphone`, `20×20`, `TextFaint`) | `{DynamicResource Settings_Microphone}` (`14px`, `TextRow`) | `Slider` (`0..200`, `TickFrequency=1`, `IsSnapToTickEnabled=True`, `Value="{Binding MicVolume}"`, `IsEnabled="{Binding MicEnabled}"`) | `TextBlock` (`"{0}%"`, `FontMono`, `11px`, `TextFaint`, class `.boosted` when `IsMicBoosted` → `#E5B84C`).
   - Row 5 (Game audio source, `Grid Auto,Auto,*,120,42`, `Margin="18,11"`, `IsVisible="{Binding ShowAppsList}"`):
     - `CheckBox` (`GameAudioEnabled`) | `MaterialIcon` (`Gamepad`, `20×20`, `TextFaint`) | Hardcoded `"Звук гри"` (`14px`, `TextRow`) | `Slider` (`0..100`, `TickFrequency=1`, `IsSnapToTickEnabled=True`, `Value="{Binding GameAudioVolume}"`, `IsEnabled="{Binding GameAudioEnabled}"`) | `TextBlock` (`"{0}%"`, `FontMono`, `11px`, `TextFaint`).
   - Row 6 (Per-app list, `ItemsControl` bound to `AudioApps`, `IsVisible="{Binding ShowAppsList}"`):
     - Each `AppAudioItem`: `Grid Auto,Auto,*,120,42`, `Margin="18,11"`: `CheckBox` (`IsEnabled`) | `Image` (`Icon`, `20×20`, `Margin="6,0,10,0"`) | `TextBlock` (`DisplayName`, `14px`, `TextRow`, `CharacterEllipsis`) | `Slider` (`0..100`, `TickFrequency=1`, `IsSnapToTickEnabled=True`, `Volume`, `IsEnabled="{Binding IsEnabled}"`) | `TextBlock` (`"{0}%"`, `FontMono`, `11px`, `TextFaint`).
   - Hint footer (`IsVisible="{Binding ShowAppsList}"`): `{DynamicResource Settings_AppsHint}`, `11.5px`, `TextFaintest`, `Margin="18,2,18,12"`, `Wrap`.

2. **DEVICES Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Devices}` + `Border.rcard`):
   - Row 1: `{DynamicResource Settings_OutputDevice}` | `ComboBox` (`MaxWidth=240`, `OutputDevices` → `SelectedOutputDevice`, item displays `Name`).
   - Row 2 (no divider between Row 1 and Row 2): `{DynamicResource Settings_OutputLevel}` | `Border.meter` (`240×8`, `CornerRadius=4`, `SurfaceInput`, `ClipToBounds=True`) containing `Border.meterfill` (`#FF43B581`, `CornerRadius=4`, `Width = OutputLevel * 240`).
   - Divider.
   - Row 3: `{DynamicResource Settings_Microphone}` | `ComboBox` (`Microphones` → `SelectedMicrophone`, item displays `Name`).
   - Divider.
   - Row 4 (Input sensitivity header, `Margin="18,15,18,4"`): `{DynamicResource Settings_InputSensitivity}` + `{DynamicResource Settings_InputSensitivityHint}` | Horizontal (`Spacing=9`): `{DynamicResource Settings_Auto}` (`12.5px`, `TextFaint`) + `ToggleSwitch` (`InputSensitivityAuto`).
   - Row 5 (Input sensitivity bar, `Margin="18,2,18,15"`, `ColumnDefinitions="*,Auto"`):
     - **When `InputSensitivityAuto` is true**: `Border.gatebar` (`480×12`, `CornerRadius=6`, `SurfaceInput`, `ClipToBounds=True`) containing `Border.meterfill` (`#FF43B581`, `CornerRadius=6`, `Width = MicLevel * 480`). *(Note: `GateThreshold` and `MicGateOpen` exist in the VM, and styles `Border.meterfill.gated` (`#FF5A6472`) and `Border.metermarker` exist in `SettingsView.axaml:75-83`, but are NOT used in the markup for this bar — see Section 9.)*
     - **When `InputSensitivityAuto` is false**: `Border.gatebar` (`480×12`, `CornerRadius=6`, `Cursor=Hand`, `PointerPressed="OnGatePressed"`, `PointerMoved="OnGateMoved"`), layered `Grid`:
       1. Background pass zone: `#3343B581` (`CornerRadius=6`)
       2. Background cut zone (left): `#33CB8438` (`CornerRadius="6,0,0,6"`, `Width = InputSensitivityThreshold * 4.8`)
       3. Live level clip container (`Width = MicLevel * 480`, `ClipToBounds=True`) holding `480px` wide bright pass (`#FF43B581`) + bright cut (`#FFCB8438`, `Width = InputSensitivityThreshold * 4.8`).
       4. Draggable handle (`IsHitTestVisible=False`): transparent spacer (`Width = InputSensitivityThreshold * 4.8`) + `Border.gatehandle` (`Width=4`, `CornerRadius=2`, `White`).
     - Col 1 (when `!InputSensitivityAuto`): `ThresholdDb` (`Margin="12,0,0,0"`, `FontMono`, `11px`, `TextFaint`).
   - Divider.
   - Row 6: `{DynamicResource Settings_NoiseSuppression}` + `{DynamicResource Settings_NoiseSuppressionHint}` | Vertical `StackPanel` (`Spacing=3`): `ToggleSwitch` (`NoiseSuppression`, right-aligned) + hardcoded `"Powered by RNNoise"` (`10.5px`, `TextFaint`, right-aligned).
   - Divider.
   - Row 7: `{DynamicResource Settings_HearYourself}` + `{DynamicResource Settings_HearYourselfHint}` | `Button.rchip` (`Classes.active="{Binding HearYourself}"`, `Command="{Binding ToggleHearYourselfCommand}"`, `Content="{Binding HearYourselfButtonText}"`).
   - Divider.
   - Row 8: `{DynamicResource Settings_MicChannels}` | `SegmentedListBox.segctl` (`SelectedIndex="{Binding MicChannelIndex, Mode=TwoWay, Converter={StaticResource OneMinus}}"`): index 0 = `{DynamicResource Channels_Mono}`, index 1 = `{DynamicResource Channels_Stereo}` (inverted via `OneMinus`: UI 0/Mono ↔ VM 1, UI 1/Stereo ↔ VM 0).

3. **ADVANCED Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Advanced}` + `Border.rcard`):
   - Row 1: `{DynamicResource Settings_Ptt}` + `{DynamicResource Settings_Ptt_Desc}` | Horizontal (`Spacing=12`):
     - `Button.bare` (`Command="{Binding CapturePttKeyCommand}"`, `IsEnabled="{Binding PushToTalkEnabled}"`, `ToolTip.Tip="{DynamicResource Settings_Hotkey_Desc}"`): shows `Border.kbd` with `{Binding PttKeyDisplayText}` when `!IsCapturingPttKey`, or `Border.kbd.listening` with `"..."` when `IsCapturingPttKey`.
     - `ToggleSwitch` (`PushToTalkEnabled`).
   - Divider.
   - Row 2: `{DynamicResource Settings_SeparateTracks}` + `{DynamicResource Settings_SeparateTracks_Desc}` | `ToggleSwitch` (`SeparateAudioTracks`).

### 3.5 Tab 2 — Overlay (`TabPanel2`, `Margin="0,28,0,0"`, `Spacing="26"`, `SettingsView.axaml:314-400`)

1. **OVERLAY Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Overlay}` + `Border.rcard`):
   - Row 1: `{DynamicResource Overlay_Enable}` + `{DynamicResource Overlay_Enable_Desc}` | `ToggleSwitch` (`OverlayEnabled`).
   - Interactive preview container (`Margin="18,4,18,15"`, `SurfaceInput`, `CornerRadius=10`, `Padding=10`, `IsVisible="{Binding OverlayEnabled}"`):
     - Stage frame (`CornerRadius=8`, `ClipToBounds=True`, `Background=VideoBrush`): `Canvas x:Name="OverlayStage"` (`Width=736`, `Height=414`, 16:9).
     - Three draggable/resizable `Border.ovbox` items (`AccentDim` bg, `Accent` `1px` border, `CornerRadius=6`, `220ms CubicEaseOut` opacity fade-in `0->1` on creation):
       1. `OvKeysBox` (`IsVisible="{Binding KeysShown}"`): `KeyboardOutline` (`22×22`) + `MouseOutline` (`18×18`), `Spacing=5`, `TextMid`.
       2. `OvWebcamBox` (`IsVisible="{Binding WebcamShown}"`): `Webcam` (`24×24`, `TextMid`).
       3. `OvStatsBox` (`IsVisible="{Binding StatsShown}"`): `Speedometer` (`16×16`, `TextMid`).
     - Hint below stage: `{DynamicResource Overlay_WheelHint}` (`11px`, `TextFaint`, centered, `Margin="0,8,0,2"`).
   - Divider (`IsVisible="{Binding OverlayEnabled}"`).
   - Row 2 (`Grid *,Auto,Auto`, `IsVisible="{Binding OverlayEnabled}"`): `{DynamicResource Overlay_Camera}` | `ComboBox` (`MinWidth=180`, `Margin="0,0,14,0"`, `Webcams` → `SelectedWebcam`, `IsEnabled="{Binding WebcamShown}"`) | `ToggleSwitch` (`WebcamShown`).
   - Divider (`IsVisible="{Binding OverlayEnabled}"`).
   - Row 3 (`IsVisible="{Binding OverlayEnabled}"`): `{DynamicResource Overlay_Keys}` | `ToggleSwitch` (`KeysShown`).
   - Divider (`IsVisible="{Binding OverlayEnabled}"`).
   - Row 4 (`Grid *,Auto,Auto`, `IsVisible="{Binding OverlayEnabled}"`): `{DynamicResource Overlay_Stats}` | `ComboBox` (`MinWidth=180`, `Margin="0,0,14,0"`, `SelectedIndex="{Binding StatsDetail, Mode=TwoWay}"`, `IsEnabled="{Binding StatsShown}"`) with literal items: `"FPS"`, `"FPS + CPU/GPU"`, `"FPS + CPU/GPU + RAM"` | `ToggleSwitch` (`StatsShown`).

### 3.6 Tab 3 — General (`TabPanel3`, `Margin="0,28,0,0"`, `Spacing="26"`, `SettingsView.axaml:621-803`)

1. **Appearance & Language Card** (no `rgroup` header above it; `Border.rcard`):
   - Row 1: `{DynamicResource Settings_Theme}` + `{DynamicResource Settings_Theme_Desc}` | `SegmentedListBox.segctl` (`ThemeOptions` → `SelectedTheme`).
   - Divider.
   - Row 2: `{DynamicResource Settings_Language}` | `ComboBox` (`LanguageOptions` → `SelectedLanguage`), item template: horizontal `StackPanel` (`Spacing=8`) with `17×12` rounded (`CornerRadius=3`) `Image` (`Code` via `LangFlag` converter) + `Display` text.

2. **HOTKEY Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Hotkey}` + `Border.rcard`):
   Four identical rows (`Button.bare`, `ToolTip.Tip="{DynamicResource Settings_Hotkey_Desc}"`, showing horizontal `Spacing=6` `Border.kbd` chips when idle or pulsing `Border.kbd.listening` with `{DynamicResource Settings_Hotkey_Listening}` while capturing):
   - Row 1: `{DynamicResource Settings_Hotkey_SaveReplay}` | `CaptureHotkeyCommand` / `HotkeyParts` / `IsCapturingHotkey`
   - Divider.
   - Row 2: `{DynamicResource Settings_Hotkey_Screenshot}` | `CaptureScreenshotHotkeyCommand` / `ScreenshotHotkeyParts` / `IsCapturingScreenshotKey`
   - Divider.
   - Row 3: `{DynamicResource Settings_Hotkey_Pause}` | `CapturePauseHotkeyCommand` / `PauseHotkeyParts` / `IsCapturingPauseKey`
   - Divider.
   - Row 4: `{DynamicResource Settings_Hotkey_Record}` | `CaptureRecordHotkeyCommand` / `RecordHotkeyParts` / `IsCapturingRecordKey`

3. **STARTUP & PERFORMANCE Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Startup}` + `Border.rcard`):
   - Row 1: `{DynamicResource Settings_StartWithWindows}` | `ToggleSwitch` (`StartWithWindows`).
   - Divider.
   - Row 2: `{DynamicResource Settings_StartMinimized}` + `{DynamicResource Settings_StartMinimized_Desc}` | `ToggleSwitch` (`StartMinimized`).
   - Divider.
   - Row 3: `{DynamicResource Settings_AutoStart}` | `ToggleSwitch` (`AutoStartRecording`).
   - Divider.
   - Row 4: `{DynamicResource Settings_DisableGameMode}` + `{DynamicResource Settings_DisableGameMode_Desc}` (`MaxWidth=560`, `HorizontalAlignment=Left`) | `ToggleSwitch` (`DisableGameMode`).

4. **STORAGE & ABOUT Card** (`TextBlock.rgroup` `{DynamicResource Settings_Group_Storage}` + `Border.rcard`):
   - Row 1: `{DynamicResource Settings_AutoCleanup}` + `{DynamicResource Settings_AutoCleanup_Desc}` | `ToggleSwitch` (`AutoCleanupEnabled`).
   - Divider.
   - Row 2 (`IsEnabled="{Binding AutoCleanupEnabled}"`): `{DynamicResource Settings_StorageLimit}` | `ComboBox` (`StorageLimitOptions` → `SelectedStorageLimit`, displays `"{Gb} GB"`).
   - Divider.
   - Row 3: `{DynamicResource Settings_LibraryFolder}` | Horizontal (`Spacing=10`): `TextBlock.rmono` (`LibraryPath`, `FontSize=11.5`, `MaxWidth=300`, `CharacterEllipsis`, `ToolTip.Tip="{Binding LibraryPath}"`) + `Button.rchip` (`{DynamicResource Settings_Hotkey_Change}`, `BrowseFolderCommand`).
   - Divider.
   - Row 4: `{DynamicResource About_Group}` + subline (when `UpdateStatus` is empty: `{DynamicResource About_Version}` + `AppVersion` with `Spacing=4`; when non-empty: `UpdateStatus`) | `Button.rchip` (`{DynamicResource About_CheckUpdates}`, `CheckForUpdatesCommand`, `IsEnabled="{Binding !IsCheckingForUpdate}"`).
   - Divider.
   - Row 5 (`Grid *,Auto,Auto`): `{DynamicResource Settings_Logs}` | `Button.rchip` (`Margin="0,0,6,0"`, `{DynamicResource Settings_OpenLogs}`, `OpenLogsFolderCommand`) | `Button.rchip` (`{DynamicResource Settings_SaveDiagnostics}`, `SaveDiagnosticsZipCommand`).

---

## 4. State & Logic

### 4.1 Initialization Sequence (`SettingsViewModel.cs:1474-1559`)

1. `_isInitializing = true` suppresses all `SaveSettings()` and `SaveSettingsRestartRequired()` calls during setup (`SettingsViewModel.cs:39, 1488, 1741, 1760`).
2. Establish defaults:
   - `SelectedLanguage` = `"en"` (`LanguageOptions[0]`)
   - `SelectedStorageLimit` = `50 GB` (`StorageLimitOptions[2]`)
   - `GpuOptions` = `BuildGpuOptions()` (`Index=-1` `"Auto (<gpus[0].Name>)"`, plus `"GPU {i}: {Name}"`); `SelectedGpu = GpuOptions[0]`
   - `RebuildLocalizedOptions()` builds `BufferOptions` (default `300s` / 5 min), `ResolutionOptions` (default `0` / Native), `CodecOptions` (default `""` / Auto), `ThemeOptions` (default `"dark"`), `BitrateOptions` (default `-1` / Auto), `FpsOptions` (default `30`), `OutputDevices` (default `""` / Default).
   - `AppVersion` = `ResolveAppVersion()` (`"Debug/Dev"` if `DevOrPortable`, else `Version.ToString(3)`).
   - `LibraryPath` = `%USERPROFILE%\Videos\Lag`.
3. Enumerate devices BEFORE loading JSON: `RefreshMonitors()` (selects primary or first), `RefreshMicrophones()`, `RefreshOutputDevices()`.
4. `LoadSettings()` reads `settings.json` and overrides defaults/selections.
5. `InitPresets()` creates the 3 `PresetCard`s, evaluates `HardwareProfiler.Capture(LibraryPath)` lazily, updates subtitles/active/recommended state and low-disk hint.
6. `InitDeviceWatcher()` registers WASAPI `IMMNotificationClient`.
7. `RefreshWebcamsAsync()` enumerates `DeviceClass.VideoCapture` on thread pool and posts back to UI thread, restoring `_savedWebcamId` under `_restoringWebcam = true` (falls back to first webcam).
8. `_isInitializing = false`.
9. Post-init side effects:
   - Apply all 4 global hotkeys to `GlobalHotkeyService`.
   - If `DisableGameMode == true`, call `ApplyDisableGameMode(true)`.
   - If `StartWithWindows == true`, call `ApplyStartWithWindows(true)` (self-heals moved exe path).
   - `ApplyPttToManager()`.
   - Subscribe `_hotkeyManager.HotkeyCaptured += OnHotkeyCaptured`.
   - Create `_audioAppsTimer` (`4s` interval) and call `UpdateAudioAppsScanner()`.

### 4.2 Option List Generation & Caps

- **Buffer durations** (`SettingsViewModel.cs:1273-1285`): `[30s, 1m, 2m, 5m, 10m, 15m]`. Labels use `Time_SecShort` (`< 60s`) or `Time_MinShort`.
- **Monitor refresh cap & FPS rungs** (`SettingsViewModel.cs:329-382`):
  - `CurrentRefreshCap()`: takes `SelectedMonitor?.RefreshRate ?? primary?.RefreshRate ?? 0`. If `< 24`, returns `360`; else `Math.Max(SnapRefresh(hz), 60)`.
  - `SnapRefresh(hz)`: snaps within `±2 Hz` to `[60, 75, 90, 100, 120, 144, 160, 165, 180, 200, 240, 280, 300, 360, 390, 480, 500]`.
  - FPS rungs: filter `[24, 30, 60, 120, 240, 360]` where `fps <= cap` (at least `[60]`), append `cap` if `cap > rungs[^1]`, then append `Custom` (`Value = 0`).
  - Changing `SelectedMonitor` rebuilds FPS and Resolution lists, preserving prior selection or snapping FPS to nearest `> 0` rung below `cap` (default `30`), and clamping `CustomFps <= cap`.
  - `OnCustomFpsChanged`: if `value > cap`, re-assigns `CustomFps = cap` and returns.
  - `EffectiveFps`: `SelectedFps.Value > 0 ? SelectedFps.Value : Math.Clamp(CustomFps, 1, CurrentRefreshCap())`.
- **Resolution rungs** (`SettingsViewModel.cs:548-578`):
  - `Native` (`TargetHeight = 0`, tier computed from monitor's native height), followed by `[2160, 1440, 1080, 720]` where `h <= native`. Labels: `2160p (4K)`, `1440p (QHD)`, `1080p`, `720p`.
- **Codec options** (`SettingsViewModel.cs:1247-1262, 1291-1296`):
  - `Auto` (`""`), then `AvailableCodecFormats()` queries `EncoderSelector.Available` (MTA thread probe opening each FFmpeg encoder at `1280×720 NV12 60fps 4Mbps`): yields `("H.264", "h264")`, `("H.265 (HEVC)", "hevc")`, `("AV1", "av1")` for tiers present, or `("H.264", "h264")` fallback if empty.
- **Bitrate options & slider** (`SettingsViewModel.cs:664-731, 1307-1315`):
  - `Auto` (`Kbps = -1`), fixed rungs `[3, 5, 7, 10, 15, 20, 25, 30, 50, 70, 100]` Mbps (`Kbps = mbps * 1000`), and `Custom` (`Kbps = 0`).
  - `EffectiveBitrateKbps`: `-1` if Auto, `SelectedBitrate.Kbps` if `> 0`, else `Math.Clamp(CustomBitrateMbps, 3, 100) * 1000`.
  - `BitrateSliderValue` setter: ignores writes if `!IsCustomBitrate`. Rounds and clamps to `[3, 100]`, sets `CustomBitrateMbps`, and if `mbps * 1000` matches a fixed preset in `BitrateOptions`, switches `SelectedBitrate` to that preset (which hides the slider!).
- **Intensity tiers & Intensive warning** (`SettingsViewModel.cs:389-543`):
  - `FpsTier(fps)`: `> 120 → 2`, `> 60 → 1`, `else 0`.
  - `BitrateTier(mbps)`: `> 50 → 2`, `> 30 → 1`, `else 0`.
  - `ResolutionTier(h)`: `> 1440 → 2`, `> 1080 → 1`, `else 0`.
  - `IsIntensiveSelection`: compares against `ceiling = PresetResolver.Resolve(RecordingIntent.Quality, Profile)`. True if `EffectiveFps > ceiling.Fps`, OR (`ceiling.BitrateKbps > 0 && EffectiveBitrateKbps > ceiling.BitrateKbps`), OR `effectiveHeight > ceilH`.
  - `MaybeWarnIntensive()`: skipped if `_isInitializing || _intensiveAcknowledged || _applyingPreset`. Shows modal once per session; `AcknowledgeIntensive()` sets `_intensiveAcknowledged = true` and hides modal.
  - `IntensiveWarnSummary`: takes `IntensiveTexts[lang].Summary` (fallback `"en"`), slices up to and including the last `':'` or `'：'`, and appends `" {ceiling.Fps} FPS / {mbps_or_Auto} / {ceilH}p"`.

### 4.3 Hardware Profiler & Preset Resolver (`HardwareProfiler.cs`, `PresetResolver.cs`)

- **`HardwareProfiler.Capture(libraryPath)`** (`HardwareProfiler.cs:73-187`):
  - `Gpus`: DXGI `Factory1.GetAdapter1(i)` skipping `VendorId == 0x1414` (Microsoft Basic Render Driver). `IsIntegrated = DedicatedVramBytes < 1 GiB`. `PrimaryGpu` = adapter with max `DedicatedVramBytes`.
  - `HardwareEncoders` / `HasAv1` / `HasHevc` / `HasH264`: from `EncoderSelector.Available`.
  - `CpuName`: HKLM `HARDWARE\DESCRIPTION\System\CentralProcessor\0` → `"ProcessorNameString"` (default `"Unknown CPU"`). `CpuLogicalCores`: `Environment.ProcessorCount`.
  - `RamGiB`: `GlobalMemoryStatusEx().ullTotalPhys / 1024^3`.
  - `DiskFreeGiB` / `DiskTotalGiB`: `DriveInfo` on root of `libraryPath` (fallback `Videos\Lag`, `"C:\\"`).
  - `MonitorWidth` / `MonitorHeight` / `MonitorHz`: primary (or first) monitor from `HardwareDetector.GetAvailableMonitors()`.
- **`PresetResolver.ClassifyGpu(profile)`** (`PresetResolver.cs:135-176`):
  - `PrimaryGpu == null || IsIntegrated` → `Entry`.
  - VRAM prior: `>= 12 GiB → Top`, `>= 8 → High`, `>= 4 → Mid`, `< 4 → Entry`.
  - Name overrides (uppercase):
    - `"RTX 50"` / `"RTX 40"`: 4-digit token `m % 100 >= 70 → Top`, else `High`.
    - `"RTX 30"`: `>= 80 → Top`, else `High`.
    - `"RTX 20"` / `"GTX 16"` / `"GTX 10"`: `min(tier, Mid)`.
    - `"RX 9"` / `"RX 7"`: contains `"900"` or `"800"` → `Top`, else `High`.
    - `"RX 6"`: `"6900"` / `"6800"` → `Top`, `"6700"` → `High`, else `Mid`.
    - `"RX 5"` / `"RX 4"` / `"RX 580"` / `"RX 570"`: `min(tier, Mid)`.
- **`PresetResolver.Resolve(intent, profile)`** (`PresetResolver.cs:50-130`):
  - `cpuOnly = HardwareEncoders.Count == 0`. `monH = MonitorHeight > 0 ? MonitorHeight : 1080`, `refresh = MonitorHz > 0 ? MonitorHz : 60`.
  - Resolution height:
    - `Performance`: `Top → 1440`, else `1080`. `Balanced`: `Top → 0 (native)`, `High → 1440`, else `1080`. `Quality`: `>= High → 0`, `Mid → 1440`, else `1080`.
    - If `cpuOnly`: `CpuLogicalCores >= 8 ? 1080 : 720`.
    - Snapped via `SnapResolution(height, monH)`: if `wanted <= 0 || wanted >= monH` → `0`, else highest `[2160, 1440, 1080, 720]` `<= monH && <= wanted`, fallback `720`.
  - FPS:
    - `Performance`: `>= Mid ? 120 : 60`. `Balanced`: `>= High ? 120 : 60`. `Quality`: `Top ? 120 : 60`.
    - If `cpuOnly`: `60`. Then `Math.Min(fps, refresh)`.
  - Codec: `Performance → HasH264 ? "h264" : ""`, `Balanced → ""`, `Quality → HasAv1 ? "av1" : HasHevc ? "hevc" : HasH264 ? "h264" : ""`.
  - Bitrate: `Quality → QualityBitrateKbps(targetHeight, monH)` (`>=2160p: 100000`, `>=1440p: 50000`, `>=1080p: 30000`, `<1080p: 15000`), others `-1` (Auto; estimated for RAM math as `70000 / 38000 / 20000 / 9000`).
  - RAM ring budget: `clamp(ramBytes * 0.15, 250 MiB, 1500 MiB)`. If fixed `Quality` bitrate × 30s > budget, trim bitrate to `budget * 8 / 30 / 1000` snapped to nearest `[3000, 5000, 7000, 10000, 15000, 20000, 25000, 30000, 50000, 70000, 100000]`.
  - Buffer target: `Performance → 60s`, `Balanced → 120s`, `Quality → 180s`. Clamped by `max(30, ringBudget * 8 / (effectiveKbps * 1000))`, snapped to nearest `[30, 60, 120, 300, 600, 900]`.
  - Disk hint: `RecommendCleanup = DiskFreeGiB > 0 && DiskFreeGiB < 40`, `SuggestedStorageGb` = nearest `[10, 25, 50, 100, 200, 500]` to `(int)(DiskFreeGiB * 0.6)`.
  - Summary format: `"{native|{H}p} · {Fps}fps · {Auto codec|H264|HEVC|AV1} · {Auto quality|{M} Mbps} · {S}s|{M}min buffer"`.

### 4.4 Audio Meter, Noise Gate, Monitoring & Per-App Scanner

- **Activation gate** (`SettingsViewModel.AudioMeter.cs:129-154`):
  - Live meter & per-app scanner run ONLY when `_audioViewAttached && _windowActive && SelectedSettingsTab == 1` (and for scanner, `&& ShowAppsList` where `ShowAppsList = IsAppsMode || AudioModeByTarget`).
  - Stopping meter forces `HearYourself = false`, stops `_meterTimer` and `AudioMonitor`, and resets `MicLevel = 0`, `OutputLevel = 0`.
- **Meter timer** (`33ms` UI `DispatcherTimer`):
  - `MicLevel = Smooth(MicLevel, Math.Min(1.0, Math.Pow(ReadMicPeak(), 0.6)))`
  - `OutputLevel = Smooth(OutputLevel, Math.Min(1.0, Math.Pow(ReadOutputPeak(), 0.6)))`
  - `Smooth(cur, tgt)`: `rate = tgt > cur ? 0.5 : 0.16; return cur + (tgt - cur) * rate;`
  - Mirrors `GateThreshold` and `MicGateOpen` from `AudioMonitor`.
- **`AudioMonitor`** (`Services/AudioMonitor.cs:17-255`):
  - Opens `WasapiCapture` on selected mic (or `DataFlow.Capture, Role.Communications` default) and `WasapiLoopbackCapture` on selected output (or `DataFlow.Render, Role.Multimedia` default).
  - Accumulates raw peak (`32-bit IEEE float` or `16-bit PCM`, clamped `<= 1.0`).
  - Runs `NoiseGate.Process(pow(rawPeak, 0.6), dt)` on every mic buffer:
    - `NoiseGate` constants (`NoiseGate.cs:29-58`): `HoldSeconds = 0.28f`, `AttackPerSec = 70f`, `ReleasePerSec = 7f`, `AutoMargin = 2.3f`, `AutoFloorFall = 2.5f`, `AutoFloorRise = 0.03f`, `_noiseFloor` init `0.02f` clamped `[0.004f, 0.5f]`, auto `Threshold` clamped `[0.012f, 0.9f]`.
  - When `HearYourself` is on (`SetMonitoring(true, outId)`): creates `WasapiOut(outDev, Shared, useEventSync: false, latency: 100)`, `BufferedWaveProvider` (`500ms`, `ReadFully = true`, `DiscardOnBufferOverflow = true`), `WdlResamplingSampleProvider` if sample rates differ, `MonoToStereoSampleProvider` / `StereoToMonoSampleProvider` if channels differ (`1↔2`).
  - In mic callback (when IEEE float): applies linear per-buffer gain ramp from `_lastMonGain` to `gain` if either `< 0.999f`, then runs `MicDenoiser` (`abuffer → arnndn (model=rnnoise/std.rnnn) → aformat → abuffersink`) if `NoiseSuppression` is enabled, and pushes bytes to `_monitorBuffer`.
- **Manual Gate Bar Interaction** (`SettingsView.axaml.cs:272-284`):
  - Left button press or drag on manual `gatebar` computes `Math.Round(Math.Clamp(x / width * 100, 0, 100))`, updates `InputSensitivityThreshold`, and captures pointer.
- **Per-App Scanner** (`SettingsViewModel.cs:806-851`, `AudioMonitor.cs` / `AudioSessionService.cs`):
  - Immediate pass on visibility + every `4s` on `DispatcherTimer`.
  - Background `Task.Run`: `AudioSessionService.GetActiveAudioApps()` enumerates `AudioSessionStateActive` sessions on default `Render/Multimedia` endpoint (excluding PID `0` and self PID), resolves `FileDescription` + associated icon PNG via `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW` + `ExtractAssociatedIcon`, then removes any exe returned by `_audioGameDetector.RunningGameExes(monitorId)`.
  - `MergeAudioApps` on UI thread: adds newly active apps, updates `DisplayName`/`Icon` on existing entries, removes inactive apps ONLY if `!item.IsEnabled`.

### 4.5 Interactive Overlay Preview (`SettingsView.axaml.cs:90-255`)

- Stage logical size: `736 × 414`.
- Aspect ratios: `Webcam = 4/3`, `Keys = 1.84`, `Stats = 7.5`.
- Box sizing: `h = scale * 414`, `w = h * aspect`; `left = xf * max(0, 736 - w)`, `top = yf * max(0, 414 - h)`.
- Hit testing (`HitZone`): if `min(w, h) < 45`, no resize edges (drag-move only; wheel to resize). Otherwise edge zone `z = min(10, min(w, h) / 5.0)`. Cursors: `TopLeftCorner`, `TopRightCorner`, `SizeWestEast`, `SizeNorthSouth`, `SizeAll`.
- Drag/resize: locks aspect ratio (`w / aspect` for horizontal-only, `h` for vertical-only, `max(w / aspect, h)` for corner), clamps `h` to `[minScale * 414, maxScale * 414]` and stage boundaries anchoring the opposite edge. Scale bounds:
  - Webcam: `[0.06, 0.45]`
  - Keys: `[0.06, 0.42]`
  - Stats: `[0.02, 0.10]`
- Mouse wheel: multiplies height by `1.08` (wheel up) or `0.92` (wheel down), clamps to `[minScale * 414, maxScale * 414]` and remaining space to right/bottom of current `(left, top)`, and commits immediately.
- `CommitBox` converts back to `(xf, yf, scale)` and calls `Vm.CommitOverlayLayout()`, which triggers `SaveSettingsRestartRequired()` and `StatsHudChanged?.Invoke()`.

### 4.6 Hotkey Capture & Suppression (`SettingsViewModel.cs:157-278, 1565-1700`, `GlobalHotkeyManager.cs:108-176`)

- `AreHotkeysSuppressed` is `true` while any of the 5 capture flags (`IsCapturingHotkey`, `IsCapturingPttKey`, `IsCapturingScreenshotKey`, `IsCapturingPauseKey`, `IsCapturingRecordKey`) is true OR `DateTime.UtcNow < _hotkeySuppressedUntil` (`800ms` grace period set in `OnHotkeyCaptured`).
- `GlobalHotkeyManager` runs `TaskPoolGlobalHook` on a background thread. When `IsCapturing == true`:
  - Standalone modifiers (`VcLeft/Right Shift/Control/Alt/Meta`) are ignored.
  - Any other key sets `IsCapturing = false` and raises `HotkeyCaptured(key, modifiers, cleared: key == VcEscape)`.
- `SettingsViewModel.OnHotkeyCaptured`:
  - If `e.Cleared` (`Escape`): sets `key = KeyCode.VcUndefined`, `mods = default`, display `"—"` (`PttKey` also `"—"`). Calls `Clear*Hotkey()` on `GlobalHotkeyService` (for the 4 global hotkeys).
  - Checks capture mode flags in order: `_screenshotCaptureMode` → `_pauseCaptureMode` → `_recordCaptureMode` → `_pttCaptureMode` (single key only, ignores modifiers) → default Save Replay hotkey.
  - Note: `FormatHotkey` displays `"Ctrl"`, `"Alt"`, `"Shift"` (does NOT include Win/Meta in display string, even though `GlobalHotkeyService.ModifiersToWin32` supports `MOD_WIN`), joined with `" + "` and `key.ToString().Replace("Vc", "")`.

---

## 5. Persistence & Formats

### 5.1 `settings.json`

- **Path**: `%AppData%\Lag\settings.json` (`Environment.SpecialFolder.ApplicationData` + `"Lag\settings.json"`, `SettingsViewModel.cs:21-23`).
- **Format**: UTF-8 JSON indented (`JsonSerializerOptions { WriteIndented = true }`, `SettingsViewModel.cs:1868`), exact C# PascalCase property names of `SettingsData` (`SettingsViewModel.cs:2071-2207`).
- **Write debounce**: `350ms` (`_saveDebounce`), flushed immediately on app shutdown (`FlushPendingSave()`).

#### Full `settings.json` Schema (all keys in declaration order)

| JSON Key | Type | Default (`SettingsData`) | Load Clamping / Fallback / Migration | Restart Flag? |
|---|---|---|---|---|
| `BufferSeconds` | `int` | `0` (VM init `300`) | If `> 0`, used directly; else falls back to `BufferMinutes * 60` (`SettingsViewModel.cs:1888-1892`). Matched against `[30, 60, 120, 300, 600, 900]`. | Yes |
| `BufferMinutes` | `int` | `5` | **Legacy read-only migration field** (written by `WriteSettingsToDisk` as default `5`, never set from VM). | — |
| `MonitorIndex` | `int` | `0` | **Dead/legacy field** (always serialized as `0`; never read in `LoadSettings`). | — |
| `HotkeyKey` | `string` | `"VcF10"` | Parsed via `Enum.TryParse<KeyCode>` (`"VcUndefined"` = unbound). | No |
| `HotkeyModifiers` | `string` | `"LeftAlt"` | Parsed via `Enum.TryParse<ModifierMask>`. | No |
| `ScreenshotKey` | `string` | `"VcF9"` | Parsed via `Enum.TryParse<KeyCode>`. | No |
| `ScreenshotModifiers` | `string` | `"LeftAlt"` | Parsed via `Enum.TryParse<ModifierMask>`. | No |
| `PauseKey` | `string` | `"VcF8"` | Parsed via `Enum.TryParse<KeyCode>`. | No |
| `PauseModifiers` | `string` | `"LeftAlt"` | Parsed via `Enum.TryParse<ModifierMask>`. | No |
| `RecordKey` | `string` | `"VcF7"` | Parsed via `Enum.TryParse<KeyCode>`. | No |
| `RecordModifiers` | `string` | `"LeftAlt"` | Parsed via `Enum.TryParse<ModifierMask>`. | No |
| `FFmpegPath` | `string` | `"ffmpeg"` | **Dead/legacy field** (always serialized as `"ffmpeg"`; never read in `LoadSettings`). | — |
| `LibraryPath` | `string` | `""` (VM init `Videos\Lag`) | `settings.LibraryPath ?? Videos\Lag` (`SettingsViewModel.cs:1920`; see Section 9 quirk if `""`). | Yes (manual edit); No (`BrowseFolderAsync`) |
| `FrameRate` | `int` | `30` | If `> 0`: clamped `[1, CurrentRefreshCap()]`; matches rung or selects `Custom` (`CustomFps = clamped`). | Yes |
| `MonitorDeviceName` | `string` | `""` | **Legacy fallback**: matched only if `MonitorStableId` is null/empty (`SettingsViewModel.cs:2021-2022`). | Yes |
| `MonitorStableId` | `string` | `""` | Primary match against `MonitorInfo.StableId` (PnP device interface path); no match keeps primary. | Yes |
| `OverlayEnabled` | `bool` | `false` | Direct bool. | Yes |
| `WebcamDeviceId` | `string` | `""` | Stored in `_savedWebcamId`, matched asynchronously when `RefreshWebcamsAsync` finishes; fallback first webcam. | Yes |
| `WebcamShown` | `bool` | `true` | Direct bool (note: VM property defaults to `false` on fresh run without `settings.json`, `SettingsData` default is `true`). | Yes |
| `WebcamX` | `double` | `0.97` | Clamped `[0, 1]`. | Yes |
| `WebcamY` | `double` | `0.95` | Clamped `[0, 1]`. | Yes |
| `WebcamScale` | `double` | `0.24` | Clamped `[0.06, 0.45]`. | Yes |
| `KeysShown` | `bool` | `false` | Direct bool. | Yes |
| `KeysX` | `double` | `0.03` | Clamped `[0, 1]`. | Yes |
| `KeysY` | `double` | `0.95` | Clamped `[0, 1]`. | Yes |
| `KeysScale` | `double` | `0.20` | Clamped `[0.06, 0.42]`. | Yes |
| `StatsShown` | `bool` | `false` | Direct bool. | No (live HUD) |
| `StatsX` | `double` | `0.5` | Clamped `[0, 1]`. | Yes (via `CommitOverlayLayout`) |
| `StatsY` | `double` | `0.02` | Clamped `[0, 1]`. | Yes (via `CommitOverlayLayout`) |
| `StatsScale` | `double` | `0.045` | Clamped `[0.02, 0.10]`. | Yes (via `CommitOverlayLayout`) |
| `StatsDetail` | `int` | `1` | Clamped `[0, 2]`. | No (live HUD) |
| `MicrophoneId` | `string` | `""` | If non-empty, matched against `Microphones` by `Id`; else keeps first. | Yes |
| `StartWithWindows` | `bool` | `false` | Direct bool; applies HKCU `Run` key post-init if `true`. | No |
| `AutoStartRecording` | `bool` | `false` | Direct bool. | No |
| `StartMinimized` | `bool` | `false` | Direct bool. | No |
| `DisableGameMode` | `bool` | `true` | Direct bool; applies HKCU `GameBar` keys post-init if `true`. | No |
| `GameCaptureEnabled` | `bool` | `false` | **Dead/legacy field** (always serialized as `false`; never read in `LoadSettings`). | — |
| `Language` | `string` | `"en"` | Matched against 17 `LanguageOptions` codes; fallback `"en"`. | No |
| `Theme` | `string` | `"dark"` | Matched against `"light"`, `"dark"`, `"system"`; fallback `"dark"`. | No |
| `MicVolume` | `int` | `100` | Clamped `[0, 200]`. | Yes |
| `OutputResolutionHeight` | `int` | `0` | Matched against `ResolutionOptions` `TargetHeight`; fallback `0` (Native). | Yes |
| `CodecName` | `string` | `""` | Matched against `CodecOptions` `EncoderId` (`""`, `"h264"`, `"hevc"`, `"av1"`); fallback `""` (Auto). | Yes |
| `AutoCleanupEnabled` | `bool` | `false` | Direct bool. | No |
| `MaxLibrarySizeGb` | `int` | `50` | Matched against `[10, 25, 50, 100, 200, 500]`; fallback `50`. | No |
| `BitrateKbps` | `int` | `-1` | If `> 0`: match fixed rung, else `Custom` with `CustomBitrateMbps = clamp(kbps / 1000, 3, 100)`; if `<= 0`: `Auto` (`-1`). | Yes |
| `GpuIndex` | `int` | `-1` | Matched against `GpuOptions` `Index`; fallback `-1` (Auto). | Yes |
| `AudioCaptureMode` | `string` | `"all"` | `"apps" → 1`, anything else `→ 0` (`"all"`). | Yes |
| `AudioModeByTarget` | `bool` | `false` | Direct bool. | Yes |
| `SystemAudioDeviceId` | `string` | `""` | If non-empty, matched against `OutputDevices` by `Id`; else keeps `""` (Default). | Yes |
| `AudioApps` | `AppAudioSetting[]` | `[]` | Each `{ Exe, Enabled, Volume }`. Only entries with `IsEnabled \|\| Volume != 100` are written (`SettingsViewModel.cs:1849`). On load: skips empty `Exe` & case-insensitive duplicates; `Volume` clamped `[0, 100]`; display strips `.exe`. | Yes |
| `PttEnabled` | `bool` | `false` | Direct bool. | No |
| `PttKey` | `string` | `"VcV"` | Parsed via `Enum.TryParse<KeyCode>`. | No |
| `MicMono` | `bool` | `false` | Maps to `MicChannelIndex = MicMono ? 1 : 0`. | Yes |
| `SystemAudioEnabled` | `bool` | `true` | Direct bool. | Yes |
| `SystemAudioVolume` | `int` | `100` | Direct int (unclamped on load). | Yes |
| `MicEnabled` | `bool` | `true` | Direct bool. | Yes |
| `GameAudioEnabled` | `bool` | `true` | Direct bool. | Yes |
| `GameAudioVolume` | `int` | `100` | Direct int (unclamped on load). | Yes |
| `InputSensitivityAuto` | `bool` | `true` | Direct bool. | Yes |
| `InputSensitivityThreshold` | `int` | `8` | Clamped `[0, 100]`. | Yes |
| `NoiseSuppression` | `bool` | `false` | Direct bool. | Yes |
| `SeparateAudioTracks` | `bool` | `false` | Direct bool. | Yes |
| `FileFormat` | `string` | `"mp4"` | Validated against `["mp4", "mov", "mkv"]`; fallback `"mp4"`. | Yes |

### 5.2 Windows Registry Keys

- **Autostart**: `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run`, value `"Lag"` (`REG_SZ`) = `"\"<Environment.ProcessPath>\""` when enabled; deleted when disabled (`SettingsViewModel.cs:1084-1085, 1155-1178`).
- **Disable Game Mode**: `HKCU\SOFTWARE\Microsoft\GameBar`, DWORDs `"AllowAutoGameMode"` and `"AutoGameModeEnabled"` set to `0` when `DisableGameMode == true`, `1` when `false` (`SettingsViewModel.cs:1136-1149`).
- **Update detection**: `HKCU` & `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag_is1` → `"InstallLocation"` compared against `AppContext.BaseDirectory` (`AppUpdateService.cs:42, 60-72`).

### 5.3 Logs, Diagnostics Zip & Updater Files

- **Session logs**: `Documents\Lag\Logs\Lag-yyyy-MM-dd_HH-mm-ss.txt` (keeps newest 15) + `crash.log` (`FileLog.cs:29-48, 140-148`).
- **Diagnostics export**: copies all files in `Documents\Lag\Logs` to `%TEMP%\Lag-diag-<guid>`, zips to `Documents\Lag\Lag-diagnostics-yyyy-MM-dd_HH-mm-ss.zip`, deletes temp dir, launches `explorer.exe /select,"<zip>"` (`SettingsViewModel.cs:1450-1472`).
- **Auto-update download**: `%TEMP%\Lag-Setup-<Version>.exe`, executed with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /AUTOSTART` (`AppUpdateService.cs:132-145`).

---

## 6. Native / External Dependencies

| C# / .NET / Avalonia API | Purpose in Settings | Suggested Rust Equivalent |
|---|---|---|
| `SharpDX.DXGI.Factory1` (`GetAdapter1`, `Description1`) | Enumerate physical GPUs (skipping `0x1414`), read `DedicatedVideoMemory` & `SharedSystemMemory` (`HardwareDetector.cs:38-49`, `HardwareProfiler.cs:102-110`) | `windows::Win32::Graphics::Dxgi::CreateDXGIFactory1` → `IDXGIFactory1::EnumAdapters1` → `DXGI_ADAPTER_DESC1` |
| `user32.dll` (`EnumDisplayMonitors`, `GetMonitorInfo`, `EnumDisplaySettings`, `EnumDisplayDevices` with `EDD_GET_DEVICE_INTERFACE_NAME`) | Enumerate monitors, pixel bounds, primary flag, refresh rate, PnP `StableId` (`HardwareDetector.cs:95-182`) | `windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, EnumDisplaySettingsW, EnumDisplayDevicesW}` |
| `NAudio.CoreAudioApi` (`MMDeviceEnumerator`, `IMMNotificationClient`, `WasapiCapture`, `WasapiLoopbackCapture`, `WasapiOut`, `AudioSessionManager`) | Enumerate capture/render endpoints, hot-plug notifications, live mic/output peak meters, "Hear yourself" playback, active per-app audio sessions (`HardwareDetector.cs:274-299`, `AudioDeviceWatcher.cs:14-40`, `AudioMonitor.cs:76-175`, `AudioSessionService.cs:37-48`) | `windows::Win32::Media::Audio::{IMMDeviceEnumerator, IMMNotificationClient, IAudioClient, IAudioCaptureClient, IAudioRenderClient, IAudioSessionManager2}` (or `cpal`/`wasapi` crate) |
| `NAudio.Wave.SampleProviders.WdlResamplingSampleProvider` | Real-time resampling for "Hear yourself" when mic/output sample rates differ (`AudioMonitor.cs:170-171`) | `rubato` or linear/sinc resampler |
| `FFmpeg.AutoGen` (`avcodec_find_encoder_by_name`, `avcodec_open2`, `avfilter` `abuffer`/`arnndn`/`aformat`/`abuffersink`) | MTA encoder capability probe (`EncoderSelector.cs:55-184`); live RNNoise mic denoising with `rnnoise/std.rnnn` (`MicDenoiser.cs:98-188`) | Same bundled FFmpeg 7.1 C API (`libavcodec`, `libavfilter`) via FFI |
| `SharpHook` (`TaskPoolGlobalHook`) | Global hotkey capture mode + PTT key tracking (`GlobalHotkeyManager.cs:71-176`) | Win32 low-level keyboard/mouse hooks (`SetWindowsHookExW` `WH_KEYBOARD_LL` / `WH_MOUSE_LL` on dedicated thread) |
| `user32.dll` (`RegisterHotKey`, `UnregisterHotKey`, `GetMessage`) | System-wide 4 global hotkeys (`Save`, `Screenshot`, `Pause`, `Record`) on dedicated thread (`GlobalHotkeyService.cs:28-363`) | `windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey}` + `GetMessageW` loop |
| `kernel32.dll` (`GlobalMemoryStatusEx`, `OpenProcess`, `QueryFullProcessImageNameW`) | Total physical RAM (`HardwareProfiler.cs:151-152, 205`); hardened process exe path resolution (`AudioSessionService.cs:86-97`) | `windows::Win32::System::SystemInformation::GlobalMemoryStatusEx`, `windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW}` |
| `System.Drawing.Icon.ExtractAssociatedIcon` | Extract app icons as PNG for per-app audio list (`AudioSessionService.cs:100-112`) | `SHGetFileInfoW` / `ExtractIconExW` + `GetDIBits` → PNG |
| `Microsoft.Win32.Registry` | HKCU `Run` autostart, HKCU `GameBar` Game Mode disable, HKCU/HKLM Inno uninstall lookup | `winreg` crate or `windows::Win32::System::Registry` |
| `Windows.Devices.Enumeration` (`DeviceInformation.FindAllAsync`) | Async webcam enumeration (`WebcamCaptureSource.cs:37-49`) | `windows::Devices::Enumeration::DeviceInformation::FindAllAsync` (or MediaFoundation `MFEnumDeviceSources`) |
| `System.Net.Http.HttpClient` + `System.IO.Compression.ZipFile` | GitHub API latest release check + installer download (`AppUpdateService.cs:96-160`); diagnostics `.zip` creation (`SettingsViewModel.cs:1466`) | `reqwest` + `zip` crate |

---

## 7. Cross-Area Interfaces

- **Into `MainViewModel`** (`ViewModels/MainViewModel.cs`):
  - Startup config/hardware log (`LogStartupDiagnostics`, lines 303-327) reads `AppVersion`, `GpuOptions`, `SelectedMonitor`, `SelectedCodec`, `AudioModeByTarget`, `IsAppsMode`, `EffectiveFps`, `SelectedResolution`, `EffectiveBitrateKbps`, `BufferSeconds`, `SelectedGpu`, `LibraryPath`, and calls `HardwareProfiler.Capture` + `PresetResolver.LogRecommendations`.
  - Builds `RecorderOptions` snapshot on `StartRecordingAsync` (lines 472-540).
  - Clears `Settings.HasPendingChanges = false` on `StopRecording` (line 582).
  - Subscribes to `Settings.StatsHudChanged` to open/close/update `StatsHudWindow` live (lines 185-186, 436-455).
  - Checks `Settings.AreHotkeysSuppressed` in `HotkeyPressed` / `PttPressed` / `PttReleased` (lines 246, 265, 271).
  - Reads `Settings.PushToTalkEnabled`, `Settings.AutoStartRecording`, `Settings.SelectedLanguage.Code`, `Settings.SelectedMonitor.DeviceName`, `Settings.LibraryPath`.
  - Triggers `Settings.AutoUpdateOnStartupAsync()` 3s after launch (lines 236-240).
- **Into `App`** (`App.axaml.cs`):
  - Reads `Settings.StartMinimized` at startup (line 230).
  - Checks `Settings.AreHotkeysSuppressed` on all 4 `GlobalHotkeyService` events (lines 261, 275, 286, 297).
  - Calls `Settings.FlushPendingSave()` on `ShutdownRequested` and `TrayIcon_ExitClicked` (lines 306, 402, 421).
  - Settings calls `App.SetLanguage(code)` and `App.SetTheme(mode)` (`SettingsViewModel.cs:1211, 1233`).
- **Into `LibraryViewModel`** (`ViewModels/LibraryViewModel.cs:261-295, 376, 829`):
  - Reads `LibraryPath`, `AutoCleanupEnabled`, `SelectedStorageLimit.Gb`.
- **Into `HardwareDetector` (outside Settings)**:
  - `Program.cs:209` (`GetAvailableMonitors()` for initial render timer Hz) and `MainWindow.axaml.cs:124-126` (`GetWindowRefreshRate`, `GetWindowMonitor` on move/open/state change).
- **Into `VfrCapture` subsystems**:
  - `SettingsViewModel` uses `EncoderSelector.Available` and `WebcamCaptureSource.ListDevicesAsync()` and `GameDetector.RunningGameExes()`.
  - `AudioMonitor` uses `NoiseGate` and `MicDenoiser`.

---

## 8. Self-Tests / Diagnostics

No `LAG_*` environment variable targets the Settings UI, `PresetResolver`, `HardwareProfiler`, or `AudioMonitor` directly (`Program.cs:70-135` lists all `LAG_*` headless probes, which belong to VFR capture/playback/audio-process loopback).

Startup and runtime diagnostics emitted by Settings & profiling:
- Monitor restore log: `[Settings] monitor restore: savedStableId='{id}' savedName='{name}' → matched {monitor} | no match, keeping {SelectedMonitor}` (`SettingsViewModel.cs:2024-2026`).
- Hardware profile + preset calibration report logged at startup via `MainViewModel.LogStartupDiagnostics()` (`HardwareProfiler.cs:51-66`, `PresetResolver.cs:224-238`).
- `AudioMonitor` failure log: `[AudioMonitor] {mic|output|monitor} meter unavailable: {ex.Message}` (`AudioMonitor.cs:253-254`).

---

## 9. Known Bugs / Quirks to Preserve or Fix

1. **Hardcoded Ukrainian source labels in Audio tab** (`SettingsView.axaml:433, 451`): System audio row uses literal `"Звук системи"` and game audio row uses literal `"Звук гри"` instead of localized resource keys regardless of selected language. *(Fix in Rust port by localizing, unless strict visual parity with current C# bug is desired.)*
2. **Hardcoded `"Powered by RNNoise"`** (`SettingsView.axaml:560`) and `SettingsViewModel.Title = "Налаштування"` (`SettingsViewModel.cs:1480`): Untranslated strings (`Title` is unused in UI).
3. **Unused mic gate bar markers in Auto mode** (`SettingsView.axaml:519-523`): `SettingsViewModel` computes `GateThreshold` and `MicGateOpen`, and `SettingsView.axaml:75-83` defines `Border.meterfill.gated` and `Border.metermarker`, yet the Auto sensitivity bar only renders a plain green `Border.meterfill` without the marker or gated-grey state. *(AudioMonitor still gates "Hear yourself" audibly.)*
4. **Conflicting capture flags if user clicks multiple hotkey buttons sequentially** (`SettingsViewModel.cs:178-265, 974-979, 1582-1587`): Clicking a second hotkey capture button before finishing the first leaves the earlier `_*CaptureMode` and `IsCapturing*` flags `true`, so `OnHotkeyCaptured` routes the keypress to whichever flag appears earlier in its `if` chain and never resets the second flag. *(Fix in Rust by cancelling any active capture before starting another.)*
5. **`LibraryPath` empty-string fallback quirk** (`SettingsViewModel.cs:1920, 2092`): `SettingsData.LibraryPath` defaults to `""` (not `null`), so `settings.LibraryPath ?? defaultPath` preserves `""` if `"LibraryPath": ""` is saved in `settings.json` instead of falling back to `Videos\Lag`. *(Fix in Rust with `string.IsNullOrWhiteSpace` fallback while keeping `Videos\Lag` default.)*
6. **`SystemAudioVolume` and `GameAudioVolume` unclamped on load** (`SettingsViewModel.cs:1977, 1987`): Unlike `MicVolume` (`[0, 200]`) and `AudioApps[].Volume` (`[0, 100]`), these two are not clamped on load. *(Safe to clamp `[0, 100]` in Rust.)*
7. **Dragging Custom bitrate slider onto a preset value hides the slider** (`SettingsViewModel.cs:722-723`): Snapping `CustomBitrateMbps` to an integer Mbps value that equals a fixed preset (`3, 5, 7, 10, 15, 20, 25, 30, 50, 70, 100`) switches `SelectedBitrate` to that preset, setting `IsCustomBitrate = false` and collapsing the slider mid-drag. *(Quirk of C# implementation — decide deliberately whether to preserve or keep Custom mode active until user picks a preset.)*
8. **Legacy `SettingsData` fields & dead UI markup**:
   - `BufferMinutes` (legacy migration read), `MonitorIndex`, `FFmpegPath`, `GameCaptureEnabled` are still serialized into `settings.json` with default values (`5`, `0`, `"ffmpeg"`, `false`) (`SettingsViewModel.cs:2077-2127`).
   - `SettingsView.axaml:157-166` has a permanently hidden `IsVisible="False"` legacy 3-tab `Button.rseg` block (`OnTabClick`).
   - `SettingsViewModel.Presets.cs:41` comment & `en.axaml:232` (`Preset_Rescan`) reference a removed `RescanHardwareCommand`.
9. **`WebcamShown` default mismatch**: `SettingsViewModel._webcamShown` defaults to `false` when no `settings.json` exists, whereas `SettingsData.WebcamShown` defaults to `true` (so first save writes `false`, while an old `settings.json` omitting `WebcamShown` loads `true`).
10. **Overlay wheel zoom only clamps against right/bottom from current left/top** (`SettingsView.axaml.cs:248-249`): If a box sits flush against the right or bottom edge, scrolling up will not shrink left/top to let it grow; it stays clamped against `(StageW - left) / aspect` and `StageH - top`.
11. **`TrayMenu` not re-localized on runtime language switch** (`App.axaml.cs:312, 322-340`): `LocalizeTrayMenu()` only runs once at startup; changing language in Settings updates XAML `{DynamicResource}` and rebuilt dropdowns live, but tray menu items stay in the startup language until restart.

---

## 10. Parity Checklist

- [ ] Settings window shows fixed `800px` centered scrollable column with header, pending-changes amber notice (only while recording & restart-requiring setting edited), and 4-tab `SegmentedListBox` (`Video`, `Audio`, `Overlay`, `General`) with sliding pill (`220ms CubicEaseOut`).
- [ ] Switching tabs resets scroll offset to `(0, 0)` and plays `240ms` rise+fade-in (`16px -> 0`).
- [ ] Startup initializes defaults and enumerates monitors/mics/outputs/GPUs/codecs BEFORE loading `%AppData%\Lag\settings.json`, without triggering dirty-save or `HasPendingChanges`.
- [ ] `HardwareProfiler` reads DXGI adapters (excluding `0x1414`), FFmpeg-probed encoders (on MTA thread), registry CPU name + logical core count, `GlobalMemoryStatusEx` RAM, library drive free/total GiB, and primary monitor dimensions/Hz.
- [ ] `PresetResolver` classifies GPU (`Entry`/`Mid`/`High`/`Top`) from VRAM + NVIDIA/AMD model strings and computes `Performance`, `Balanced` (marked Recommended), and `Quality` presets adapted to RAM budget, monitor height/Hz, and available codecs.
- [ ] Clicking a preset card applies resolution, FPS, codec, bitrate, and buffer without triggering the intensive-quality warning on intermediate states; active card highlights when current settings match all 5 values.
- [ ] Low-disk hint appears below preset cards iff `0 < DiskFreeGiB < 40` and `!AutoCleanupEnabled`.
- [ ] Monitor change rebuilds FPS rungs (capped to snapped monitor Hz, min `60`, fallback `360` if `< 24`) and Resolution rungs (capped to monitor native height), preserving or snapping selection.
- [ ] Resolution, FPS, and Bitrate options display tier colors (normal = inherit, caution = `#E5B84C`, extreme = `#E0564C`).
- [ ] Manual settings exceeding the `Quality` preset ceiling trigger the one-time-per-session localized Intensive Quality modal with hardware-specific summary values.
- [ ] `Audio` tab live meters (`33ms` tick, `pow(level, 0.6)` curve, `0.5` attack / `0.16` release smoothing) and per-app audio scanner (`4s` tick + immediate pass) run ONLY when Settings is attached, window is active, and Audio tab is selected.
- [ ] `AudioMonitor` drives `NoiseGate` + optional `MicDenoiser` (`FFmpeg arnndn` with `rnnoise/std.rnnn`) on the `"Hear yourself"` `WasapiOut` preview; `"Hear yourself"` automatically resets to off when switching tabs, navigating away, or deactivating window.
- [ ] Manual input sensitivity bar (`480×12`) renders cut/pass zones, live level fill, draggable threshold handle (`0–100`), and `-100 dB`..`0 dB` readout.
- [ ] Per-app audio list excludes self PID, PID `0`, and currently detected games; keeps unchecked apps only while actively playing audio, and keeps checked apps across restarts/exits; only saves entries where `Enabled || Volume != 100`.
- [ ] Audio device hot-plug (`IMMNotificationClient`) debounces at `300ms`, refreshes mic/output dropdowns preserving ID, and restarts active audio meters.
- [ ] `Overlay` tab reveals the `736×414` interactive stage and per-overlay controls when `OverlayEnabled` is on; drag moves boxes, edge/corner drag (`10px` zone when `min(w,h) >= 45`) and mouse wheel (`×1.08` / `×0.92`) resize aspect-locked within stage and scale bounds; `StatsHudWindow` updates live without restart flag.
- [ ] `Theme` (`light`/`dark`/`system`) cross-fades the window (`280ms` snapshot fade) and updates all palette resources live; `Language` (17 codes with vector flags) updates UI strings and rebuilds localized dropdowns (`Buffer`, `Resolution`, `Codec`, `Theme`, `Bitrate`, `FPS`, `OutputDevices`) without losing selections or setting `HasPendingChanges`.
- [ ] All 4 global hotkeys (`Save`, `Screenshot`, `Pause`, `Record`) + PTT key support live capture via hook, `Escape` unbinds to `"—"` (`VcUndefined`), `AreHotkeysSuppressed` blocks triggers during capture + `800ms` grace, and global hotkeys register via Win32 `RegisterHotKey` (`MOD_NOREPEAT`).
- [ ] `StartWithWindows` (HKCU `Run\Lag`) and `DisableGameMode` (HKCU `GameBar`) update registry on toggle and re-assert at launch when enabled; `StartMinimized` hides window to tray on first open.
- [ ] `BrowseFolder` picks `LibraryPath`; `OpenLogsFolder` opens `Documents\Lag\Logs`; `SaveDiagnosticsZip` exports timestamped zip to `Documents\Lag` and selects it in Explorer; `CheckForUpdates` (and 3s startup check) queries GitHub Releases and launches silent Inno installer if newer (or migrating legacy Velopack).
- [ ] `settings.json` persists all schema keys with `350ms` debounce and flushes synchronously on app shutdown/tray exit; migrates legacy `BufferMinutes` and `MonitorDeviceName`.
