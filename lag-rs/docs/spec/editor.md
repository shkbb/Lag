# Clip Editor (`EditorView` / `EditorViewModel`) — 1:1 Port Spec

## 1. Scope

Primary source files covered by this specification:

| File | Lines | Role |
|---|---:|---|
| `ViewModels/EditorViewModel.cs` | 1706 | Editor state, undo stack, FFprobe/FFmpeg pipelines, LibVLC preview control, export orchestration |
| `Views/EditorView.axaml` | 648 | Editor UI layout, styles, bindings, key bindings, flyouts, timeline & tool tabs |
| `Views/EditorView.axaml.cs` | 820 | Preview reframe/crop/text pointer interaction, 30 fps playhead interpolation, timeline scrubbing/trimming, cut bands |
| `Controls/TrimRangeBar.cs` | 200 | Custom-rendered per-audio-track range/trim bar with faux waveform and magnetic snapping |

Supporting / cross-referenced files consulted for exact styling, rendering, and shell integration:
- `Services/VlcVideoRenderer.cs` (224 lines) — triple-buffered `WriteableBitmap` vmem bridge for LibVLC
- `Controls/SegmentedListBox.cs` (140 lines) — sliding-pill segmented control used for speed presets
- `Themes/Palette.axaml` (287 lines), `Themes/DarkTheme.axaml` (757 lines), `Themes/Components.axaml` (137 lines), `Themes/Tokens.axaml` (32 lines) — colour tokens, fonts, corner radii, shared control styles
- `Core/UiConverters.cs` (94 lines), `Core/Localizer.cs` (26 lines), `Core/DeferredDispose.cs` (21 lines), `Assets/Langs/en.axaml` (265 lines)
- `ViewModels/MainViewModel.cs` (744 lines), `ViewModels/LibraryViewModel.cs` (876 lines), `ViewModels/PlayerViewModel.cs` (593 lines), `Models/ReplayClip.cs` (79 lines), `App.axaml` (351 lines), `App.axaml.cs` (425 lines)

---

## 2. Features

1. **Clip picker sidebar**: Lists all non-image video clips (`MainViewModel.EditableClips`, excluding screenshots) with 16:9 thumbnail, filename (without extension), and `duration · dd.MM` metadata; clicking any card loads it immediately into the editor and starts preview playback (`EditorView.axaml:184-214`, `EditorViewModel.cs:741-795`).
2. **Software-decoded live video preview**: Renders decoded video frames via LibVLC `vmem` into an Avalonia bitmap (`VlcVideoRenderer`) inside a rounded dark stage (`#FF07090C`), auto-starting playback when a clip is opened (`EditorViewModel.cs:774-791`, `EditorView.axaml:219-275`).
3. **Play / Pause & keyboard shortcuts**: Toggle button in the control bar, `Space` keybinding, clicking outside the reframe box or clicking-without-dragging on the video toggles play/pause; `Ctrl+Z` triggers multi-step Undo (`EditorView.axaml:12-15`, `EditorView.axaml.cs:506-514, 606-618`, `EditorViewModel.cs:297-334`).
4. **Thumbnail strip timeline & 30 fps playhead**: Extracts 10 evenly spaced thumbnails via `ffmpeg` and lays them out across a 56 px tall horizontal filmstrip; overlays dimmed left/right trim shades, cyan 10 px trim handles, red cut bands, and a smooth 30 fps interpolated white playhead constrained to `[TrimStartSec, TrimEndSec]` (`EditorViewModel.cs:850-899`, `EditorView.axaml:350-389`, `EditorView.axaml.cs:620-690`).
5. **Video trimming**: Drag start/end handles on the timeline (`±12 px` hit reach), scrub anywhere else on the timeline (clamped to trimmed range), or use control-bar "Set clip start/end here" and "Reset trim" buttons. Start handle magnetically follows the playhead ("magnet" on `TrimStartSec` change). Minimum trim duration is `0.2 s` (`EditorView.axaml.cs:739-818`, `EditorViewModel.cs:517-538, 1147-1167`).
6. **Multi-track audio lanes (`TrimRangeBar`)**: Probes audio stream count via `ffprobe` and builds 0, 1, or 2 `EditorAudioTrack` rows (`Editor_TrackAudio`, or `Editor_TrackSystem` + `Editor_TrackMic`). Each row has a mute checkbox, 0–100% volume slider, and independent audible `[StartSec, EndSec]` handles with faux waveform and `±10 px` magnetic snapping to video trim edges (`EditorViewModel.cs:823-840`, `EditorView.axaml:395-439`, `TrimRangeBar.cs:95-199`).
7. **Linked video + audio trimming**: `Shift+click` on the video timeline row or any audio track row toggles selection (accent border + dim fill). When `IsVideoSelected` is true, dragging the video start/end trim handle also updates `StartSec`/`EndSec` of all selected audio tracks (`EditorView.axaml.cs:131-139, 725-730, 795-808`).
8. **Middle-section cuts ("Cut" / "End here")**: Two-click workflow (`ToggleCutCommand`) marks `[PendingCutStart, playhead]` with a live translucent red band, then commits merged/sorted `EditorCut` intervals (minimum `0.05 s`, merge tolerance `0.01 s`). Live preview skips over cuts during playback; export renders kept segments and concatenates them (`EditorViewModel.cs:1171-1269, 1063-1079, 1336-1379`, `EditorView.axaml.cs:631-635, 673-718`).
9. **Output format (aspect ratio) & interactive Reframe**: Dropdown selects `Native`, `16:9`, `9:16`, `16:10`, `21:9`, `4:3`, `1:1`. Default placement scales the (cropped) source to *cover* the output frame (`CoverPlacement`). Clicking the video selects the reframe box, revealing dim shades outside the output frame, 8 resize handles (stretch/zoom), move drag with `9 px` magnetic snapping to output frame edges, a "Deselect" chip button, and right-click to reset (`EditorViewModel.cs:187-210, 338-374, 585-595`, `EditorView.axaml.cs:249-299, 483-618`).
10. **Source Crop tool**: Independent `ToggleButton` (`CropToolActive`) switches the preview overlay to show the full source and edit normalized source crop `[CropX, CropY, CropW, CropH]` (min 5% per axis), applied *before* format/reframe. Right-click resets crop to `(0, 0, 1, 1)` (`EditorViewModel.cs:230-261`, `EditorView.axaml.cs:224-296, 489-496, 559-565`).
11. **Rotate & flip**: Four transport buttons rotate `90°` left/right (`0/90/180/270` clockwise) and toggle horizontal/vertical flip; preview transforms `PreviewContent` around center (scaled down to fit stage on `90°`/`270°`), and export applies `hflip`/`vflip` then `transpose` (`EditorViewModel.cs:376-388, 1457-1464`, `EditorView.axaml.cs:408-420`).
12. **Playback speed**: Segmented presets (`0.5×`, `1×`, `1.5×`, `2×`) synced with a manual `TextBox` (`SpeedText`, comma or dot decimals allowed), clamped to `[0.1, 10.0]` for export (`setpts=PTS/speed` + chained `atempo` filters) and `[0.25, 4.0]` for LibVLC preview (`EditorViewModel.cs:140-178, 640-644, 1408-1417, 1528-1529, 1586-1587`).
13. **Video filter presets & manual colour sliders**: 8 filter presets (`Normal`, `Grayscale`, `Sepia`, `Warm`, `Cold`, `Vibrant`, `Dramatic`, `Bright`) with auto-generated 128 px preview thumbnails, plus 3 manual sliders (`Brightness`, `Contrast`, `Saturation`, range `0–200%`, neutral `100%`) and a reset button. Previewed via LibVLC `VideoAdjustOption` and exported via ffmpeg filter graph + `eq` (`EditorViewModel.cs:87-112, 391-427, 647-668, 902-948, 1530-1538`).
14. **Text / caption overlays**: Add/select/delete captions on the Text tab (`EditorTextItem`), edit text string, font size (`3–25%` of frame height), pick from 6 preset colour swatches (`#FFFFFF`, `#000000`, `#FFD43B`, `#FF6B6B`, `#4DABF7`, `#51CF66`), and drag captions directly over the output frame on the preview. Exported via `ffmpeg` `drawtext` using `arialbd.ttf` and temporary UTF-8 text files (`EditorViewModel.cs:59-66, 431-473, 1432-1453`, `EditorView.axaml.cs:303-403`).
15. **Export with options & cancellation**: Export flyout configures Resolution (`Native`, `1080p`, `720p`, `480p`), container Format (`mp4`, `mkv`, `mov` — hidden when GIF is checked), and `Export as GIF` (15 fps 2-pass `palettegen`/`paletteuse`, no audio, ignores middle cuts). Video export tries `h264_nvenc` first and falls back to `libx264`, mixes all audible tracks via `amix` (or maps single track) as AAC 192k, reports live `%` from `ffmpeg -progress pipe:1`, supports mid-export cancellation with partial file cleanup, and refreshes `LibraryViewModel` on completion (`EditorViewModel.cs:553-576, 1273-1686`, `EditorView.axaml:129-176`).

---

## 3. UI

### 3.1 Theme & Shared Token Reference

All `{DynamicResource}` and `{StaticResource}` values used in `EditorView.axaml` (from `Themes/Palette.axaml`, `Themes/Tokens.axaml`, `Themes/DarkTheme.axaml`, `Themes/Components.axaml`):

| Token | Dark Value | Light ("Cream") Value | Source |
|---|---|---|---|
| `FontDisplay` / `FontUi` | `Inter` | `Inter` | `Tokens.axaml:13-14` |
| `FontMono` | `Cascadia Code, Consolas, Courier New, monospace` | same | `Tokens.axaml:15` |
| `SurfaceCard` | `#FF14161C` | `#FFF8F4EA` | `Palette.axaml:34, 168` |
| `SurfaceInput` | `#FF1A1D25` | `#FFE2D6BA` | `Palette.axaml:35, 169` |
| `SurfaceHover2` | `#FF22262F` | `#FFDCD3BB` | `Palette.axaml:37, 171` |
| `SurfaceBrush` | `#FF20252D` | `#FFEBE4D2` | `Palette.axaml:65, 199` |
| `SurfaceHoverBrush` | `#FF272D36` | `#FFE2DAC4` | `Palette.axaml:66, 200` |
| `BorderCard` | `#12FFFFFF` | `#182A2415` | `Palette.axaml:41, 175` |
| `BorderStrong` | `#1AFFFFFF` | `#302A2415` | `Palette.axaml:43, 177` |
| `AccentBrush` | `#FF22D3EE` | `#FFA9722E` | `Palette.axaml:71, 205` |
| `AccentHoverBrush` | `#FF4ADDF2` | `#FF946228` | `Palette.axaml:72, 206` |
| `AccentDim` | `#1F22D3EE` | `#29A9722E` | `Palette.axaml:26, 160` |
| `AccentRing` | `#5222D3EE` | `#5CA9722E` | `Palette.axaml:27, 161` |
| `AccentText` | `#FF7BE7F5` | `#FF7C5320` | `Palette.axaml:29, 163` |
| `OnAccentBrush` | `#FF0A0C0F` | `#FFFBF4E8` | `Palette.axaml:73, 207` |
| `TextHi` | `#FFECEDEF` | `#FF2A2820` | `Palette.axaml:46, 180` |
| `TextMid` | `#FF9AA0AA` | `#FF6E6A5A` | `Palette.axaml:50, 184` |
| `TextMuted` | `#FF8B919C` | `#FF807B69` | `Palette.axaml:51, 185` |
| `TextFaint` | `#FF7C828D` | `#FF928D78` | `Palette.axaml:52, 186` |
| `TextFaintest` | `#FF6A707B` | `#FFA39D86` | `Palette.axaml:53, 187` |
| `FgBrush` | `#FFE7EAEE` | `#FF2A2820` | `Palette.axaml:76, 210` |
| `Fg2Brush` | `#FF9AA3AD` | `#FF6E6A5A` | `Palette.axaml:77, 211` |
| `Fg3Brush` | `#FF6A7380` | `#FFA39D86` | `Palette.axaml:78, 212` |
| `Divider6Brush` | `#0FFFFFFF` | `#142A2415` | `Palette.axaml:80, 214` |
| `Overlay85Brush` | `#D90A0C0F` | `#D90A0C0F` | `Palette.axaml:82, 216` |
| `SegSelBrush` | `#FF2A313B` | `#FFF8F4EA` | `Palette.axaml:69, 203` |
| `SegHoverBrush` | `#FF20262E` | `#FFE2DAC4` | `Palette.axaml:99, 232` |
| `TransportHoverBrush` | `#1AFFFFFF` | `#1A2A2415` | `Palette.axaml:102, 235` |
| `RadGeneral` / `RadInput` / `RadSeg` | `13` / `9` / `7` | `13` / `9` / `7` | `Tokens.axaml:19, 22, 23` |

