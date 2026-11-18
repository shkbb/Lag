# Lag Slint Design System (`ui/`)

1:1 port of the Avalonia 11.2.3 UI design system (`E:\project\InstantReplay\InstantReplay\Themes\`, `App.axaml`, `Controls/`, `Core/`, `Views/`).

## Directory Layout

- `ui/theme/tokens.slint` — `export global Theme` with Dark & Light ("cream") palettes switching reactively on `Theme.mode` (`"dark"` | `"light"` | `"system"`), typography, corner radii, sizing, shadows, gradients, and transition durations.
- `ui/theme/icons.slint` — `export global Icons` (49 Material Design Icons SVGs from `Material.Icons.Avalonia 2.1.10`), `MaterialIcon`, and `LagLogoMark` (22×22 stroked rewind-clock logo).
- `ui/icons/*.svg` — 49 Material Design Icon SVG files (`viewBox="0 0 24 24"`, exact path data from `Material.Icons.dll` 2.1.10).
- `ui/fonts/Inter-*.ttf` — 7 Inter font weights (`Thin`, `ExtraLight`, `Light`, `Regular`, `Medium`, `SemiBold`, `Bold`) extracted from `Avalonia.Fonts.Inter` 11.2.3.
- `ui/components/*.slint` — Reusable Slint components for every Avalonia style/control (`index.slint` re-exports all).
- `ui/gallery.slint` — Component & token showcase (`GalleryWindow`), compiled alongside `ui/spike.slint` via `build.rs`.

---

## Avalonia → Slint Mapping Table

### Theme & Resource Tokens (`Themes/Tokens.axaml`, `Themes/Palette.axaml`, `App.axaml`, `Core/UiConverters.cs`)

| Avalonia Key | Slint Token (`Theme.*`) | Notes |
|---|---|---|
| `RequestedThemeVariant` (`Dark`/`Light`/`Default`) | `Theme.mode` (`"dark"` \| `"light"` \| `"system"`) + `Theme.is-dark` | Switches all themed colors reactively |
| `AccentColor`, `Accent`, `AccentDim`, `AccentRing`, `AccentHover`, `AccentText`, `OnAccent` | `Theme.accent-color`, `Theme.accent`, `Theme.accent-dim`, `Theme.accent-ring`, `Theme.accent-hover`, `Theme.accent-text`, `Theme.on-accent` | ARGB `#AARRGGBB` → CSS `#RRGGBBAA` |
| `SurfaceApp`, `SurfaceCard`, `SurfaceInput`, `SurfaceHover`, `SurfaceHover2` | `Theme.surface-app`, `Theme.surface-card`, `Theme.surface-input`, `Theme.surface-hover`, `Theme.surface-hover-2` | Exact Dark / Light cream hexes |
| `BorderFaint`, `BorderCard`, `BorderInput`, `BorderStrong` | `Theme.border-faint`, `Theme.border-card`, `Theme.border-input`, `Theme.border-strong` | Exact alpha borders |
| `TextHi`, `TextRow`, `TextBtn`, `TextKbd`, `TextMid`, `TextMuted`, `TextFaint`, `TextFaintest`, `DotDivider` | `Theme.text-hi`, `Theme.text-row`, `Theme.text-btn`, `Theme.text-kbd`, `Theme.text-mid`, `Theme.text-muted`, `Theme.text-faint`, `Theme.text-faintest`, `Theme.dot-divider` | Exact Dark / Light hexes |
| `DangerClose`, `DangerStop`, `RecDot`, `StatusIdle` | `Theme.danger-close`, `Theme.danger-stop`, `Theme.rec-dot`, `Theme.status-idle` | Exact Dark / Light hexes |
| `SidebarBrush` .. `VolumeFillBrush` (24 parallel brushes) | `Theme.sidebar-brush` .. `Theme.volume-fill-brush` | 1:1 kebab-case names |
| `StatusReadyBrush` .. `AccentCyanBrush` (7 legacy aliases) | `Theme.status-ready-brush` .. `Theme.accent-cyan-brush` | 1:1 kebab-case names |
| `NavHoverBrush` .. `ScrollThumbPressedBrush` (7 hover/track brushes) | `Theme.nav-hover-brush` .. `Theme.scroll-thumb-pressed-brush` | 1:1 kebab-case names |
| `ToggleSwitch*` (18 keys) | `Theme.toggle-switch-*` | Exact Dark / Light hexes |
| `ComboBoxDropDown*` (3 keys) | `Theme.combo-box-drop-down-*` | Exact Dark / Light hexes |
| `MenuFlyout*` (9 keys) | `Theme.menu-flyout-*` | Exact Dark / Light hexes |
| `ToolTip*` (3 keys) | `Theme.tool-tip-*` | Exact Dark / Light hexes |
| `SystemAccentColor*` (7 keys) | `Theme.system-accent-color*` | Exact Dark / Light hexes |
| `Slider*` (`App.axaml:232-248`) | `Theme.slider-horizontal-height`, `Theme.slider-horizontal-thumb-*`, `Theme.slider-track-theme-height`, `Theme.slider-thumb-background*`, `Theme.slider-track-*` | Exact sizing & colors |
| `IntensityToBrushConverter` (`#E5B84C`, `#E0564C`) | `Theme.intensity-caution`, `Theme.intensity-extreme` | Also used for mic boost & warnings |
| `LinearGradientBrush` (`LibraryView.axaml:376-380`) | `Theme.card-overlay-gradient` | `@linear-gradient(0deg, #08090CD1 0%, #08090C1A 55%, #08090C1A 100%)` |
| `BoxShadow` (`MainWindow.axaml:401`, `SettingsView.axaml:812`) | `Theme.shadow-flyout-*`, `Theme.shadow-modal-*` | Offset/blur/color tokens |
| `FontUi`, `FontDisplay`, `FontMono` | `Theme.font-ui`, `Theme.font-display`, `Theme.font-mono` | Inter bundled in `ui/fonts/` |
| `RadCard` .. `RadKbd` + `OverlayCornerRadius` | `Theme.rad-card` .. `Theme.rad-kbd`, `Theme.rad-overlay`, etc. | Exact pixel radii |
| `LogoArcGeometry`, `LogoCornerGeometry`, `LogoHandsGeometry` | `LagLogoMark` (`ui/theme/icons.slint`) | 22×22 stroked `Path` elements |
| `MaterialIcon Kind="..."` (49 icons) | `Icons.<kebab-name>` + `MaterialIcon` (`ui/theme/icons.slint`) | Exact Material.Icons 2.1.10 SVGs |

### Control Styles → Slint Components (`ui/components/`)

| Avalonia Style / Control | Slint Component | File |
|---|---|---|
| `Button.tbar-accent` (+ `.rec` blink) | `TbarAccentButton` | `buttons.slint` |
| `Button.tbar` | `TbarButton` | `buttons.slint` |
| `Button.wctl` (+ `.close`) | `WindowCtlButton` | `buttons.slint` |
| `Button.railitem` (+ `.active`, `:pressed` icon scale) | `RailItemButton` | `buttons.slint` |
| `Button.cta` (+ `.rec`) | `CtaButton` | `buttons.slint` |
| `Button.surface` | `SurfaceButton` | `buttons.slint` |
| `Button.chip` (+ `.danger`) | `ChipButton` | `buttons.slint` |
| `Button.rchip` (+ `.active`, danger tint) | `RChipButton` | `buttons.slint` |
| `Button.gamechip` (+ `.sel`) | `GameChipButton` | `buttons.slint` |
| `Button.pbtn` | `PlayerIconButton` | `buttons.slint` |
| `Button.transport` | `TransportButton` | `buttons.slint` |
| `Button.playbtn` / `Button.ovplay` | `PlayCircleButton` | `buttons.slint` |
| `Button.nav` / `Button.nav-active` | `NavButton` | `buttons.slint` |
| `ToggleSwitch` (`App.axaml:256-320`) | `LagToggleSwitch` | `toggle_switch.slint` |
| `CheckBox.app` (`DarkTheme.axaml:506-543`) | `LagCheckBox` | `checkbox.slint` |
| `Slider` + `Slider.volume` (`DarkTheme.axaml:553-599`) | `LagSlider` | `slider.slint` |
| `ComboBox` + `ComboBoxItem` (`DarkTheme.axaml:353-407`) | `LagComboBox` | `combobox.slint` |
| `Border.rpill` (`Components.axaml:100-106`) | `RPillTrigger` | `combobox.slint` |
| `SegmentedListBox` / `ListBox.segctl` (`SegmentedListBox.cs`) | `SegmentedControl` | `segmented_control.slint` |
| `Border.rsegtrack` + `Button.rseg` (`Components.axaml:69-97`) | `RSegControl` | `segmented_control.slint` |
| `TextBox` (`DarkTheme.axaml:475-503`) | `LagTextInput` | `text_input.slint` |
| `Border.searchbox` (`LibraryView.axaml:37-57`) | `LagSearchBox` | `text_input.slint` |
| `Border.rcard` | `RCard` | `cards.slint` |
| `Border.rdivider` / `Border.divider` | `RDivider` / `Divider` | `cards.slint` |
| `TextBlock.rgroup` / `TextBlock.section` / `TextBlock.rrow` / `TextBlock.rsub` / `TextBlock.rmono` | `RGroupLabel` / `SectionLabel` / `RRowText` / `RSubText` / `RMonoText` | `cards.slint` |
| `Border.presetcard` (+ `.active`) | `PresetCard` | `cards.slint` |
| `Border.hint` | `HintBanner` | `cards.slint` |
| `Border.captureflyout` | `CaptureFlyoutCard` | `cards.slint` |
| Intensive-quality modal (`SettingsView.axaml:809-821`) | `ModalCard` | `cards.slint` |
| Toast card (`ToastService.cs:77-121`) | `ToastCard` | `cards.slint` |
| Stats HUD (`StatsHudWindow.cs:45-60`) | `StatsHudPill` | `cards.slint` |
| Top-bar status pill (`MainWindow.axaml:289-305`) | `StatusPill` | `cards.slint` |
| `Border.badge` (`SettingsView.axaml:48-53`) | `RecommendedBadge` | `badges.slint` |
| `Border.editedbadge` (`LibraryView.axaml:356-361`) | `EditedBadge` | `badges.slint` |
| Duration badge (`LibraryView.axaml:364-368`) | `DurationBadge` | `badges.slint` |
| `Border.rkbd` (`Components.axaml:55-66`) | `RKbdChip` | `badges.slint` |
| `Border.kbd` + `.listening` (`DarkTheme.axaml:322-350`) | `KbdChipButton` | `badges.slint` |
| `ToolTip` (`DarkTheme.axaml:464-472`) | `LagTooltip` | `tooltip.slint` |
| `ScrollViewer` / `ScrollBar` (`DarkTheme.axaml:703-742`) | `LagScrollView` | `scrollbar.slint` |
| `ContextMenu` / `MenuFlyoutPresenter` / `FlyoutPresenter` / `MenuItem` / `Separator` | `LagContextMenuContainer`, `LagFlyoutPresenter`, `LagMenuItem`, `LagMenuSeparator` | `menu.slint` |
| `Button.tab` + `Border.tabline` (`DarkTheme.axaml:218-252`) | `LagTabHeader` | `tab_header.slint` |
| `ProgressBar.buffer` (`DarkTheme.axaml:745-755`) | `BufferProgressBar` | `meters_and_bars.slint` |
| `Border.meter` + `meterfill` + `metermarker` | `LevelMeterBar` | `meters_and_bars.slint` |
| `Border.gatebar` + `gatehandle` | `NoiseGateBar` | `meters_and_bars.slint` |
| `TrimRangeBar` (`Controls/TrimRangeBar.cs`) | `TrimRangeBar` | `meters_and_bars.slint` |
| `ListBox.replays` item | `ReplayListItem` | `list_items.slint` |
| `ListBox.speedlist` item | `SpeedListItem` | `list_items.slint` |
| `ListBox.filters` item | `FilterCardItem` | `list_items.slint` |
| `Button.clipitem` | `ClipPickerItem` | `list_items.slint` |
| `Border.trow` (+ `.selected`) | `TimelineTrackRow` | `list_items.slint` |
| `Border.chandle` | `CropHandle` | `list_items.slint` |
| `Border.ovbox` | `OverlayPreviewBox` | `list_items.slint` |
| `Border.clipcardr` (+ overlay & badges) | `LibraryClipCard` | `list_items.slint` |

---

## Slint vs. Avalonia Gaps & Workarounds

1. **Font fallback chains (`FontFamily="Cascadia Code, Consolas, ..."`)**: Slint `font-family` accepts a single family name rather than a CSS fallback list. `Theme.font-ui` / `Theme.font-display` use `"Inter"` backed by the 7 TTF weights bundled in `ui/fonts/`, and `Theme.font-mono` uses `"Cascadia Code"`.
2. **Per-corner `CornerRadius` (`3,0,0,3`, `6,0,0,6`)**: Slint `Rectangle.border-radius` is uniform on all four corners. `TrimRangeBar` handles cover the inner `3px` edge with a same-color square `Rectangle`; `NoiseGateBar` relies on parent `clip: true` with `6px` radius to round only outer corners.
3. **Window-snapshot theme cross-fade (`App.SetTheme` 280 ms)**: Avalonia freezes `MainWindow` into a `RenderTargetBitmap` on `OverlayLayer` and fades it out over `280ms`. Slint `.slint` markup has no window-bitmap snapshot overlay; `Theme.mode` flips all `Theme.*` properties instantaneously in one frame (`Theme.dur-theme-fade: 280ms` is exported for optional Rust-side cross-fade if desired).
4. **`PopupWindow` outside-click dismissal (`LagComboBox`)**: Slint 1.18 `PopupWindow` (`close-policy: close-on-click-outside`) does not emit a `closed` callback when dismissed by clicking outside, so `LagComboBox.dropdown-open` (driving the `180deg` chevron rotation) only resets when an item is selected. Views can host an in-window scrim overlay if synced outside-click rotation is required.
5. **Infinite keyframe animations (`RecordCta.rec`, `recpulse`, `capturepulse`, `kbd.listening`)**: Slint lacks CSS `@keyframes`; infinite opacity pulses are implemented with cosine modulation of `animation-tick()` gated behind the active boolean (`recording ? ... : 1.0`) so idle controls do not schedule animation frames.
6. **`FxAnimations.SlideFadeIn` & `TransitioningContentControl` page transitions**: Imperative view/tab transition helpers (`240ms` rise+fade and `220ms` vertical `PageSlide` + `CrossFade`) belong to view hosts rather than leaf widgets; `Theme.dur-slide-fade` and `Theme.dur-pill` provide their exact timings.
7. **Arbitrary `DrawingContext` canvas in `TrimRangeBar.cs`**: Slint has no imperative `OnRender` 2D draw loop. `TrimRangeBar` renders a pool of `160` deterministic `Rectangle` bars using the exact `sin` formula from `TrimRangeBar.cs:113-114` (covers track widths up to `646px`; wider timelines can expand the pool or render a procedural `slint::Image` from Rust).
8. **Tooltips & Context Menus**: Slint has no attached `ToolTip.Tip` or native OS-window `ContextMenu` trigger on arbitrary items. `LagTooltip`, `LagContextMenuContainer`, `LagFlyoutPresenter`, `LagMenuItem`, and `LagMenuSeparator` implement the exact visuals for views to mount in `PopupWindow` / overlay layers.
9. **`TextInput` `CaretBrush`, `Text.line-height`, `Text.max-lines`**: Slint 1.18 `TextInput` does not expose a separate caret color (`CaretBrush` uses `color`), and `Text` does not expose `line-height` (`11px` on `railitem`) or `max-lines` (`2` on `railitem`); `RailItemButton` uses `9.5px` `word-wrap` + `elide` constrained to `56px` width.
10. **Arbitrary `DataTemplate` item templates (`ComboBox`, `SegmentedControl`)**: Slint components cannot accept arbitrary Avalonia `DataTemplate`s per list item across a `for` loop; `LagComboBox` and `SegmentedControl` take typed item structs (`ComboBoxItemData` and `SegmentedOption` with `{ text, tier }`) to preserve `IntensityToBrushConverter` coloring.
11. **`System` theme detection**: `Theme.is-dark` reads `Palette.color-scheme` from `std-widgets.slint` and falls back to `Theme.system-is-dark` (`true`) when the platform reports `ColorScheme.unknown`.