### 3.2 Root & Keyboard Shortcuts

- Root control: `UserControl` (`Focusable="True"`), `Grid` with `RowDefinitions="Auto,*"`, `Margin="24,20,24,24"` (`EditorView.axaml:9, 104`).
- Key bindings (`EditorView.axaml:12-15`):
  - `Space` -> `PlayPauseCommand`
  - `Ctrl+Z` -> `UndoCommand`
- Note: Transport/action buttons in the video control bar and tools panel use `Focusable="False"` or `Classes="transport"` (`Focusable="False"` in `DarkTheme.axaml:687`) so `Space` isn't captured by a focused button (`EditorView.axaml:282, 301, 308, 315, 325, 497-508`). On view attach, `EditorView` calls `Focus()` (`EditorView.axaml.cs:89`), and after preview clicks calls `Focus()` (`EditorView.axaml.cs:511, 616`).
- Global tunnel pointer handler on `EditorView` (`RoutingStrategies.Tunnel, handledEventsToo: true`): if `!CropMode && _reframeSelected` and `HitTest(e.GetPosition(CropOverlay)) == CropDrag.None`, clears `_reframeSelected = false` and calls `UpdateReframeOverlay()` (`EditorView.axaml.cs:47-59`).

### 3.3 Top Header (`Grid.Row="0"`)

- Layout: `Grid ColumnDefinitions="Auto,*,Auto,Auto"`, `Margin="0,0,0,14"` (`EditorView.axaml:107`).
- Column 0 (`StackPanel Spacing="2"`):
  - Title: `Text="{DynamicResource Editor_Title}"`, `FontFamily="Inter"`, `FontSize="22"`, `FontWeight="SemiBold"`, `Foreground="{DynamicResource TextHi}"` (`EditorView.axaml:109-110`).
  - Clip name: `Text="{Binding CurrentClip.DisplayName, FallbackValue=''}"`, `FontFamily="{StaticResource FontMono}"`, `FontSize="11"`, `Foreground="{DynamicResource TextFaint}"`, `TextTrimming="CharacterEllipsis"`, `MaxWidth="420"` (`EditorView.axaml:111-113`).
- Column 2 (Export status): `TextBlock` bound to `ExportStatus`, `FontSize="12"`, `Foreground="{DynamicResource TextMuted}"`, `VerticalAlignment="Center"`, `Margin="0,0,12,0"`, `MaxWidth="340"`, `TextTrimming="CharacterEllipsis"`, `ToolTip.Tip="{Binding ExportStatus}"` (`EditorView.axaml:116-120`).
- Column 3 (`StackPanel Orientation="Horizontal" Spacing="8" VerticalAlignment="Center"`):
  - **Back button** (`Classes="rchip"`): `SurfaceInput` bg, `TextHi` fg, `BorderStrong` 1 px, `CornerRadius="9"`, `Padding="15,8"`, `FontSize="12.5"`, `FontWeight="SemiBold"`, `Cursor="Hand"`; hover bg `SurfaceHover2` (140 ms `BrushTransition`); press scale `0.97` (80 ms) (`Components.axaml:109-125`, `DarkTheme.axaml:36-47`). Binds `Command="{Binding $parent[Window].DataContext.NavigateToPlayerCommand}"`, `Content="{DynamicResource Editor_Back}"` (`EditorView.axaml:123-126`).
  - **Export button** (`Classes="cta"`, `MinWidth="140"`, `IsVisible="{Binding !IsExporting}"`): `AccentBrush` bg, `OnAccentBrush` fg, `Height="36"`, `CornerRadius="8"`, `FontSize="13"`, `FontWeight="Medium"`, hover bg `AccentHoverBrush` (150 ms) (`DarkTheme.axaml:50-70`). Inside: `StackPanel Orientation="Horizontal" Spacing="8"`: `MaterialIcon Kind="MovieCheckOutline"` (`15×15`), `TextBlock` `{DynamicResource Editor_Export}`, `MaterialIcon Kind="ChevronDown"` (`14×14`) (`EditorView.axaml:129-134`).
    - **Export Flyout** (`Placement="BottomEdgeAlignedRight"`, styled by `FlyoutPresenter`: `SurfaceCard` bg, `BorderStrong` 1 px, `CornerRadius="13"`, `Padding="8"`, `Components.axaml:129-135`):
      - `StackPanel Width="230" Spacing="14" Margin="4"` (`EditorView.axaml:136-164`):
      - Heading: `{DynamicResource Editor_ExportOptions}`, `FontWeight="Medium"`, `FontSize="13"`.
      - Resolution block (`Spacing="6"`): label `{DynamicResource Editor_ExportResolution}` (`FontSize="12"`, `Fg2Brush`); `ComboBox HorizontalAlignment="Stretch"` bound to `ExportResolutionOptions` / `SelectedExportResolution`, displaying `Display` (`Native`, `1080p`, `720p`, `480p`).
      - Format block (`Spacing="6"`, `IsVisible="{Binding !ExportAsGif}"`): label `{DynamicResource Editor_Format}` (`FontSize="12"`, `Fg2Brush`); `ComboBox HorizontalAlignment="Stretch"` bound to `ExportFormatOptions` / `SelectedExportFormat` (`mp4`, `mkv`, `mov`).
      - GIF toggle: `CheckBox IsChecked="{Binding ExportAsGif}" Content="{DynamicResource Editor_ExportGif}"`.
      - Confirm Export button: `Button Classes="cta" HorizontalAlignment="Stretch" Command="{Binding ExportCommand}" Content="{DynamicResource Editor_Export}"`.
  - **Cancel Export button** (`Classes="cta"`, `MinWidth="140"`, `IsVisible="{Binding IsExporting}"`, `Command="{Binding CancelExportCommand}"`): `MaterialIcon Kind="Close"` (`14×14`), `TextBlock Text="{Binding ExportProgress, StringFormat='{}{0}%'}"`, `TextBlock Text="{DynamicResource Editor_CancelExport}"` (`EditorView.axaml:169-176`).

### 3.4 Main Workspace (`Grid.Row="1"`, `ColumnDefinitions="240,*,286"`)

#### 3.4.1 Left Column — Clip Picker Sidebar (`240 px`, `Margin="0,0,14,0"`)
- Outer card: `Border` `Background="{DynamicResource SurfaceCard}"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="1"`, `CornerRadius="14"`, `Padding="0,14"` (`EditorView.axaml:184-185`).
- `Grid RowDefinitions="Auto,*"`:
  - Row 0: `Text="{DynamicResource Editor_Clips}"`, `Margin="16,0,16,10"`, `FontFamily="{StaticResource FontMono}"`, `FontSize="11"`, `Foreground="{DynamicResource TextFaintest}"` (`EditorView.axaml:187-188`).
  - Row 1: `ScrollViewer Padding="8,0" VerticalScrollBarVisibility="Auto"` (`AllowAutoHide="True"`, custom 10 px vertical scrollbar with 6 px thumb visible on hover, `DarkTheme.axaml:703-742`) -> `ItemsControl` bound to `$parent[Window].DataContext.EditableClips` (`EditorView.axaml:189-190`).
  - Each item: `Button Classes="clipitem"` (`Background="Transparent"`, `BorderThickness="0"`, `Padding="6"`, `Margin="0,0,0,4"`, `CornerRadius="9"`, `Cursor="Hand"`, hover bg `SurfaceInput` in 120 ms, `HorizontalAlignment="Stretch"`, `HorizontalContentAlignment="Center"`, `Command="{Binding $parent[Window].DataContext.OpenEditorForClipCommand}"`, `CommandParameter="{Binding}"`) (`EditorView.axaml:88-101, 193-196`).
  - Card content (`StackPanel Spacing="6" Width="190" HorizontalAlignment="Center"`):
    - Thumbnail box: `Border Height="107" CornerRadius="8" ClipToBounds="True" Background="#16181E"` -> `Image Source="{Binding Thumbnail}" Stretch="UniformToFill"` (`EditorView.axaml:199-202`).
    - Name: `TextBlock Text="{Binding DisplayName}" FontSize="11.5" FontWeight="SemiBold" Foreground="{DynamicResource TextHi}" TextTrimming="CharacterEllipsis"` (`EditorView.axaml:203-204`).
    - Meta: `TextBlock Text="{Binding ShortMeta}" FontFamily="{StaticResource FontMono}" FontSize="10" Foreground="{DynamicResource TextFaint}"` (`EditorView.axaml:205-206`).

#### 3.4.2 Center Column — Preview + Timeline (`*`, `Margin="0,0,14,0"`)

Outer layout: `Grid RowDefinitions="*,Auto"`.

**A. Video Preview Card (`Grid.Row="0"`)** (`EditorView.axaml:219-333`):
- `Border x:Name="VideoFrame"`: `Background="#FF07090C"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="1"`, `CornerRadius="14"`, `ClipToBounds="True"`, `MinHeight="180"`.
- `Grid RowDefinitions="*,Auto"`:
  - **Stage (`Panel x:Name="VideoSurface"`, `Background="Transparent"`, `Grid.Row="0"`)**:
    - `Grid x:Name="PreviewContent" RenderTransformOrigin="50%,50%"` (transformed as a unit by `ScaleTransform` for FlipH/FlipV + fit scale, followed by `RotateTransform` for `0/90/180/270`, `EditorView.axaml.cs:408-420`):
      - `Canvas x:Name="VideoCanvas" Background="Transparent"`:
        - `Image x:Name="VideoImage" Stretch="Fill" RenderOptions.BitmapInterpolationMode="HighQuality"` (placed via `Canvas.Left/Top`, `Width/Height` >= 1, `EditorView.axaml.cs:257-261`).
      - `Canvas x:Name="CropOverlay" Background="Transparent"` (pointer events `OnCropPointerPressed`, `OnCropPointerMoved`, `OnCropPointerReleased`):
        - 4 dim shades `CropShadeTop`, `CropShadeBottom`, `CropShadeLeft`, `CropShadeRight` (`Background="#99000000"`, `IsHitTestVisible="False"`).
        - `FrameOutline` (`BorderBrush="White"`, `BorderThickness="1"`, `Opacity="0.55"`, `Background="Transparent"`, `IsHitTestVisible="False"`, always visible).
        - `CropRect` (`BorderBrush="{DynamicResource AccentBrush}"`, `BorderThickness="1.5"`, `Background="Transparent"`, `IsHitTestVisible="False"`).
        - 8 handles `HTL`, `HTR`, `HBL`, `HBR`, `HT`, `HB`, `HL`, `HR` (`Classes="chandle"`: `12×12`, `AccentBrush` fill, `White` `1.5 px` border, `CornerRadius="3"`, `IsHitTestVisible="False"`, centered on edge/corner at `(cx - 6, cy - 6)`).
        - Chrome visibility (`CropRect`, 4 `CropShade*`, 8 handles): visible when `CropToolActive || _reframeSelected`.
    - `Canvas x:Name="TextOverlay" Background="{x:Null}"` (transparent to hits except child caption borders). Each caption is a `Border` (`Background="Transparent"`, `BorderThickness="1.5"`, `BorderBrush="#22D3EE"` when selected else `Transparent`, `Padding="4,2"`, `CornerRadius="4"`, `Cursor="SizeAll"`) wrapping a `TextBlock` (`TextWrapping="NoWrap"`, `FontWeight="Bold"`, `FontSize = max(6, FrameRect.Height * clamp(FontPercent, 1, 50) / 100.0)`, `Foreground` parsed from `item.Color` with `White` fallback). Note: `TextOverlay` is outside `PreviewContent` (`EditorView.axaml:258`).
    - Empty state prompt: `TextBlock Text="{DynamicResource Editor_PickClip}"`, `IsHitTestVisible="False"`, `IsVisible="{Binding CurrentClip == null}"`, centered, `FontSize="14"`, `Foreground="{DynamicResource TextFaint}"`.
    - `DeselectButton`: `Button Classes="chip"`, `IsVisible` true only when `!CropToolActive && _reframeSelected`, `HorizontalAlignment="Right"`, `VerticalAlignment="Top"`, `Margin="12"`, containing `MaterialIcon Kind="Close"` (`13×13`) + `{DynamicResource Editor_Deselect}`.
  - **Control Bar (`Grid.Row="1"`, `EditorView.axaml:278-331`)**:
    - `Border Margin="12" Background="#D10A0C0F" CornerRadius="10" Padding="14,10"`.
    - `Grid ColumnDefinitions="Auto,Auto,*"`:
      - Col 0: Play/Pause button (`Width="30"`, `Height="30"`, `CornerRadius="15"`, `Padding="0"`, `Background="White"`, `BorderThickness="0"`, `Cursor="Hand"`, `Focusable="False"`, `Command="{Binding PlayPauseCommand}"`, `ToolTip.Tip="{DynamicResource Tooltip_PlayPause}"`): `MaterialIcon Kind="Play"` (`15×15`, `!IsPlaying`) or `Pause` (`14×14`, `IsPlaying`), `Foreground="{DynamicResource OnAccentBrush}"`.
      - Col 1: `TextBlock Text="{Binding TimeDisplay}" Margin="12,0,0,0" FontSize="11" Foreground="{DynamicResource Fg2Brush}" VerticalAlignment="Center"`.
      - Col 2 (`StackPanel Orientation="Horizontal" Spacing="8" HorizontalAlignment="Right" VerticalAlignment="Center"`):
        - Set Start (`Classes="chip"`, `Focusable="False"`, `Command="{Binding SetTrimStartHereCommand}"`, `ToolTip.Tip="{DynamicResource Editor_SetStart}"`): `ArrowCollapseLeft` (`14×14`) + `TrimStartDisplay` (`FontSize="12"`).
        - Set End (`Classes="chip"`, `Focusable="False"`, `Command="{Binding SetTrimEndHereCommand}"`, `ToolTip.Tip="{DynamicResource Editor_SetEnd}"`): `TrimEndDisplay` (`FontSize="12"`) + `ArrowCollapseRight` (`14×14`).
        - Cut (`Classes="chip"`, `Focusable="False"`, `Command="{Binding ToggleCutCommand}"`, `ToolTip.Tip="{DynamicResource Editor_Cut}"`): `ContentCut` (`14×14`) + `{DynamicResource Editor_Cut}` (`!IsMarkingCut`) or `{DynamicResource Editor_CutEnd}` (`Foreground="{DynamicResource AccentBrush}"`, `IsMarkingCut`).
        - Reset Trim (`Classes="transport"`, `Command="{Binding ResetTrimCommand}"`, `ToolTip.Tip="{DynamicResource Editor_ResetTrim}"`): `Restore` (`16×16`).

**B. Timeline Card (`Grid.Row="1"`, `EditorView.axaml:336-441`)**:
- `Border Background="{DynamicResource SurfaceCard}"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="1"`, `CornerRadius="14"`, `Padding="10,8,10,10"`, `Margin="0,14,0,0"`.
- `StackPanel Spacing="4"`:
  1. **Time readout header** (`Grid ColumnDefinitions="Auto,*,Auto" Margin="4,0,4,2"`):
     - Col 0: `TrimStartDisplay` (`FontSize="11"`, `Fg3Brush`)
     - Col 1: `TrimDurationDisplay` (`FontSize="11"`, `FontWeight="Medium"`, `AccentBrush`, `HorizontalAlignment="Center"`)
     - Col 2: `TrimEndDisplay` (`FontSize="11"`, `Fg3Brush`)
  2. **Video strip row** (`Border Classes="trow" Classes.selected="{Binding IsVideoSelected}"`):
     - `trow` style: `CornerRadius="8"`, `BorderThickness="1"`, `BorderBrush="Transparent"`, `Padding="4"`, `Background="Transparent"`, 120 ms `BrushTransition` on `BorderBrush` & `Background`. When `.selected`: `BorderBrush="{DynamicResource AccentBrush}"`, `Background="{DynamicResource AccentDim}"` (`EditorView.axaml:19-35`).
     - Inner `Panel x:Name="TimelinePanel" Height="56" Background="#10151B"`:
       - `ItemsControl ItemsSource="{Binding TimelineThumbs}"` in `UniformGrid Rows="1"`, each item `Image Stretch="UniformToFill" Height="56" ClipToBounds="True"`.
       - `Canvas x:Name="TimelineOverlay" Background="Transparent"`:
         - **Pixel-level drawing / layout** (`UpdateOverlay()` in `EditorView.axaml.cs:647-690`, `AddCutBand()` at `705-718`):
           - Let `w = TimelineOverlay.Bounds.Width`, `h = TimelineOverlay.Bounds.Height` (56 px).
           - `sx = TrimStartSec / DurationSec * w`, `ex = TrimEndSec / DurationSec * w`.
           - Cut bands (inserted at `TimelineOverlay.Children` index `0`, i.e., **behind** shades, handles, and playhead):
             - Pending cut band (`cut == null`): `cs = min(PendingCutStart, playSec)`, `ce = max(PendingCutStart, playSec)` where `playSec = clamp(Position, 0, 1) * DurationSec`; `Background="#66F04848"`, `BorderBrush="#F04848"`, `BorderThickness="1,0,1,0"`.
             - Committed cut band (`cut != null`): `cs = cut.StartSec`, `ce = cut.EndSec`; `Background="#CC2A0E12"`, `BorderBrush="#F04848"`, `BorderThickness="1,0,1,0"`.
             - Left `bx = clamp(cs / DurationSec, 0, 1) * w`, right `be = clamp(ce / DurationSec, 0, 1) * w`, `Width = max(0, be - bx)`, `Height = h`, `Top = 0`.
           - `LeftShade`: `Canvas.Left = 0`, `Canvas.Top = 0`, `Width = max(0, sx)`, `Height = h`, `Background="#B30A0C0F"`.
           - `RightShade`: `Canvas.Left = ex`, `Canvas.Top = 0`, `Width = max(0, w - ex)`, `Height = h`, `Background="#B30A0C0F"`.
           - `Playhead`: `Width = 2`, `Height = h`, `Background="White"`, `Opacity="0.9"`, `Canvas.Top = 0`. When paused or scrubbing: `px = clamp(clamp(Position, 0, 1) * w, sx, ex)`, `Canvas.Left = clamp(px - 1, 0, w - 2)`. When playing (not scrubbing): 30 fps timer computes `Canvas.Left` (see Section 4).
           - `StartHandle`: `Width = 10`, `Height = h`, `Background="{DynamicResource AccentBrush}"`, `CornerRadius="3,0,0,3"`, `Canvas.Top = 0`, `Canvas.Left = clamp(sx, 0, w) - (sx <= 5 ? 0 : 5)`. Inner grip notch: `Border` `Width="2"`, `Height="18"`, `CornerRadius="1"`, `Background="{DynamicResource OnAccentBrush}"`, `Opacity="0.65"`, centered.
           - `EndHandle`: `Width = 10`, `Height = h`, `Background="{DynamicResource AccentBrush}"`, `CornerRadius="0,3,3,0"`, `Canvas.Top = 0`, `Canvas.Left = clamp(ex, 0, w) - (ex >= w - 5 ? 10 : 5)`. Inner grip notch: `Border` `Width="2"`, `Height="18"`, `CornerRadius="1"`, `Background="{DynamicResource OnAccentBrush}"`, `Opacity="0.65"`, centered.
  3. **Audio track rows** (`ItemsControl ItemsSource="{Binding AudioTracks}"`, `EditorView.axaml:395-439`):
     - Each row: `Border Classes="trow" Classes.selected="{Binding IsSelected}" PointerPressed="OnTrackRowPressed"` wrapping `Panel Height="32"`:
       - `TrimRangeBar` (`Duration=ClipDurationSec`, `RangeStart=StartSec` TwoWay, `RangeEnd=EndSec` TwoWay, `SnapA=TrimStartSec`, `SnapB=TrimEndSec`, `IsEnabled=!IsMuted`).
       - Left floating chip (`Border Background="{DynamicResource Overlay85Brush}" CornerRadius="6" Padding="8,3" Margin="16,0,0,0" HorizontalAlignment="Left" VerticalAlignment="Center"`): `CheckBox Classes="app" IsChecked="{Binding !IsMuted}"` (`16×16` rounded 4 px box, checkmark `M2 5.5 L4.2 8 L8.5 2.5` stroke `1.8`, `Spacing="10"`, `DarkTheme.axaml:506-543`) + `TextBlock Text="{Binding Name}" FontSize="11" TextTrimming="CharacterEllipsis" MaxWidth="130"`.
       - Right floating chip (`Border Background="{DynamicResource Overlay85Brush}" CornerRadius="6" Padding="8,2" Margin="0,0,16,0" HorizontalAlignment="Right" VerticalAlignment="Center"`): `StackPanel Orientation="Horizontal" Spacing="8"` -> `Slider Width="72" Minimum="0" Maximum="100" TickFrequency="1" IsSnapToTickEnabled="True" Value="{Binding Volume}" IsEnabled="{Binding !IsMuted}"` + `TextBlock Text="{Binding Volume, StringFormat='{}{0}%'}" FontSize="10" Foreground="{DynamicResource Fg2Brush}" Width="30"`.

**C. `TrimRangeBar` Pixel-Level Drawing (`Controls/TrimRangeBar.cs:78-143`)**:
- `MinHeight = 24` (rendered at `32 px` inside the `Panel`), `ClipToBounds = true`, `Cursor = Hand`.
- Palette (hardcoded RGB/ARGB, theme-invariant):
  - `BodyBrush`: `#FF232C36` (`IsEnabled`) / `BodyMutedBrush`: `#FF1A1F27` (`!IsEnabled`)
  - `WaveBrush`: `#FF415061` (`IsEnabled`) / `WaveMutedBrush`: `#FF2A313B` (`!IsEnabled`)
  - `ShadeBrush`: `#B30A0C0F`
  - `HandleBrush`: `#FF22D3EE` (`IsEnabled`) / `HandleMutedBrush`: `#55FFFFFF` (`!IsEnabled`)
  - `NotchBrush`: `#A60A0C0F`
- Constants: `HandleWidth = 10`, `MinGapSec = 0.1`.
- `Render(DrawingContext)` (returns early if `w <= 0 || Duration <= 0`):
  1. `sx = clamp(RangeStart / Duration, 0, 1) * w`, `ex = clamp(RangeEnd / Duration, 0, 1) * w`, `mid = h / 2`.
  2. Body: `RoundedRect(0, 0, w, h, radius=4)` filled with `BodyBrush` or `BodyMutedBrush`.
  3. Faux waveform: loop `for (double x = 3; x < w - 3; x += 4)`:
     - `t = x * 0.55`
     - `amp = (sin(t) * 0.5 + sin(t * 0.31 + 1.7) * 0.32 + sin(t * 0.071 + 0.4) * 0.18) * 0.5 + 0.5`
     - `barH = max(2, amp * (h - 10))`
     - Draw `Rect(x, mid - barH / 2, 2, barH)` with `WaveBrush` or `WaveMutedBrush`.
  4. Shades outside audible range:
     - If `sx > 0`: `Rect(0, 0, sx, h)` filled with `ShadeBrush`.
     - If `ex < w`: `Rect(ex, 0, w - ex, h)` filled with `ShadeBrush`.
  5. Trim handles (`hsx = clamp(sx, 0, w) - (sx <= 5 ? 0 : 5)`, `hex = clamp(ex, 0, w) - (ex >= w - 5 ? 10 : 5)`):
     - Start handle: `RoundedRect(hsx, 0, 10, h)` with top-left `(3,3)` and bottom-left `(3,3)` radii, top-right/bottom-right `(0,0)`, filled with `HandleBrush` or `HandleMutedBrush`.
     - End handle: `RoundedRect(hex, 0, 10, h)` with top-right `(3,3)` and bottom-right `(3,3)` radii, top-left/bottom-left `(0,0)`, filled with `HandleBrush` or `HandleMutedBrush`.
  6. Handle notches (`notchH = max(8, h * 0.45)`):
     - Start notch: `RoundedRect(hsx + 4, mid - notchH / 2, 2, notchH, radius=1)` filled with `NotchBrush`.
     - End notch: `RoundedRect(hex + 4, mid - notchH / 2, 2, notchH, radius=1)` filled with `NotchBrush`.

#### 3.4.3 Right Column — Tools Panel (`286 px`) (`EditorView.axaml:445-644`)
- Outer card: `Border` `Background="{DynamicResource SurfaceCard}"`, `BorderBrush="{DynamicResource BorderCard}"`, `BorderThickness="1"`, `CornerRadius="14"`, `Padding="16,14"`.
- `Grid RowDefinitions="Auto,*"`:
  - **Row 0 — 3-tab segmented switcher** (`Border Classes="rsegtrack" Margin="0,0,0,16"`): `SurfaceInput` bg, `CornerRadius="9"`, `Padding="3"` (`Components.axaml:69-73`). Contains `UniformGrid Columns="3"` with 3 `Button.rseg` (`Tag="0"`, `"1"`, `"2"`, `Click="OnToolsTabClick"`, labels `Editor_TabClip`, `Editor_TabFilters`, `Editor_TabText`).
    - `rseg` styling: `Padding="12,6"`, `FontSize="12.5"`, `FontWeight="Medium"`, `CornerRadius="7"`, `TextMid` fg, hover fg `TextHi`, `.sel` (`SelectedToolsTab == index` via `Eq` converter) sets `Foreground="AccentText"`, `BorderBrush="AccentRing"`, `Background="AccentDim"` (`Components.axaml:74-97`).
  - **Row 1 — Tab content** (`Carousel SelectedIndex="{Binding SelectedToolsTab}"` with `CrossFade Duration="0:0:0.15"`):
    - **Tab 0 (`Clip`)** (`ScrollViewer` -> `StackPanel Spacing="14"`):
      - Format: `Editor_Format` (`FontSize="12"`, `Fg2Brush`) + `ComboBox HorizontalAlignment="Stretch"` (`AspectOptions` / `SelectedAspect`, `TextBlock Text="{Binding Display}"`).
      - Crop toggle: `ToggleButton IsChecked="{Binding CropToolActive}" HorizontalAlignment="Stretch" HorizontalContentAlignment="Center" Cursor="Hand" Padding="12,8"` containing `MaterialIcon Kind="Crop"` (`15×15`) + `{DynamicResource Editor_Crop}`.
      - Rotate / flip row (`StackPanel Orientation="Horizontal" Spacing="8"`): 4 `Button.transport` buttons (`RotateLeftCommand` -> `RotateLeft` `16×16`, `RotateRightCommand` -> `RotateRight` `16×16`, `ToggleFlipHCommand` -> `FlipHorizontal` `16×16`, `ToggleFlipVCommand` -> `FlipVertical` `16×16`).
      - Speed: `Editor_Speed` (`FontSize="12"`, `Fg2Brush`) + `StackPanel Orientation="Horizontal" Spacing="8"`: `SegmentedListBox Classes="segctl"` (`SpeedOptions` / `SelectedSpeed`, 220 ms `CubicEaseOut` sliding `SegSelBrush` pill, `SegmentedListBox.cs:119-139`) + `TextBox Width="52" Text="{Binding SpeedText}"`.
    - **Tab 1 (`Filters`)** (`ScrollViewer` -> `StackPanel Spacing="14"`):
      - `ListBox Classes="filters"` (`WrapPanel Orientation="Horizontal"`, `FilterOptions` / `SelectedFilter`): each item `StackPanel Spacing="3" Width="78"`, preview `Border Width="74" Height="42" CornerRadius="8" Background="#10151B" ClipToBounds="True"` with `RectangleGeometry Rect="0,0,74,42" RadiusX="8" RadiusY="8"` clip + `Image Source="{Binding Preview}" Stretch="UniformToFill"`, name `TextBlock Text="{Binding Name}" FontSize="10" HorizontalAlignment="Center"`. Hover bg `#20262E`, selected bg `AccentBrush` + `OnAccentBrush` medium text.
      - Divider: `Border Height="1" Background="{DynamicResource Divider6Brush}"`.
      - Manual Color (`StackPanel Spacing="10"`):
        - Header `Grid ColumnDefinitions="*,Auto"`: `{DynamicResource Editor_Color}` (`FontSize="12"`, `Fg2Brush`) + `Button.transport` `ResetColorCommand` (`MaterialIcon Kind="Restore"` `14×14`).
        - 3 sliders (`Editor_Brightness`, `Editor_Contrast`, `Editor_Saturation`): label `FontSize="11" Fg3Brush`, readout `{0}%` `FontSize="11" Fg3Brush`, `Slider Minimum="0" Maximum="200"` (`Height="20"`, 4 px track, 12 px white thumb scaling to 1.25× on hover, `DarkTheme.axaml:553-589`) with `PointerPressed="OnColorSliderPressed"`.
    - **Tab 2 (`Text`)** (`ScrollViewer` -> `StackPanel Spacing="14"`):
      - Add Text CTA: `Button.cta HorizontalAlignment="Stretch" Command="{Binding AddTextCommand}"` (`MaterialIcon Kind="Plus"` `15×15` + `{DynamicResource Editor_AddText}`).
      - Selected text editor (`IsVisible="{Binding SelectedText != null}"`, `Spacing="10"`):
        - `TextBox Text="{Binding SelectedText.Text}" AcceptsReturn="False"`
        - Size (`Spacing="2"`): `{DynamicResource Editor_TextSize}` (`FontSize="11" Fg3Brush`) + `Slider Minimum="3" Maximum="25" Value="{Binding SelectedText.FontPercent}"`
        - Colour chips (`ItemsControl ItemsSource="{Binding TextColors}"`, `WrapPanel Orientation="Horizontal"`): `Button Width="26" Height="26" Margin="0,0,6,6" CornerRadius="13" Padding="0" Cursor="Hand" Background="{Binding Brush}" BorderBrush="#50FFFFFF" BorderThickness="1" Command="{Binding $parent[ItemsControl].DataContext.SetTextColorCommand}" CommandParameter="{Binding Hex}"`.
        - Delete button: `Button.chip HorizontalAlignment="Left" Command="{Binding RemoveSelectedTextCommand}"` (`DeleteOutline` `14×14` + `{DynamicResource Editor_DeleteText}`).
      - Empty hint (`IsVisible="{Binding !HasTexts}"`): `TextBlock Text="{DynamicResource Editor_TextHint}" FontSize="11" TextWrapping="Wrap" Foreground="{DynamicResource Fg3Brush}"`.

### 3.5 Localisation Keys Used

From `Assets/Langs/en.axaml` (all keys referenced in `EditorViewModel.cs` and `EditorView.axaml`):

| Key | English value | Usage |
|---|---|---|
| `Editor_Title` | `Editor` | Header title (`EditorView.axaml:109`) |
| `Editor_Back` | `Back` | Header Back button (`EditorView.axaml:126`) |
| `Editor_Clips` | `CLIPS` | Sidebar header (`EditorView.axaml:187`) |
| `Editor_PickClip` | `Choose a clip to edit` | Empty preview prompt (`EditorView.axaml:261`) |
| `Editor_Deselect` | `Deselect` | Reframe deselect button (`EditorView.axaml:272`) |
| `Editor_Export` | `Export` | Export button & flyout CTA (`EditorView.axaml:132, 162`) |
| `Editor_ExportOptions` | `Export options` | Flyout title (`EditorView.axaml:138`) |
| `Editor_ExportResolution` | `Resolution` | Flyout resolution label (`EditorView.axaml:140`) |
| `Editor_Format` | `Format` | Flyout & Clip-tab format label (`EditorView.axaml:153, 471`) |
| `Editor_ExportGif` | `Export as GIF` | Flyout GIF checkbox (`EditorView.axaml:159`) |
| `Editor_CancelExport` | `Cancel` | Cancel button while exporting (`EditorView.axaml:174`) |
| `Editor_Exporting` | `Exporting… {0}%` | `ExportStatus` during export (`EditorViewModel.cs:1282, 1301`) |
| `Editor_ExportDone` | `Saved: {0}` | `ExportStatus` on success (`EditorViewModel.cs:1308`) |
| `Editor_ExportError` | `Export failed: {0}` | `ExportStatus` on error (`EditorViewModel.cs:1313, 1324`) |
| `Editor_TabClip` | `Clip` | Tools tab 0 (`EditorView.axaml:452`) |
| `Editor_TabFilters` | `Filters` | Tools tab 1 (`EditorView.axaml:455`) |
| `Editor_TabText` | `Text` | Tools tab 2 (`EditorView.axaml:458`) |
| `Editor_Crop` | `Crop` | Crop toggle button (`EditorView.axaml:491`) |
| `Editor_Speed` | `Speed` | Speed section label (`EditorView.axaml:512`) |
| `Editor_Color` | `Color` | Manual color section label (`EditorView.axaml:561`) |
| `Editor_Brightness` | `Brightness` | Brightness slider label (`EditorView.axaml:570`) |
| `Editor_Contrast` | `Contrast` | Contrast slider label (`EditorView.axaml:578`) |
| `Editor_Saturation` | `Saturation` | Saturation slider label (`EditorView.axaml:586`) |
| `Editor_AddText` | `Add text` | Add text button (`EditorView.axaml:601`) |
| `Editor_TextSize` | `Size` | Font size slider label (`EditorView.axaml:610`) |
| `Editor_DeleteText` | `Delete` | Delete text button (`EditorView.axaml:633`) |
| `Editor_TextDefault` | `Text` | Initial text of new `EditorTextItem` (`EditorViewModel.cs:454`) |
| `Editor_TextHint` | `Add text, then drag it on the video` | Hint when `Texts.Count == 0` (`EditorView.axaml:638`) |
| `Editor_TrackSystem` | `System sound` | Track label when `audioStreams >= 2` (`EditorViewModel.cs:828, 834`) |
| `Editor_TrackMic` | `Microphone` | Track label when `audioStreams >= 2` (`EditorViewModel.cs:829, 835`) |
| `Editor_TrackAudio` | `Audio` | Default / single-stream track label (`EditorViewModel.cs:772, 839`) |
| `Editor_SetStart` | `Set clip start to the current moment` | Control bar tooltip (`EditorView.axaml:302`) |
| `Editor_SetEnd` | `Set clip end to the current moment` | Control bar tooltip (`EditorView.axaml:309`) |
| `Editor_Cut` | `Cut` | Control bar button & tooltip (`EditorView.axaml:316, 319`) |
| `Editor_CutEnd` | `End here` | Control bar button text while marking cut (`EditorView.axaml:321`) |
| `Editor_ResetTrim` | `Reset trim` | Control bar tooltip (`EditorView.axaml:326`) |
| `Option_Native` | `Native` | Default Aspect & ExportResolution label (`EditorViewModel.cs:587, 599`) |
| `Tooltip_PlayPause` | `Play / Pause` | Control bar play/pause tooltip (`EditorView.axaml:284`) |

*(Unused keys present in `en.axaml`: `Editor_NoClips`, `Editor_Tracks` — not referenced anywhere in Editor code or XAML.)*

---

## 4. State & Logic

### 4.1 Sub-Models (`EditorViewModel.cs:19-112`)

- **`EditorAudioTrack`** (`ObservableObject`):
  - `int StreamIndex` (read-only, zero-based ffmpeg audio stream index `0:a:N`)
  - `string Name` (read-only)
  - `int Volume = 100` (observable, `0..100`)
  - `bool IsMuted = false` (observable)
  - `bool IsSelected = false` (observable; Shift+click selection)
  - `double StartSec = 0`, `double EndSec = ClipDurationSec`, `double ClipDurationSec = max(0.1, clipDurationSec)` (observable)
- **`EditorTextItem`** (`ObservableObject`):
  - `string Text = "Text"`, `double Nx = 0.5`, `double Ny = 0.12`, `int FontPercent = 8`, `string Color = "#FFFFFF"` (all observable)
- **Records**:
  - `EditorSpeedOption(string Display, double Value)`: `("0.5×", 0.5)`, `("1×", 1.0)`, `("1.5×", 1.5)`, `("2×", 2.0)`
  - `EditorExportRes(string Display, int Height)`: `(Localizer("Option_Native"), 0)`, `("1080p", 1080)`, `("720p", 720)`, `("480p", 480)`
  - `EditorTextColor(string Hex, IBrush Brush)`: `#FFFFFF`, `#000000`, `#FFD43B`, `#FF6B6B`, `#4DABF7`, `#51CF66`
  - `EditorCut(double StartSec, double EndSec)`
  - `EditorAspectOption(string Display, double? Ratio, string? CropGeometry)`: `(Option_Native, null, null)`, `("16:9", 16/9, "16:9")`, `("9:16", 9/16, "9:16")`, `("16:10", 16/10, "16:10")`, `("21:9", 21/9, "21:9")`, `("4:3", 4/3, "4:3")`, `("1:1", 1.0, "1:1")`. *Note: `CropGeometry` is unused dead data.*
  - `EditorFilterOption` (`ObservableObject`): `Name`, `Graph`, `Contrast=1f`, `Brightness=1f`, `Saturation=1f`, `Hue=0f`, `HasAdjust` (true if any differs from neutral by `> 0.001`), observable `Bitmap? Preview`. Presets (`EditorViewModel.cs:391-403`):
    - `Normal`: `Graph = null` (1, 1, 1, 0)
    - `Grayscale`: `"hue=s=0"`, `Saturation = 0f`
    - `Sepia`: `"colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131"`, `Saturation = 0.4f, Hue = 30f, Brightness = 1.03f`
    - `Warm`: `"colortemperature=temperature=4500"`, `Hue = -12f, Saturation = 1.1f`
    - `Cold`: `"colortemperature=temperature=8500"`, `Hue = 12f, Saturation = 1.05f`
    - `Vibrant`: `"eq=saturation=1.6:contrast=1.05"`, `Saturation = 1.6f, Contrast = 1.05f`
    - `Dramatic`: `"eq=contrast=1.35:brightness=-0.06:saturation=0.8"`, `Contrast = 1.35f, Brightness = 0.94f, Saturation = 0.8f`
    - `Bright`: `"eq=brightness=0.08:saturation=1.15"`, `Brightness = 1.08f, Saturation = 1.15f`

### 4.2 ViewModel Properties & Derived Values

- `Title = "Editor"`, `ObservableCollection<EditorAudioTrack> AudioTracks`, `ObservableCollection<Bitmap> TimelineThumbs`, `VlcVideoRenderer VideoRenderer`, `ObservableCollection<EditorTextItem> Texts`, `ObservableCollection<EditorCut> Cuts`.
- **Playback / clip**: `ReplayClip? CurrentClip`, `MediaPlayer? Player` (unused in XAML), `bool IsPlaying`, `double Position` (`0..1`), `string TimeDisplay = "0:00 / 0:00"`, `double DurationSec = 1`, `double TrimStartSec = 0`, `double TrimEndSec = 1`, `bool IsVideoSelected`.
  - `TrimStartDisplay` / `TrimEndDisplay` / `TrimDurationDisplay` formatted as `h\:mm\:ss` when `>= 1 hr`, else `m\:ss` (`EditorViewModel.cs:540-542, 1688-1689`).
  - `OnTrimStartSecChanged`: updates trim displays, and if `DurationSec > 0`, computes `pos = clamp(value / DurationSec, 0, 1)`, sets `_pendingSeekPosition = (float)pos`, starts 80 ms VLC apply timer, and sets `Position = pos` (with `_isUpdatingPosition = true` to avoid re-triggering `OnPositionChanged`) (`EditorViewModel.cs:517-533`).
  - `OnPositionChanged`: if `!_isUpdatingPosition`, sets `_pendingSeekPosition = (float)clamp(value, 0, 1)` and calls `StartVlcApplyTimer()` (`EditorViewModel.cs:498-503`).
- **Speed**: `SpeedOptions`, `SelectedSpeed` (default `1×`), `SpeedText` (`"1"`), `EffectiveSpeed` (parses `SpeedText.Trim().Replace(',', '.')` with `InvariantCulture`, clamped `[0.1, 10.0]`, fallback `1.0`). Two-way sync between `SelectedSpeed` and `SpeedText`; `OnSpeedTextChanged` calls `ApplyPreviewRate()` (`EditorViewModel.cs:140-178`).
- **Aspect & Reframe**: `AspectOptions`, `SelectedAspect` (default `Native`), `VidX`, `VidY`, `VidW = 1`, `VidH = 1`, `SourceWidth`, `SourceHeight`.
  - `SourceAspectRaw`: `SourceWidth > 0 && SourceHeight > 0 ? (double)SourceWidth / SourceHeight : 16.0 / 9.0`.
  - `SourceAspect`: `SourceWidth > 0 && SourceHeight > 0 ? (SourceWidth * CropW) / (SourceHeight * CropH) : 16.0 / 9.0`.
  - `FrameAspect`: `SelectedAspect?.Ratio is double r && r > 0 ? r : SourceAspect`.
  - `OnSelectedAspectChanging`: calls `PushUndo()`. `OnSelectedAspectChanged`: notifies `FrameAspect`, and unless `_suppressUndo`, calls `ResetReframe()` (`EditorViewModel.cs:194-200`).
  - `CoverPlacement()`: if `SourceAspect >= FrameAspect`, `w = SourceAspect / FrameAspect`, returns `((1 - w) / 2, 0, w, 1)`; else `h = FrameAspect / SourceAspect`, returns `(0, (1 - h) / 2, 1, h)`.
  - `IsReframed`: true if any of `VidX, VidY, VidW, VidH` differs from `CoverPlacement()` by `> 0.003`.
  - `SetVideoRect(x, y, w, h)`: clamps `w, h` to `[0.05, 6]`, `x, y` to `[-3, 3]`, raises `IsReframed` and `ReframeChanged` (`EditorViewModel.cs:358-367`).
- **Crop tool**: `CropToolActive`, `CropX`, `CropY`, `CropW = 1`, `CropH = 1`. `IsCropped` true if `CropX > 0.002 || CropY > 0.002 || CropW < 0.998 || CropH < 0.998`. `SetCrop(x, y, w, h)` clamps `w, h` to `[0.05, 1]`, `x` to `[0, 1 - w]`, `y` to `[0, 1 - h]`, notifies `IsCropped`, `SourceAspect`, `FrameAspect`, and calls `ResetReframe()` (`EditorViewModel.cs:231-255`).
- **Rotation / Flip**: `int Rotation` (`0, 90, 180, 270`), `bool FlipH`, `bool FlipV`. Each raises `ReframeChanged` on change. `RotateRight` (`(Rotation + 90) % 360`), `RotateLeft` (`(Rotation + 270) % 360`), `ToggleFlipH`, `ToggleFlipV` all call `PushUndo()` first (`EditorViewModel.cs:377-387`).
- **Filters & Colour**: `FilterOptions`, `SelectedFilter` (default `Normal`, `OnSelectedFilterChanging` calls `PushUndo()`, `OnSelectedFilterChanged` calls `ApplyPreviewFilter()`). `ColorBrightness = 100`, `ColorContrast = 100`, `ColorSaturation = 100` (each calls `ApplyPreviewFilter()`). `HasColorAdjust` true if any `!= 100`. `ResetColor` pushes undo and sets all three to `100` (`EditorViewModel.cs:405-427`).
- **Text**: `Texts`, `SelectedText`, `HasTexts`, `TextColors`. `Texts.CollectionChanged` attaches `PropertyChanged` to each item to raise `TextsChanged`, and updates `HasTexts`. `AddText` pushes undo, inserts `EditorTextItem { Text = Localizer.Get("Editor_TextDefault") }`, selects it. `RemoveSelectedText` pushes undo, clears `SelectedText`, removes item. `SetTextColor(hex)` sets `SelectedText.Color = hex` (does NOT push undo) (`EditorViewModel.cs:431-473, 619-629`).
- **UI tab**: `int SelectedToolsTab` (`0 = Clip`, `1 = Filters`, `2 = Text`).
- **Cuts**: `Cuts`, `HasCuts`, `bool IsMarkingCut`, `double? PendingCutStart`.
- **Export**: `bool IsExporting`, `int ExportProgress` (`0..100`), `string ExportStatus`, `SelectedExportResolution` (default `Native`), `ExportFormatOptions` (`"mp4"`, `"mkv"`, `"mov"`), `SelectedExportFormat = "mp4"`, `bool ExportAsGif`.

### 4.3 Undo Stack (`EditorViewModel.cs:265-334`)

- Unbounded `Stack<EditSnap>` storing: `TrimStart`, `TrimEnd`, `Speed`, `Aspect` (index), `Filter` (index), `CropX/Y/W/H`, `VidX/Y/W/H`, `Rotation`, `FlipH`, `FlipV`, `ColorB/C/S`, `EditorCut[] Cuts`, `TextSnap[] Texts`.
- `PushUndo()` is a no-op when `_suppressUndo == true`. Cleared on `OpenClip`.
- `Undo()` pops snapshot, sets `_suppressUndo = true`, restores all fields, rebuilds `Cuts` and `Texts` (clearing `SelectedText = null`), then calls `ApplyPreviewFilter()`, `ReframeChanged`, `CutsChanged`, `TextsChanged`, and notifies `IsCropped`, `IsReframed`, `SourceAspect`, `FrameAspect`, `HasCuts`, `HasTexts`, `UndoCommand`.
- **Not captured in `EditSnap`**: `AudioTracks` (volume, mute, range, selection), `IsVideoSelected`, `PendingCutStart`, `Text` text/font-size/color property edits (only add/remove/drag push undo), `SetTrimStartHere`/`SetTrimEndHere`/`ResetTrim` (unlike timeline handle drag which does push undo!).

### 4.4 Pointer Interactions in View & `TrimRangeBar`

1. **Preview Reframe & Crop (`EditorView.axaml.cs:446-618`)**:
   - `HitTest(p)` against `EditRect(RefRect())` using `HandleReach = 11 px`:
     - Priority order: `NW` -> `NE` -> `SW` -> `SE` -> `W` -> `E` -> `N` -> `S` -> `Move` (strictly inside `bx..bx+bw`, `by..by+bh`) -> `None`.
     - Hover cursor when `_cropDrag == None`: `SizeAll` (`Move`), `SizeWestEast` (`W/E`), `SizeNorthSouth` (`N/S`), `TopLeftCorner` (`NW/SE`), `TopRightCorner` (`NE/SW`), `Cursor.Default` (`None`).
   - Right-click on `CropOverlay`: calls `PushUndo()`, executes `ResetCropCommand` (if `CropMode`) or `ResetReframeCommand` (`EditorView.axaml.cs:489-496`).
   - Left-click on `CropOverlay`:
     - In Reframe mode when `!_reframeSelected`, any non-`None` hit is coerced to `CropDrag.Move` (handles only activate once selected).
     - If `mode == None`: deselects reframe (if selected), toggles `PlayPauseCommand`, focuses `EditorView`, returns.
     - Otherwise: selects reframe (if not already), captures pointer on `CropOverlay`, records `_csX/Y/W/H` and `_cropStart`.
   - Pointer move while dragging:
     - Undo snapshot is pushed once when `|p.X - _cropStart.X| > 3 || |p.Y - _cropStart.Y| > 3` (`_cropMoved = true`).
     - `CropMode`: enforces minimum `0.05` per axis (`EnforceMin`), calls `SetCrop` (clamped to `[0, 1]`).
     - Reframe mode:
       - `snapX = 9 / refr.Width`, `snapY = 9 / refr.Height`.
       - `minX = -refr.X / refr.Width`, `maxX = (sw - refr.X) / refr.Width`, `minY = -refr.Y / refr.Height`, `maxY = (sh - refr.Y) / refr.Height`.
       - `Move`: snaps nearest left/right edge to `0` or `1` if within `snapX`, top/bottom edge to `0` or `1` if within `snapY` (preserving size), then shifts to stay within `[minX, maxX]` and `[minY, maxY]`.
       - Resize: snaps each dragged edge to `0` or `1` if within `snapX`/`snapY`, clamps dragged edges to stage bounds (`minX..maxX`, `minY..maxY`), enforces `0.05` minimum size (`EnforceMin`).
   - Pointer release: releases pointer capture; if `_cropDrag == Move && !_cropMoved`, executes `PlayPauseCommand` and focuses `EditorView`.
2. **Caption Overlays (`EditorView.axaml.cs:365-403`)**:
   - Left-press on caption `Border`: sets `SelectedText = item`, switches `SelectedToolsTab = 2` (Text tab), captures pointer.
   - Move: pushes undo once after `> 3 px` movement; updates `Nx = clamp(_dtNx + dx / max(1, fr.Width - cw), 0, 1)`, `Ny = clamp(_dtNy + dy / max(1, fr.Height - ch), 0, 1)`.
3. **Video Timeline (`EditorView.axaml.cs:720-819`)**:
   - `Shift + click` anywhere on `TimelineOverlay`: toggles `_vm.IsVideoSelected`, returns.
   - Normal left-press: computes `sx`, `ex` in px.
     - If `|x - sx| <= 12 && |x - sx| <= |x - ex|` -> `DragMode.TrimStart` (pushes undo).
     - Else if `|x - ex| <= 12` -> `DragMode.TrimEnd` (pushes undo).
     - Else -> `DragMode.Scrub`.
   - Dragging `TrimStart`/`TrimEnd`:
     - Snaps to any `AudioTracks[i].StartSec` or `EndSec` within `10.0 / w * DurationSec` (`SnapToTrackEdges`, returns first match).
     - Clamps `TrimStartSec` to `[0, max(0, TrimEndSec - 0.2)]`; clamps `TrimEndSec` to `[min(DurationSec, TrimStartSec + 0.2), DurationSec]`.
     - If `_vm.IsVideoSelected`, updates `StartSec`/`EndSec` of all `AudioTracks` where `IsSelected == true`, clamped with `0.1 s` min gap.
   - Dragging `Scrub`: sets `Position = clamp(x / w, TrimStartSec / DurationSec, TrimEndSec / DurationSec)`.
4. **Audio `TrimRangeBar` (`TrimRangeBar.cs:145-199`)**:
   - Disabled if `!IsEnabled || Bounds.Width <= 0 || Duration <= 0`.
   - `Shift + click`: returns without `e.Handled = true`, bubbling to `Border.trow` (`OnTrackRowPressed`), which toggles `track.IsSelected`.
   - Normal press: picks nearer handle (`|x - sx| <= |x - ex| ? Start : End`) regardless of distance, captures pointer, applies immediately.
   - Dragging: snaps to `SnapA` (`TrimStartSec`) or `SnapB` (`TrimEndSec`) within `10.0 / Bounds.Width * Duration` (~10 px); clamps with `MinGapSec = 0.1`.

### 4.5 Threading, Timers & Throttling

- **VLC seek throttling (`EditorViewModel.cs:1005-1047`)**: `_vlcApplyTimer` (`DispatcherTimer`, `80 ms` interval). `StartVlcApplyTimer()` runs `ApplyPendingVlcWrites()` immediately if timer isn't running, then starts timer so rapid scrub/trim drags coalesce seeks to `<= 12.5 Hz`. Stops automatically when `_pendingSeekPosition == null`.
- **VLC playback polling (`EditorViewModel.cs:966-967, 1049-1105`)**: `System.Timers.Timer(250)` (background threadpool thread) reads `_mediaPlayer.Position`, `_mediaPlayer.Time`, `_mediaPlayer.Length`, and posts to `Dispatcher.UIThread` to:
  1. Skip over any `EditorCut` when `IsPlaying && posSec >= c.StartSec - 0.03 && posSec < c.EndSec - 0.03` (seeks to `min(c.EndSec, TrimEndSec)`).
  2. Auto-pause and park at `TrimEndSec` when `IsPlaying && posSec >= TrimEndSec`.
  3. Otherwise update `Position` (with `_isUpdatingPosition = true`) and `TimeDisplay`.
- **VLC Playing re-assertion (`EditorViewModel.cs:969-986`)**: On `_mediaPlayer.Playing`, posts to UI thread to start `_positionTimer`, immediately calls `ApplyPreviewFilter()` & `ApplyPreviewVolume()`, then schedules a `DispatcherTimer.RunOnce(..., 300 ms)` to re-apply `ApplyPreviewFilter()` & `ApplyPreviewRate()` (avoids dropped first GOP / uninitialized vmem vout filter).
- **UI 30 fps Playhead Interpolator (`EditorView.axaml.cs:26-28, 81-84, 621-642`)**: `DispatcherTimer` at `33 ms` running while view is attached. When `_vm.IsPlaying && _drag != DragMode.Scrub`, predicts `predicted = _lastVmPosition + elapsed * _vm.EffectiveSpeed / _vm.DurationSec`, jumps `predicted` past any cut `[StartSec, EndSec)`, clamps to `[TrimStartSec/DurationSec, TrimEndSec/DurationSec]`, and sets `Canvas.SetLeft(Playhead, clamp(px - 1, 0, w - 2))`.
- **Probing & Thumbnails**: `ProbeAsync` (`Task` using `FFMpegCore.FFProbe.AnalyseAsync`) and `GenerateTimelineThumbsAsync` (`_thumbsCts` cancels prior clip's jobs) run sequentially off the UI thread and marshal bitmap/property updates via `Dispatcher.UIThread.InvokeAsync`.
- **Audio track preview (`EditorViewModel.cs:683-729`)**: Since LibVLC plays only one audio stream at a time:
  - Filters `unmuted = AudioTracks.Where(t => !t.IsMuted && t.Volume > 0)`.
  - If `unmuted.Count == 0` -> `_mediaPlayer.Mute = true`.
  - Else `_mediaPlayer.Mute = false`; if `unmuted.Count == 1`, selects `unmuted[0].StreamIndex`; if `> 1` and `AudioTracks.All(t => t.StreamIndex >= 1)` (3+ stream VFR layout where stream 0 is pre-mixed "All"), selects stream `0`, else selects `unmuted[0].StreamIndex`. Sets `_mediaPlayer.Volume = clamp(unmuted.Max(t => t.Volume), 0, 100)`.
- **PlayPause edge cases (`EditorViewModel.cs:1112-1141`)**:
  - If `State == VLCState.Ended`: calls `Stop()`, queues seek to `TrimStartSec`, calls `Play()`.
  - If `CurrentPreviewSec < TrimStartSec - 0.05 || CurrentPreviewSec >= TrimEndSec - 0.05`: queues seek to `TrimStartSec`, then calls `Play()`.

### 4.6 Cuts & Segment Concatenation (`EditorViewModel.cs:1211-1269, 1336-1379`)

- `AddCut(a, b)`: adds `EditorCut(a, b)`, sorts by `StartSec`, merges intervals where `next.StartSec <= prev.EndSec + 0.01`.
- `KeptSegments(start, end)`: walks sorted cuts clamped to `[start, end]`, emits `(cur, cs)` if `cs > cur`, advances `cur = max(cur, ce)`, finally emits `(cur, end)` if `cur < end - 0.01`.
- On export (`ExportAsync` lines 1288-1296): if `ExportAsGif || Cuts.Count == 0`, exports single segment `(start, max(start + 0.2, end))`; otherwise uses `KeptSegments(start, end)` (with fallback to single segment if empty).
- Multiple segments: renders each segment to `%TEMP%\lag-seg-<guid>.<ext>` via `RunOneAsync`, scaling overall progress as `(segIndex * 100.0 + clamp(p, 0, 100)) / segments.Count`, writes `%TEMP%\lag-concat-<guid>.txt` (`file '<escaped_path>'`), then runs `ffmpeg` concat demuxer with `-c copy` (and `-movflags +faststart` for `.mp4`/`.mov`), deleting temp files in `finally`.

---

## 5. Persistence & Formats

The editor itself has **no settings/project file persistence** (all edit state is in-memory per `OpenClip` session). Temporary and output files created/read:

1. **Temp thumbnail & filter preview PNGs** (`EditorViewModel.cs:863-947`):
   - Directory: `%TEMP%\Lag-editor`
   - Timeline thumbs: `thumb-<32hexguid>.png` (10 files, `160px` wide even-height PNGs, deleted immediately after `Bitmap` load).
   - Filter base & previews: `fbase-<32hexguid>.png` (`128px` wide), `f-<32hexguid>.png` (deleted after `Bitmap` load; `fbase` deleted in `finally`).
2. **Temp caption text files** (`EditorViewModel.cs:1441-1443, 1626`):
   - `%TEMP%\lag-txt-<32hexguid>.txt`, UTF-8 plain text containing `EditorTextItem.Text`, deleted in `RunExportAsync` `finally`.
3. **Temp segment & concat files** (`EditorViewModel.cs:1352, 1364-1378`):
   - `%TEMP%\lag-seg-<32hexguid>.<ext>` (for multi-segment cuts).
   - `%TEMP%\lag-concat-<32hexguid>.txt` with lines `file '<path_with_single_quotes_escaped_as_'\''>'`. Both deleted in `finally` / after concat.
4. **Exported output file** (`EditorViewModel.cs:1396-1405`):
   - Saved in the **same directory** as the source `clip.FilePath`: `<baseName>-edit.<ext>`, or `<baseName>-edit2.<ext>`, `<baseName>-edit3.<ext>`, ... if taken, where `<ext>` is `"gif"` (if `ExportAsGif`) or `SelectedExportFormat` (`"mp4"`, `"mkv"`, `"mov"`).
   - Deleted on non-zero exit code or `OperationCanceledException` (`EditorViewModel.cs:1314, 1320`).

---

## 6. Native / External Dependencies

### 6.1 LibVLC (`LibVLCSharp.Shared`)

- **Init (`EditorViewModel.cs:958-964`)**: `Core.Initialize()`, `new LibVLC(new[] { "--no-video-title-show", "--no-osd" })`, `new MediaPlayer(_libVLC)`, `VideoRenderer.Attach(_mediaPlayer)` (which hooks `SetVideoFormatCallbacks` requesting `"RV32"` 32-byte aligned pitch and `SetVideoCallbacks` lock/display into triple-buffered `WriteableBitmap`).
- **Media load (`EditorViewModel.cs:777-786`)**: `new Media(_libVLC, new Uri(clip.FilePath))`, `media.AddOption(":avcodec-hw=none")` (forced software decode because LibVLC `adjust` colour filter bypasses `d3d11va` hardware frames).
- **Adjust filter (`EditorViewModel.cs:654-665`)**: `SetAdjustInt(VideoAdjustOption.Enable, 1|0)`, `SetAdjustFloat(Contrast, f.Contrast * (ColorContrast / 100f))`, `Brightness` (`f.Brightness * (ColorBrightness / 100f)`), `Saturation` (`f.Saturation * (ColorSaturation / 100f)`), `Hue` (`f.Hue`).
- **Playback control**: `Play()`, `Pause()`, `SetPause(true)`, `Stop()`, `State`, `IsPlaying`, `IsSeekable`, `Position`, `Time`, `Length`, `SetRate((float)clamp(EffectiveSpeed, 0.25, 4.0))`, `Mute`, `Volume`, `AudioTrackDescription`, `SetAudioTrack(id)`.
- **Rust (`libmpv`) equivalent**: `libmpv` with `vo=libmpv` render context (OpenGL/software FBO into Slint `Image`), `hwdec=no`, `speed`, `brightness`/`contrast`/`saturation`/`hue` properties (note mpv `brightness`/`contrast`/`saturation` are `-100..100` vs VLC multiplicative factors), `aid` / `mute` / `volume`, `time-pos` / `percent-pos`.

### 6.2 FFprobe (`FFMpegCore.FFProbe`) & FFmpeg CLI (`ffmpeg.exe`)

- **Probe (`EditorViewModel.cs:802-808`)**: `FFProbe.AnalyseAsync(clip.FilePath)` (configured in `App.axaml.cs:44` to `<BaseDir>\ffmpeg`) reads `AudioStreams.Count`, `Duration`, `PrimaryVideoStream?.Width/Height`.
  - *Rust equivalent*: `ffprobe` CLI JSON (`-v quiet -print_format json -show_format -show_streams`) or FFmpeg `libavformat` C API (`avformat_open_input` / `avformat_find_stream_info`).
- **FFmpeg binary resolution (`EditorViewModel.cs:1637-1638`)**: `<AppContext.BaseDirectory>\ffmpeg\ffmpeg.exe`, fallback `"ffmpeg"` on `PATH`. Runs with `UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true`; stderr drained asynchronously (`ReadToEndAsync`); stdout parsed for `out_time_ms=<microseconds>` to report `% = us / 1_000_000.0 / expectedDurationSec * 100.0`. On cancellation: `process.Kill(entireProcessTree: true)`.
- **Exact FFmpeg command lines**:
  1. **Timeline thumbnail (×10)** (`EditorViewModel.cs:872-873`):
     `-ss {t} -i "{clip.FilePath}" -frames:v 1 -vf scale=160:-2 -y "{outPng}"` (`t = DurationSec * (i + 0.5) / 10`)
  2. **Filter preview base frame** (`EditorViewModel.cs:905-906`):
     `-ss {DurationSec / 2} -i "{clip.FilePath}" -frames:v 1 -vf scale=128:-2 -y "{basePng}"`
  3. **Filter preview variant** (`EditorViewModel.cs:921`):
     `-i "{basePng}" -vf "{filter.Graph}" -y "{outPng}"`
  4. **Export — Video chain order** (`EditorViewModel.cs:1526-1551`):
     1. Speed (`|speed - 1.0| > 0.001`): `setpts=PTS/{speed}`
     2. Filter preset (`SelectedFilter.Graph != null`): `{filter.Graph}`
     3. Manual colour (`HasColorAdjust`): `eq=contrast={C/100}:brightness={B/100 - 1}:saturation={S/100}`
     4. Source crop (`IsCropped`): `crop=iw*{CropW}:ih*{CropH}:iw*{CropX}:ih*{CropY}`
     5. Reframe (`AppendReframe` when `SelectedAspect.Ratio != null || IsReframed || IsCropped`, provided `SourceWidth > 0 && SourceHeight > 0`):
        - `baseW = Even(SourceWidth * CropW)`, `baseH = Even(SourceHeight * CropH)`
        - `ow = Even(min(baseW, baseH * r))`, `oh = Even(min(baseH, baseW / r))` (or `baseW, baseH` if Native)
        - `vw = Even(VidW * ow)`, `vh = Even(VidH * oh)`, `ox = Round(VidX * ow)`, `oy = Round(VidY * oh)`
        - `cropX = max(0, -ox) & ~1`, `cropY = max(0, -oy) & ~1`, `px = max(0, ox) & ~1`, `py = max(0, oy) & ~1`
        - `cw = Even(min(vw - cropX, ow - px))`, `ch = Even(min(vh - cropY, oh - py))` (aborts reframe if `cw < 2 || ch < 2`)
        - Appends `scale={vw}:{vh}`, `crop={cw}:{ch}:{cropX}:{cropY}`, `pad={ow}:{oh}:{px}:{py}:black`
     6. Rotate / Flip (`AppendRotateFlip`): `hflip`, `vflip`, `transpose=1` (90°), `transpose=1,transpose=1` (180°), `transpose=2` (270°).
     7. Export Resolution (`SelectedExportResolution.Height > 0`): `scale=-2:{Height}`
     8. Captions (`AppendText`): per non-whitespace item, `drawtext=fontfile='C\:/Windows/Fonts/arialbd.ttf':textfile='{escapedPath}':x=(w-text_w)*{nx}:y=(h-text_h)*{ny}:fontsize=h*{FontPercent/100}:fontcolor={ColorToFfmpeg}:borderw=2:bordercolor=black@0.7`
  5. **Export — GIF (`ExportAsGif == true`)** (`EditorViewModel.cs:1553-1561`):
     `-y -ss {start} -t {duration} -i "{clip.FilePath}" -filter_complex "[0:v:0]{vchain}fps=15,split[ga][gb];[ga]palettegen=max_colors=256[gp];[gb][gp]paletteuse=dither=sierra2_4a[v]" -map "[v]" -an -loop 0 -progress pipe:1 "{outPath}"`
  6. **Export — Video (`ExportAsGif == false`)** (`EditorViewModel.cs:1521, 1563-1620`):
     - Input: `-y -ss {start} -t {duration} -i "{clip.FilePath}"`
     - Video map: `[0:v:0]{vchain}[v]` + `-map "[v]"` if `vparts.Count > 0`, else `-map 0:v:0`.
     - Per audible track (`!IsMuted && Volume > 0` and overlap `(a, b)` inside `[0, duration]` has `b > 0.01 && a < duration - 0.01`):
       - `volume={Volume/100}`
       - If `a > 0.01 || b < duration - 0.01`: `volume=0:enable='lt(t\,{a})+gt(t\,{b})'`
       - If `speedChanged`: chained `atempo` (`0.5` steps if `< 0.5`, `2.0` steps if `> 2.0`, remainder)
       - Label `[a{StreamIndex}]`
     - Audio mix: 1 track -> `-map "[a{idx}]"`; `> 1` tracks -> `{labels}amix=inputs={N}:duration=longest:normalize=0[aout]` + `-map "[aout]"`; `0` tracks -> `-an`.
     - Video codec (first attempt `useNvenc = true`; on non-zero exit retries with `useNvenc = false`):
       - NVENC: `-c:v h264_nvenc -preset p5 -rc vbr -cq 19 -b:v 0`
       - Fallback x264: `-c:v libx264 -preset veryfast -crf 19`
     - Audio codec (when `hasAudio`): `-c:a aac -b:a 192k`
     - Tail: `-movflags +faststart -progress pipe:1 "{outPath}"`
  7. **Export — Multi-segment concat** (`EditorViewModel.cs:1370-1371`):
     `-y -f concat -safe 0 -i "{listFile}" -c copy [-movflags +faststart ]"{outPath}"`

### 6.3 Win32 / OS Dependencies

- `VlcVideoRenderer` P/Invokes `kernel32.dll` `RtlMoveMemory` and `RtlZeroMemory` (`VlcVideoRenderer.cs:142-146`).
- `AppendText` hardcodes Windows font path `C:\Windows\Fonts\arialbd.ttf` (`EditorViewModel.cs:1449`).

---

## 7. Cross-Area Interfaces

- **Incoming (called by Shell / Library / Player)**:
  - Registered as singleton in DI (`App.axaml.cs:365`) and exposed on `MainViewModel.Editor` (`MainViewModel.cs:27`).
  - `MainViewModel.OpenEditor(ReplayClip)` calls `Editor.OpenClip(clip)` (ignoring `clip.IsImage`) and sets `CurrentView = Editor` (`MainViewModel.cs:378-383`). Triggered from:
    - Library context menu -> `LibraryViewModel.RequestEdit` -> `EditClipRequested` (`MainViewModel.cs:284`)
    - Player context menu -> `PlayerViewModel.RequestEditClip` -> `LibraryViewModel.RequestEdit` (`PlayerViewModel.cs:467`)
    - Player toolbar "Edit" button -> `MainViewModel.OpenEditorFromPlayerCommand` (`MainViewModel.cs:387-391`)
    - Left sidebar clip cards in `EditorView` -> `$parent[Window].DataContext.OpenEditorForClipCommand` (`EditorView.axaml:195`, `MainViewModel.cs:403-406`).
  - `MainViewModel.NavigateToEditorCommand` switches `CurrentView = Editor` (preserving last loaded clip), stops `Player` if active, and triggers `Library.RefreshCommand` (`MainViewModel.cs:362-368`).
  - `EditorView` `AttachedToVisualTree` / `DetachedFromVisualTree` call `EditorViewModel.StartPreview()` / `StopPreview()` (`EditorView.axaml.cs:76, 104`).
  - `App` shutdown disposes DI container -> `EditorViewModel.Dispose()` (`App.axaml.cs:307`, `EditorViewModel.cs:1691-1705`).
- **Outgoing (calls into Shell / Library)**:
  - `EditorView.axaml:125`: Back button binds `$parent[Window].DataContext.NavigateToPlayerCommand`.
  - `EditorView.axaml:190`: Left sidebar binds `$parent[Window].DataContext.EditableClips` (`MainViewModel.EditableClips`, rebuilt whenever `Library.Clips` changes, filtering `!c.IsImage`).
  - `EditorViewModel.ExportAsync`: injects `LibraryViewModel` and executes `_library.RefreshCommand.ExecuteAsync(null)` after successful export (`EditorViewModel.cs:122, 1309`).
  - `Localizer.Get` / `Localizer.Format` for runtime strings (`EditorViewModel.cs:454, 587, 599, 772, 828-839, 1282, 1301, 1308, 1313, 1324`).

---

## 8. Self-Tests / Diagnostics

- **No `LAG_*` environment variables** exist for the Editor area (`Program.cs` `LAG_*` tests cover only VFR capture, audio, GC, cadence, stall, and duplication probes).
- **Logging**: Errors and diagnostics are written via `System.Diagnostics.Debug.WriteLine("[Editor] ...")` (`EditorViewModel.cs:643, 667, 710, 845, 894, 942, 1001, 1043, 1071, 1091`), plus `Console.WriteLine("[VlcVideoRenderer] ...")` on video format setup (`VlcVideoRenderer.cs:105`).

---

## 9. Known Bugs / Quirks to Preserve or Fix

1. **Right-clicking a cut band does not remove it (`EditorViewModel.cs:1233-1241`, `EditorView.axaml.cs:705-718`)**: `EditorViewModel.RemoveCut(EditorCut)` has doc comment *"the view calls this when a band is right-clicked"*, but `AddCutBand` sets `IsHitTestVisible = false` and attaches no pointer handler. *Decision*: Decide whether to wire right-click removal in Rust (fix) or omit `RemoveCut` UI (preserve).
2. **Undo omits audio tracks, `IsVideoSelected`, `PendingCutStart`, `SetTrimStartHere`/`SetTrimEndHere`/`ResetTrim`, and text-tab input edits (`EditorViewModel.cs:265-334, 470-473, 1147-1167`)**: `EditSnap` doesn't record `AudioTracks` state or `IsVideoSelected`, and `SetTrimStartHere`/`SetTrimEndHere`/`ResetTrim` / `SetTextColor` / `TextBox` / `FontPercent` slider don't call `PushUndo()`. *Fix in Rust* unless strict 1:1 undo quirk parity is desired.
3. **Text overlay preview vs export when rotated `90°`/`270°` (`EditorView.axaml:226-258`, `EditorView.axaml.cs:338-363`, `EditorViewModel.cs:1547-1550`)**: `TextOverlay` sits outside `PreviewContent`, so `LayoutTextOverlays()` places captions onto the *un-rotated* `FrameAspect`, whereas export runs `AppendRotateFlip` *before* `AppendText` (`drawtext` on rotated output dimensions). When rotated 90°/270°, caption placement and `fontsize=h*frac` differ between preview and export. *Fix in Rust* by computing caption frame/export consistently.
4. **Reframe preview vs export brightness/crop differences**:
   - LibVLC `VideoAdjustOption.Brightness` is multiplicative (`0..2`, `1` = neutral), whereas `ffmpeg eq=brightness=` is additive (`-1..1`, `0` = neutral) — preview `Bright`/`Dramatic`/manual brightness only approximate export (`EditorViewModel.cs:658, 1535`). Preserve approximation if using filter preview, or match `eq` curve more closely.
   - `Warm`/`Cold` use `Hue` `±12` in preview vs `colortemperature=4500|8500` in export (`EditorViewModel.cs:397-398`).
   - `CropMode` preview shows the full source frame and dims outside the crop box, *ignoring* any `SelectedAspect` / `Reframe` until exiting `CropToolActive`. Preserve this UX.
5. **Multi-track preview limitation (`EditorViewModel.cs:683-709`)**: LibVLC plays only one track at a time, so with 2 tracks unmuted on a 2-stream legacy file, only the first unmuted track is heard in preview (export mixes both with `amix`). Preserve or improve with libmpv if desired; export mixing must remain identical.
6. **Timeline handle cursor (`EditorView.axaml:373-385`)**: `StartHandle` and `EndHandle` declare `Cursor="SizeWestEast"` with `IsHitTestVisible="False"`, so the hover cursor never changes to `SizeWestEast` over handles. *Fix in Rust* (show resize cursor within `±12 px` of handles) or preserve.
7. **GIF export ignores middle cuts (`EditorViewModel.cs:1292-1295`)**: Explicitly coded `(ExportAsGif || Cuts.Count == 0)` with comment *"GIF ignores cuts — concatenating GIFs isn't supported"*. **Preserve** (or document if improved).
8. **Single-segment duration floor (`EditorViewModel.cs:1294`)**: `Math.Max(start + 0.2, end)` can extend `end` up to `0.2 s` past `start` if `TrimEndSec - TrimStartSec < 0.2`, and `RunOneAsync` uses `Math.Max(0.05, e - s)`. Preserve safety floor.
9. **Export error with `ffmpeg` fails silently on temp file if `outPath` exists? (`EditorViewModel.cs:1314`)**: Cleans up `outPath` on non-zero exit and cancellation. Preserve cleanup.
10. **Duplicate `ffmpeg -ss` + `Stream` speed (`EditorViewModel.cs:1521, 1529`)**: `-ss` and `-t` are placed *before* `-i` (input options) intentionally so `setpts=PTS/speed` alters output duration properly. **Must preserve** input `-ss`/`-t` placement.
11. **Export button remains disabled only while `IsExporting` (`EditorViewModel.cs:1273`)**: Not disabled when `CurrentClip == null` (though `ExportAsync` early-returns if `CurrentClip == null`). Preserve.
12. **Dead fields/methods**: `EditorAspectOption.CropGeometry` (`EditorViewModel.cs:81`), `EditorViewModel.ApplyAllPreview()` (`EditorViewModel.cs:731`), unused local `srcPng` (`EditorViewModel.cs:916`), unused `Editor_NoClips`/`Editor_Tracks` strings. Do not treat `CropGeometry` as an active LibVLC feature.

---

## 10. Parity Checklist

- [ ] **Header**: Displays localized `Editor` title (`22 px` SemiBold), current clip `DisplayName` (`11 px` mono, ellipsis at `420 px`), `ExportStatus` (`12 px`, ellipsis at `340 px`, tooltip), `Back` button (`rchip`, navigates to Player), and `Export` CTA (`MinWidth 140`) with flyout or `Cancel` CTA with live `%` during export.
- [ ] **Left Sidebar (`240 px`)**: Lists all non-image clips (`EditableClips`) with `190×107` `UniformToFill` thumbnail (`r=8`), filename (`11.5 px` SemiBold), and `ShortMeta` (`10 px` mono); clicking any card loads it and immediately begins playback.
- [ ] **Empty State**: When `CurrentClip == null`, preview stage centers `Choose a clip to edit` (`14 px`, hit-test transparent).
- [ ] **Probing (`ffprobe`)**: Extracts `SourceWidth`, `SourceHeight`, `DurationSec` (min `1 s`), and audio stream count: `>= 3` streams -> tracks `1` (`System sound`) & `2` (`Microphone`); `2` streams -> `0` (`System sound`) & `1` (`Microphone`); `1` stream -> `0` (`Audio`); `0` streams -> empty `AudioTracks`.
- [ ] **Timeline Thumbnails**: Generates 10 frames at `DurationSec * (i + 0.5) / 10` scaled to `160:-2` into `%TEMP%\Lag-editor` and displays them evenly in a `56 px` tall `UniformGrid`.
- [ ] **Filter Thumbnails**: Generates 1 frame at `DurationSec / 2` scaled to `128:-2` and runs each non-null `FilterOptions[i].Graph` to populate `74×42` rounded (`r=8`) filter cards.
- [ ] **Preview Playback & Interpolation**: Software decode (`hwdec=no`); polling timer every `250 ms` syncs time/position, skips over `EditorCut` bands (`posSec in [StartSec - 0.03, EndSec - 0.03)`), and pauses/parks at `TrimEndSec`. UI timer at `33 ms` interpolates playhead at `EffectiveSpeed / DurationSec` across kept ranges.
- [ ] **Space & Ctrl+Z Shortcuts**: `Space` toggles play/pause (not hijacked by focused transport/chip buttons); `Ctrl+Z` pops `EditSnap` and restores trim, speed, aspect, filter, crop, reframe, rotation, flip, manual colour, cuts, and text items.
- [ ] **Timeline Pixel Layout & Interaction**: `56 px` video strip with `#B30A0C0F` outside shades, `10 px` cyan handles (`r=3` outer corners) with `2×18` inner notch, `2 px` white (`0.9` opacity) playhead; `±12 px` handle grab reach; `0.2 s` minimum trim range; `10 px` magnetic snap to audio track edges; dragging `TrimStartSec` magnets playhead to start frame; clicking outside handles scrubs within trimmed range.
- [ ] **Control Bar**: `30×30` white circular play/pause button, `m:ss` / `h:mm:ss` time display, `Set start here`, `Set end here`, `Cut` / `End here` (accent text while marking), and `Reset trim` (resets trim to `0..DurationSec` and clears cuts).
- [ ] **Cuts**: First `Cut` press marks `PendingCutStart` and draws live `#66F04848` band with `#F04848` 1 px side borders between `PendingCutStart` and playhead; second press discards if `< 0.05 s`, else merges overlapping/adjacent (`<= 0.01 s`) cuts and draws `#CC2A0E12` bands behind shades/handles.
- [ ] **Audio Track Lanes (`TrimRangeBar`)**: `32 px` high full-width bar matching video x-scale; `#232C36` body (`#1A1F27` muted), deterministic sine-sum faux waveform (`#415061` / `#2A313B`, `2 px` bars every `4 px`), `#B30A0C0F` shades outside `[RangeStart, RangeEnd]`, `10 px` cyan (`#22D3EE` / `#55FFFFFF` muted) handles with `2×max(8, h*0.45)` `#A60A0C0F` notches, `0.1 s` min gap, `10 px` magnetic snap to video `TrimStartSec`/`TrimEndSec`; overlaid mute checkbox chip (left) and `0–100%` volume slider chip (right).
- [ ] **Linked Trimming**: `Shift+click` on video strip or audio row toggles `.selected` (`AccentBrush` border + `AccentDim` background); dragging video trim handles while video and audio row(s) are selected moves selected audio tracks' matching edge synchronously.
- [ ] **Clip Tools Tab**: Format dropdown (`Native`, `16:9`, `9:16`, `16:10`, `21:9`, `4:3`, `1:1`) resets reframe to `CoverPlacement`; `Crop` toggle switches preview to source-crop mode; Rotate Left/Right (`±90°`) and Flip H/V transform preview & export; Speed segmented strip (`0.5×`, `1×`, `1.5×`, `2×`) + manual `TextBox` clamped to `[0.1, 10.0]` (`[0.25, 4.0]` in preview).
- [ ] **Reframe & Crop Preview Interaction**: Unselected reframe click selects video & starts move; handles (`12×12`, `±11 px` reach) allow non-uniform resize; `9 px` magnetic snap to output frame edges; clamped inside stage bounds; `0.05` minimum normalized size; click-without-move toggles play/pause; click outside deselects & toggles play/pause; `Deselect` chip visible only when `!CropMode && _reframeSelected`; right-click resets crop or reframe (with undo).
- [ ] **Filters & Colour Tab**: 8 presets (`Normal`, `Grayscale`, `Sepia`, `Warm`, `Cold`, `Vibrant`, `Dramatic`, `Bright`) + `Brightness`/`Contrast`/`Saturation` sliders (`0–200%`, default `100%`) + Reset button; live preview approximation combined with preset.
- [ ] **Text Tab & Caption Overlay**: `Add text` adds `"Text"` at `(0.5, 0.12)`, `8%` height, `#FFFFFF`; editable single-line text, `3–25%` size slider, 6 colour swatches (`#FFFFFF`, `#000000`, `#FFD43B`, `#FF6B6B`, `#4DABF7`, `#51CF66`), `Delete` button; dragging caption over preview moves `(Nx, Ny)` within `[0, 1]` and auto-selects tab 2.
- [ ] **Export Options & Output Naming**: Flyout offers `Native`/`1080p`/`720p`/`480p`, `mp4`/`mkv`/`mov` (hidden when `Export as GIF` is checked), and `Export as GIF`; output written alongside source as `<name>-edit.<ext>` (incrementing `2, 3, ...` if exists).
- [ ] **FFmpeg Export Pipeline**: Applies filters in exact order (`setpts` -> preset filter -> `eq` -> source `crop` -> reframe `scale+crop+pad` -> `hflip`/`vflip`/`transpose` -> export resolution `scale=-2:H` -> `drawtext`); audio applies per-track `volume`, `volume=0:enable` gate, chained `atempo`, and `amix=inputs=N:duration=longest:normalize=0`; tries `h264_nvenc` (`-preset p5 -rc vbr -cq 19 -b:v 0`) then `libx264` (`-preset veryfast -crf 19`) with `aac 192k` + `-movflags +faststart`; GIF uses `fps=15,split,palettegen=max_colors=256,paletteuse=dither=sierra2_4a -an -loop 0` ignoring cuts; multi-segment cuts render temp segments and join with `-f concat -safe 0 -c copy`.
- [ ] **Export Progress, Cancel & Refresh**: Parses `out_time_ms=` (microseconds) from `-progress pipe:1`, updates `ExportProgress` & `ExportStatus`, kills process tree & deletes partial file on Cancel or failure, and refreshes `LibraryViewModel` on success.
