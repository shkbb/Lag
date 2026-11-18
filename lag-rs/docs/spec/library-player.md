# Clip library and clip player — Rust parity specification

## 1. **Scope**

This is the acceptance specification for the current C# behavior, ported to Rust with Slint and libmpv. Source paths below are relative to `E:\project\InstantReplay\InstantReplay`. All twelve requested files were read in full before this document was written. Citations use physical, one-based source lines. Implementation suggestions are explicitly distinguished from existing behavior; comments do not override executable code.

| Required source | Lines | Responsibility |
|---|---:|---|
| `ViewModels/LibraryViewModel.cs` | 876 | Scan, hydrate, caches, filters, grouping, storage, selection, file commands, group/chip models |
| `ViewModels/PlayerViewModel.cs` | 593 | Shared library sidebar, media lifecycle, transport, rate, volume, position |
| `Views/LibraryView.axaml` | 505 | Library layout and local styles |
| `Views/LibraryView.axaml.cs` | 119 | Input, context menus, chip scrolling, entrance timing |
| `Views/PlayerView.axaml` | 445 | Player layout, controls, sidebar, local styles |
| `Views/PlayerView.axaml.cs` | 359 | Presentation, clock interpolation, mute feedback, fullscreen overlays |
| `Services/VlcVideoRenderer.cs` | 224 | Visible-frame cropping, triple buffering, UI presentation |
| `Services/VlcEngine.cs` | 65 | Untracked work-in-progress shared-engine/prewarming proposal |
| `Services/ClipMetadata.cs` | 111 | Metadata model and hashed JSON store |
| `Models/ReplayClip.cs` | 79 | Shared clip model and display formatting |
| `Services/DynamicRenderTimer.cs` | 189 | Monitor-vblank render cadence and clock fallback |
| `Core/DeferredDispose.cs` | 21 | Delayed disposal of UI-bound resources |
| **Total** | **3,586** | |

Supplemental reads resolve actual shared appearance and callers: `Themes/Tokens.axaml`, `Themes/Palette.axaml`, `Themes/Components.axaml`, relevant `Themes/DarkTheme.axaml` rules, `App.axaml`, `App.axaml.cs`, `Program.cs`, `Views/MainWindow.axaml`, `Views/MainWindow.axaml.cs`, relevant `ViewModels/MainViewModel.cs` and `SettingsViewModel.cs` passages, `Services/HardwareDetector.cs`, `Core/FxAnimations.cs`, English/Ukrainian localization entries, and the installed FFMpegCore 5.4.0 package provenance. No build or runtime playback test was performed. This specification describes the source, including limitations that require runtime validation.

## 2. **Features**

1. Discover saved media in the configured library folder: MP4/MKV/MOV/AVI/GIF videos and PNG/JPG/JPEG screenshots, newest creation time first; no recursive subfolder search. (`ViewModels/LibraryViewModel.cs:181–204`, `ViewModels/LibraryViewModel.cs:391`.)
2. Publish cards after a cheap filesystem/sidecar pass; stream thumbnails and missing durations into already visible cards in the background. (`ViewModels/LibraryViewModel.cs:378–458`, `ViewModels/LibraryViewModel.cs:474–610`.)
3. Retain existing card instances and selection on unchanged-folder refreshes; cancel superseded refreshes and resume missing hydration on a matching scan. (`ViewModels/LibraryViewModel.cs:368–371`, `ViewModels/LibraryViewModel.cs:431–469`.)
4. Cache video JPEG thumbnails persistently, decoded thumbnails in process memory, and positive probed durations in metadata; downscale screenshot previews without generating separate screenshot files. (`ViewModels/LibraryViewModel.cs:530–665`.)
5. Group cards by local creation day with localized Today/Yesterday headings and culture-formatted dates/weekdays. (`ViewModels/LibraryViewModel.cs:113–148`.)
6. Combine immediate name/game search, one toggleable game/Desktop category, and favorites-only filtering; expose no user-controlled sort direction or sort menu. (`ViewModels/LibraryViewModel.cs:52–149`, `Views/LibraryView.axaml:234–275`.)
7. Display image previews, creation clock time, optional game name, duration for videos, persistent favorite badge, and EDITED badge. (`Views/LibraryView.axaml:333–444`.)
8. Hover cards to reveal play, favorite, video-edit actions, an accent ring, and a selection circle; lift cards and animate entrances. (`Views/LibraryView.axaml:59–175`, `Views/LibraryView.axaml:343–428`.)
9. Support accumulating selection, Ctrl-toggle, Shift-ranges across visible day groups, Ctrl+A, Escape, empty-space clearing, and immediate bulk deletion. (`Views/LibraryView.axaml.cs:58–95`, `ViewModels/LibraryViewModel.cs:723–799`.)
10. Offer Edit, Show in folder, and Delete from each card's context menu; open the library directory from its header. (`Views/LibraryView.axaml:225–229`, `Views/LibraryView.axaml:319–331`, `ViewModels/LibraryViewModel.cs:801–837`.)
11. Display drive usage and optionally delete oldest video files to enforce the Settings storage limit; screenshots and favorite status are not quota exclusions in the same way (screenshots are not counted at all; favorites are eligible). (`ViewModels/LibraryViewModel.cs:234–344`.)
12. Open both videos and screenshots in the integrated player, with a full-resolution screenshot and reduced control bar for stills. (`ViewModels/LibraryViewModel.cs:670–675`, `ViewModels/PlayerViewModel.cs:368–414`, `Views/PlayerView.axaml:246–347`.)
13. Play/pause by transport button, center overlay, video click, or Space; Stop resets transport state; natural end parks the seek position at 100%, and Play restarts. (`Views/PlayerView.axaml:12–17`, `Views/PlayerView.axaml:237–293`, `ViewModels/PlayerViewModel.cs:338–357`, `ViewModels/PlayerViewModel.cs:498–528`, `Views/PlayerView.axaml.cs:261–272`.)
14. Seek with a normalized slider and ±10-second buttons/arrow keys, with latest-value coalescing for slider writes. (`Views/PlayerView.axaml:257–258`, `ViewModels/PlayerViewModel.cs:124–129`, `ViewModels/PlayerViewModel.cs:180–217`, `ViewModels/PlayerViewModel.cs:531–542`.)
15. Select playback speeds 2x, 1.5x, 1x, 0.5x, 0.25x, and 0.1x; a newly selected clip resets speed to 1x. (`ViewModels/PlayerViewModel.cs:163–178`, `ViewModels/PlayerViewModel.cs:373–375`.)
16. Adjust volume 0–100 (default 80), mute independently with the speaker/M key, and show brief mute/unmute visual feedback. (`ViewModels/PlayerViewModel.cs:138–161`, `Views/PlayerView.axaml.cs:274–295`.)
17. Select another clip from a live shared sidebar; Next wraps through the unfiltered library and skips screenshots. (`ViewModels/PlayerViewModel.cs:47–81`, `ViewModels/PlayerViewModel.cs:471–487`.)
18. Show total clip count, summed duration, and total media size below the replay list; reuse library context actions on sidebar rows and current video. (`Views/PlayerView.axaml:356–439`, `ViewModels/PlayerViewModel.cs:53–69`, `ViewModels/PlayerViewModel.cs:465–469`.)
19. Enter true window fullscreen from the fullscreen button, exit with Escape, reveal controls near the bottom and the sidebar near the right edge, and hide the cursor after 2.5 seconds idle. (`Views/PlayerView.axaml.cs:19–21`, `Views/PlayerView.axaml.cs:57–126`, `Views/PlayerView.axaml.cs:318–357`, `Views/MainWindow.axaml.cs:129–132`, `Views/MainWindow.axaml.cs:159–165`.)
20. Present video as an ordinary clipped UI visual with overlays and pointer input, protect bitmap lifetimes, and compose at the active monitor's vblank rather than imposing 60 fps. (`Services/VlcVideoRenderer.cs:12–26`, `Services/VlcVideoRenderer.cs:148–197`, `Services/DynamicRenderTimer.cs:81–134`.)
21. Stop playback when leaving the player or hiding the main window to the tray. (`Views/PlayerView.axaml.cs:167–194`, `Views/MainWindow.axaml.cs:43–53`.)
22. **No player audio-track switching feature exists in these files.** The player does not enumerate audio tracks, select a track, or change tracks for slow motion; `ApplyRate` only sets rate. Track controls found in the editor belong to that other area. (`ViewModels/PlayerViewModel.cs:241–248`, `Views/PlayerView.axaml:264–347`; comparison: `ViewModels/EditorViewModel.cs:688–719`.)

## 3. **UI**

### Shared dimensions, palette, and control states

All dimensions are Avalonia logical pixels/DIPs and must scale with display DPI. Four-value margins/paddings are left, top, right, bottom. `FontDisplay=Inter`, `FontMono=Cascadia Code, Consolas, Courier New, monospace`; ordinary TextBlocks use `Inter, Segoe UI, sans-serif`. Radii: `RadGeneral=13`, `RadPill=11`, `RadInput=9`, `RadKbd=6`. (`Themes/Tokens.axaml:12–24`, `Themes/DarkTheme.axaml:15–19`.)

Dynamic resources follow live theme changes. Literal colors in the screen markup remain literal in either theme. Color notation with eight hex digits is **AARRGGBB**. Relevant resolved values follow. (`Themes/Palette.axaml:12–15`, `Themes/Palette.axaml:24–104`, `Themes/Palette.axaml:158–237`.)

| Token | Dark | Light |
|---|---|---|
| Accent / AccentBrush | `#FF22D3EE` | `#FFA9722E` |
| AccentDim | `#1F22D3EE` | `#29A9722E` |
| AccentRing | `#5222D3EE` | `#5CA9722E` |
| AccentText | `#FF7BE7F5` | `#FF7C5320` |
| OnAccent | `#FF06222A` | `#FFFBF4E8` |
| SurfaceApp | `#FF0C0D11` | `#FFEAE3D1` |
| SurfaceCard | `#FF14161C` | `#FFF8F4EA` |
| SurfaceInput | `#FF1A1D25` | `#FFE2D6BA` |
| SurfaceHover2 | `#FF22262F` | `#FFDCD3BB` |
| BorderFaint | `#0EFFFFFF` | `#0F2A2415` |
| BorderCard | `#12FFFFFF` | `#182A2415` |
| BorderInput | `#17FFFFFF` | `#242A2415` |
| BorderStrong | `#1AFFFFFF` | `#302A2415` |
| TextHi | `#FFECEDEF` | `#FF2A2820` |
| TextBtn | `#FFD7DADE` | `#FF3D3A30` |
| TextKbd | `#FFC4C8CE` | `#FF4A4638` |
| TextMid | `#FF9AA0AA` | `#FF6E6A5A` |
| TextMuted | `#FF8B919C` | `#FF807B69` |
| TextFaint | `#FF7C828D` | `#FF928D78` |
| TextFaintest | `#FF6A707B` | `#FFA39D86` |
| DotDivider | `#FF4A505B` | `#FFB7AF98` |
| DangerStop | `#FFEF4D62` | `#FFD93B50` |
| EmptyIconBrush | `#FF3E4754` | `#FFC9C2AC` |
| PlayerBarBrush | `#D10A0C0F` | `#EBF8F4EA` |
| Track14Brush | `#24FFFFFF` | `#262A2415` |
| VolumeFillBrush | `#FFAEB6C0` | `#FF8F8770` |
| CardBrush | `#FF181C23` | `#FFF8F4EA` |
| SurfaceBrush | `#FF20252D` | `#FFEBE4D2` |
| FgBrush | `#FFE7EAEE` | `#FF2A2820` |
| Fg2Brush | `#FF9AA3AD` | `#FF6E6A5A` |
| PopupBorderBrush | `#1FFFFFFF` | `#242A2415` |
| ScrollThumbBrush | `#59FFFFFF` | `#552A2415` |
| ScrollThumbPressedBrush | `#8CFFFFFF` | `#882A2415` |

The shared Button base has `scale(1)` at rest and `scale(0.97)` pressed, with 80 ms transform and 150 ms background transitions. Locally declared transitions can replace the base transitions; retain each local rule below rather than assuming all transitions accumulate. No area-specific disabled button recoloring is defined; stock Fluent behavior supplies otherwise unspecified states. There is no blanket disabled transport state for no clip or screenshots. (`Themes/DarkTheme.axaml:35–47`, `Views/PlayerView.axaml:20–51`, `Views/LibraryView.axaml:11–175`.)

All horizontal sliders retain the Fluent template: height/min-height 20, 4-pixel track, 12×12 white thumb with radius 6 and 4-pixel pre/post content margins, transparent container. Timeline filled portion is AccentBrush, remainder Track14Brush; colors stay pinned during hover/press. Thumb scales 1→1.25 on hover over 120 ms CubicEaseOut; disabled opacity 0.45, with explicitly white disabled thumb and disabled fill `#22D3EE`/track `#24FFFFFF`. Volume uses VolumeFillBrush for its fill, including hover/press. (`App.axaml:230–248`, `Themes/DarkTheme.axaml:545–598`.)

Shared vertical scrollbars: 10-pixel transparent strip, vertical reversed track, 6-pixel-wide thumb, minimum thumb height 28, radius 3. Thumb background is transparent at rest, ScrollThumbBrush over the scrollbar, ScrollThumbPressedBrush when dragging; background transition 150 ms. ScrollViewers allow autohide. (`Themes/DarkTheme.axaml:698–742`.)

Context menus open without a custom entrance animation: CardBrush surface, PopupBorderBrush 1-pixel border, radius 10, padding 4. Rows: transparent background, FgBrush, font 13, padding 10,7, radius 6, margin 0,1, hand cursor; 120 ms row-background transition. Icons are Fg2Brush and turn AccentBrush when selected, with 120 ms foreground transition. Dark hover/pressed row colors `#272D36`/`#2A313B`; Light `#E2DAC4`/`#DCD3BB`. Tooltip surface is SurfaceBrush, FgBrush text, PopupBorderBrush 1-pixel border, radius 8, padding 10,6, font 12; no custom keyframe entrance. (`Themes/DarkTheme.axaml:409–471`, `Themes/Palette.axaml:130–142`, `Themes/Palette.axaml:263–275`.)

### Library layout and header

Layout tree: SurfaceApp root Grid rows `Auto,*`; header StackPanel in first row; body ScrollViewer in second row containing a Panel with day-group ItemsControl, no-results text, and empty-library stack; two loading ProgressBars also occupy the second row. The grid is a nested ItemsControl/WrapPanel, **not virtualized**. (`Views/LibraryView.axaml:184–187`, `Views/LibraryView.axaml:279–316`, `Views/LibraryView.axaml:455–501`, `ViewModels/LibraryViewModel.cs:354–358`.)

Header margin 38,30,38,0. Title `Library_Title`, display font 32 Bold TextHi. Below, margin-top 8 and spacing 16: ClipCountLabel and UsedSpaceDisplay, both mono 12 TextMuted, then 120×5 usage track (BorderStrong, radius 3, clipped) with left-aligned Accent fill, height 5/radius 3, width UsedSpaceBarWidth. Right-aligned header actions are bottom-aligned, horizontal spacing 8. Each uses `rchip`: SurfaceInput, TextHi, 1-pixel BorderStrong, radius 9, padding 15,8, font 12.5 SemiBold, hand cursor; hover SurfaceHover2 with 140 ms background transition. (`Views/LibraryView.axaml:187–231`, `Themes/Components.axaml:109–125`.)

Actions in order: bulk DeleteSelectedLabel (TrashCanOutline 15×15, DangerStop icon/text, only when HasSelection); Clear selection (CloseCircleOutline 15×15, Library_ClearSelection, same visibility); Open folder (FolderOutline 15×15, Library_OpenFolder, always). Icon/label spacing 8. No toolbar Refresh button, rename button, sort control, per-card delete overlay, or confirmation dialog is declared. (`Views/LibraryView.axaml:207–231`, `Views/LibraryView.axaml:405–414`, `ViewModels/LibraryViewModel.cs:700–721`.)

Filter Grid columns `Auto,*,Auto`, margin 0,24,0,4, visible iff Clips.Count is nonzero. Search surface 320×38, padding 14,0, radius 11, SurfaceCard, 1-pixel BorderCard; pointerover border BorderInput over 120 ms. Magnify icon 15×15 TextFaint with right margin 9; text font 13.5 TextHi, watermark Library_Search, vertically centered; TextBox has no inner border/background/padding and MinHeight 0. SearchText is TwoWay. (`Views/LibraryView.axaml:36–57`, `Views/LibraryView.axaml:234–244`.)

Game-chip strip occupies the middle column, margin 10,0,12,0, horizontal StackPanel spacing 7; horizontal scrollbar Hidden, vertical Disabled. Vertical wheel is translated to horizontal offset; tilt-wheel X takes precedence, otherwise Y; offset becomes `clamp(oldX - delta*60, 0, max(0, extentWidth-viewportWidth))`, and nonzero events are handled. (`Views/LibraryView.axaml:246–260`, `Views/LibraryView.axaml.cs:17–20`, `Views/LibraryView.axaml.cs:36–44`.)

Each game chip binds Label, IsActive→`.sel`, FilterKey→SetGameFilterCommand. Favorites is a separate right-column gamechip toggling FavoritesOnly, with 14×14 Star AccentText when on or StarOutline TextMuted when off, icon/text spacing 7, Library_Favorites label. Common chip style: SurfaceCard/TextMuted/BorderCard 1, radius 9, padding 12,7, font 12.5 Medium, hand cursor; hover text TextHi; selected text AccentText/border AccentRing/background AccentDim. Content background transition 120 ms. Selected-chip animation 180 ms CubicEaseOut, ScaleX/Y 0.92→1, FillMode None. There is no explicit All chip; click an active game chip to return to all games. (`Views/LibraryView.axaml:11–34`, `Views/LibraryView.axaml:163–175`, `Views/LibraryView.axaml:255–275`, `ViewModels/LibraryViewModel.cs:55–64`.)

### Library body, groups, and cards

ScrollViewer padding 38,16,38,0, background Transparent, vertical Auto. Group ItemsControl bottom margin 56 **in content flow**, not viewport padding; every group StackPanel bottom margin 34. Group header bottom margin 16; columns `Auto,Auto,*,Auto`: Label display 15 SemiBold TextHi; Sub mono 11 TextFaintest, left margin 13; 1-pixel BorderFaint line with 13-pixel horizontal margins; CountLabel mono 11 TextFaintest. Cards wrap horizontally with fixed width 248 and margin 0,0,20,20, including the last card in each row. (`Views/LibraryView.axaml:285–317`.)

Card outer Border `.clipcardr`: transparent background, hand cursor, focusable; `.selected` binds IsSelected. Content is preview panel then metadata stack. Preview is fixed height 140 at width 248, radius 13, clipped; placeholder background `#16181E`; Thumbnail Image UniformToFill. Accent ring covers preview, 2 pixels, radius 13, non-hit-testable. Ring opacity 0→1 on hover or selection, 150 ms. (`Views/LibraryView.axaml:59–84`, `Views/LibraryView.axaml:316–345`.)

Preview layers, back to front: thumbnail; accent ring; favorite badge; EDITED badge; duration badge; hover overlay with actions; selection circle. Favorite badge: shown iff IsFavorite, top-right margin 0,9,9,0, 26×26, radius 8, `#B30C0D11`, Star 14 Accent, non-hit-testable. EDITED: shown iff IsEdited, top-left 9,9,0,0, Accent background, radius 5, padding 6,3; literal unlocalized `EDITED`, mono 9 Medium OnAccent; fades to opacity 0 on card hover or **this card's selection**, 120 ms. Duration: videos only, bottom-right 0,0,9,9, background `#C70C0D11`, radius 6, padding 7,2, DurationDisplay mono 11 White; non-hit-testable. (`Views/LibraryView.axaml:111–120`, `Views/LibraryView.axaml:347–368`.)

Overlay: radius 13, clipped; IsVisible=IsNotSelecting for the **whole library**, opacity 0/not hit-testable until hovered, then 1/hit-testable, 150 ms. Vertical gradient runs bottom→top: offset 0 `#D108090C`, 0.55 `#1A08090C`, 1 `#1A08090C`. Center play: 48×48/radius 24, Accent, zero padding/border, Play icon 24 OnAccent with left margin 2, PlayClipCommand parameter card (also opens screenshots). Top-right favorite toggle: 28×28 at 0,9,9,0, radius 8, `#EB1A1D25`, Star/StarOutline 15 AccentText/TextBtn. Bottom-left edit: videos only, height 28, padding 11,0, margin 9,0,0,9, radius 8, same dark action background, PencilOutline 13 + Editor_Edit text font 12 SemiBold, spacing 6. Action buttons hover SurfaceHover2 over 120 ms. (`Views/LibraryView.axaml:122–145`, `Views/LibraryView.axaml:370–416`.)

Selection circle stays above overlay: top-left margin 9,9,0,0, 26×26/radius 13, border/padding 0, background `#B30C0D11` and hover `#C70C0D11`; opacity 0/no hit-testing at rest, revealed on card hover or own selection over 120 ms. Unselected icon CheckboxBlankCircleOutline 20 White; selected CheckCircle 20 Accent. Click toggles this clip. Other unselected cards do not permanently reveal circles merely because selection mode is active. (`Views/LibraryView.axaml:86–109`, `Views/LibraryView.axaml:418–428`, `Views/LibraryView.axaml.cs:75–83`.)

Metadata stack margin 2,10,2,0, spacing 3: DisplayName font 13 SemiBold TextHi with CharacterEllipsis; secondary horizontal stack spacing 8: CreatedDate `HH:mm`, mono 10.5 TextFaint; only if HasGame, 3×3 DotDivider ellipse and Game font 11.5 TextMuted. No file-size label on these cards. (`Views/LibraryView.axaml:431–443`.)

Card entrance is **opacity only**, 0→1 in 300 ms CubicEaseOut/FillMode None, despite the comment calling it fade-rise. Hover translates Y from 0 to −3 in 160 ms CubicEaseOut. On each LibraryView attach, LibraryBody is hidden for 240 ms, then shown and SlideFadeIn(body,12,220): opacity 0→1 and Y +12→0 in 220 ms CubicEaseOut, armed at Background UI priority. No cancellation of the delayed attach timer is implemented. (`Views/LibraryView.axaml:147–162`, `Views/LibraryView.axaml.cs:22–33`, `Core/FxAnimations.cs:20–45`.)

Card context menu, in order: Editor_Edit/MovieEditOutline, Library_ShowInFolder/FolderSearchOutline, Library_Delete/TrashCanOutline; all icons 16×16. Edit remains present on screenshots but is a no-op. Right click leaves library selection unchanged. (`Views/LibraryView.axaml:319–331`, `Views/LibraryView.axaml.cs:58–69`, `Views/LibraryView.axaml.cs:97–118`, `ViewModels/LibraryViewModel.cs:172–175`.)

No-results: HasNoResults text Library_NoResults, horizontal center, margin-top 60, TextFaintest, font 13.5. Empty library: `!Clips.Count`, centered stack margin-top 140, spacing 14; 44×44 logo Viewbox around 22×22 Canvas with the three shared logo paths, EmptyIconBrush stroke 0.8, round caps/joins; Library_EmptyTitle font 16 Medium TextHi; Library_Empty font 13 TextMuted, line-height 20, wrapping/centered/max-width 430; HotkeyParts from Window.DataContext.Settings appear as centered keycaps, top margin 2, horizontal spacing 6. Keycaps use SurfaceInput, BorderStrong 1, radius 6, padding 9,4; mono 11.5 TextKbd. Logo geometry comes from `LogoArcGeometry`, `LogoCornerGeometry`, `LogoHandsGeometry`, not a substituted generic icon. (`Views/LibraryView.axaml:455–494`, `Themes/Components.axaml:55–65`, `Themes/Tokens.axaml:28–30`.)

Two indeterminate loading bars share IsLoading: full-width height/min-height 3 at body top, ZIndex 5/border 0; another height 2 at body top with margin 38,0. Both Accent/Transparent. IsLoading covers scan/publication, not complete thumbnail hydration. (`Views/LibraryView.axaml:280–283`, `Views/LibraryView.axaml:498–501`, `ViewModels/LibraryViewModel.cs:457–469`.)

### Player layout and presentation

Root focusable UserControl; PlayerArea Grid margin 24,20,24,24, SurfaceApp. Child Grid columns `*,Auto`: left Grid rows `Auto,*` for title and video, right ReplayPanel. HeaderBlock hidden when Window.DataContext.IsFullscreen; spacing 3/bottom margin 16. Title CurrentClip.DisplayName, fallback empty, display 22 SemiBold TextHi, CharacterEllipsis. Meta horizontal spacing 5: CreatedDate `d MMM, HH:mm`, middle dot visible iff CurrentClip exists, DurationDisplay; mono 11.5 TextFaint. Screenshot headers still show the duration label (usually 0:00). (`Views/PlayerView.axaml:1–8`, `Views/PlayerView.axaml:139–163`.)

VideoFrame: background literal `#FF07090C`, BorderCard 1-pixel border, radius **12** (comment says r16 but code wins), ClipToBounds. VideoSurface transparent Panel handles left press and context menu; VideoImage stretches uniformly (letterboxes, no fill-crop), HighQuality interpolation, stretches in both layout axes. Same image control switches between live renderer Bitmap and full-resolution StillImage. (`Views/PlayerView.axaml:165–192`, `Views/PlayerView.axaml.cs:252–259`.)

With no clip, centered non-hit-testable placeholder StackPanel spacing 10: shared 44×44 logo as above and Player_Placeholder font 13 TextFaint. Big center play button is 66×66/radius 33 Accent, non-focusable, Play icon 26 OnAccent with left margin 3. Its **only** visibility condition is `!IsPlaying`; it therefore also appears with no clip and in screenshot mode, where its command does nothing. (`Views/PlayerView.axaml:195–212`, `Views/PlayerView.axaml:237–244`, `ViewModels/PlayerViewModel.cs:500–502`.)

MuteOverlay is non-hit-testable, hidden for images, centered 96×96/radius 48, `#8C000000`, initial opacity 0, opacity transition 300 ms. VolumeOff/VolumeHigh glyphs are 46×46 White, according to IsMuted. Any IsMuted change while attached sets opacity 1 and restarts an 850 ms timer; expiry stops timer and sets opacity 0. Speaker-icon press also scales VolumeIcon to 1.35 and returns to 1 after 130 ms; transform transition 130 ms CubicEaseOut, origin 50%,50%. Its handler does not check left mouse button. (`Views/PlayerView.axaml:214–230`, `Views/PlayerView.axaml:324–335`, `Views/PlayerView.axaml.cs:33–48`, `Views/PlayerView.axaml.cs:203–213`, `Views/PlayerView.axaml.cs:274–295`.)

Clip-open loading strip: indeterminate, IsClipLoading, height/min-height 3, top aligned, margin 1,1,1,0, Accent, transparent background, border 0. Its intended end condition in executable code is first FrameRendered, **not Playing**. (`Views/PlayerView.axaml:232–235`, `ViewModels/PlayerViewModel.cs:319–324`.)

### Player controls — every control and binding

ControlsBar overlays video bottom with margin 12, PlayerBarBrush, radius 12, padding 14,12,14,10; inner vertical spacing 10. Screenshot mode `.compact` right-aligns the bar. Progress row hidden for images: columns `Auto,*`; TimeText initialized to `0:00.000 / 0:00.000`, mono 11.5 TextMid, margin-right 12/min-width 120; seek Slider 0–1 TwoWay Position. Buttons row is height 40. (`Views/PlayerView.axaml:133–136`, `Views/PlayerView.axaml:246–262`.)

Standard `.pbtn`: transparent, TextMid, 38×38, radius 10, no padding/border, hand cursor, centered content; hover TextHi and BorderFaint background with 120 ms content-background transition. `.playbtn`: Accent, zero border/padding, hand, centered content. Local controls define no custom pressed color; inherit base pressed scale. (`Views/PlayerView.axaml:20–51`.)

| Order / position | Control / appearance | Action and tooltip resource |
|---|---|---|
| Center transport 1 | pbtn, Stop 17 | StopCommand; Tooltip_Stop |
| Center transport 2 | pbtn, RotateLeft 18 | SeekBackwardCommand; Tooltip_Rewind |
| Center transport 3 | playbtn 48×48/radius 24, margin 4,0, Focusable false; Play 19/left margin 2 when paused; Pause 18 when playing; OnAccent | PlayPauseCommand; Tooltip_PlayPause |
| Center transport 4 | pbtn, RotateRight 18 | SeekForwardCommand; Tooltip_Forward |
| Center transport 5 | pbtn, SkipNext 18 | PlayNextCommand; Tooltip_Next |
| Right cluster 1 | SpeedButton pbtn auto width (`NaN`), min-width 40, padding 9,0; SpeedLabel mono 12 | Opens speed flyout above; no explicit tooltip |
| Right cluster 2 | pbtn 34×34/radius 9, MovieEditOutline 16 | Window.MainViewModel.OpenEditorFromPlayerCommand; Editor_Edit |
| Right cluster 3 | VolumeIcon transparent Panel/hand, VolumeHigh 16 TextMid or VolumeOff 16 DangerStop | OnVolumeIconPressed→ToggleMuteCommand; Tooltip_Volume |
| Right cluster 4 | Slider.volume width 56, 0–100, TwoWay Volume | Volume property; Tooltip_Volume |
| Right cluster 5 | pbtn 34×34/radius 9, Fullscreen 16 | Window.MainViewModel.ToggleFullscreenCommand; Tooltip_Fullscreen |

Transport stack is horizontally centered, vertical center, spacing 6. Right cluster is right/center aligned, spacing 6. Entire transport, timeline, speed, edit, speaker, volume are hidden for screenshots; fullscreen remains. Fullscreen icon stays Fullscreen even while fullscreen. There are no previous-clip, frame-step, audio-track, zoom, repeat, or separate exit-fullscreen buttons. (`Views/PlayerView.axaml:264–347`.)

Speed flyout: placement Top; shared FlyoutPresenter SurfaceCard, BorderStrong 1, radius 13, padding 8. ListBox width 96/max-height 320, transparent/no border/padding, ItemsSource SpeedOptions, SelectedItem TwoWay PlaybackSpeed; label `{0}x`, font 12.5. Rows padding 10,7, margin 0,1, radius 7, TextMid, content stretched; hover SurfaceInput, selected AccentDim with AccentText SemiBold; background transition 120 ms. Opening pins the control bar visible until closing; closing only clears `_controlsPinned`, without immediately recomputing hover visibility or explicitly closing on selection. (`Views/PlayerView.axaml:74–99`, `Views/PlayerView.axaml:299–315`, `Themes/Components.axaml:127–135`, `Views/PlayerView.axaml.cs:244–250`.)

Seek bar has no application-custom drag-start/drag-end, drag pause/resume, hover time preview, snapping, release-only seek, scrub-specific timer suppression, or debounce duration. It is a standard Fluent Slider whose **every changed Value** updates Position; see coalescing under State & logic. Native position ticks can update Position during a drag. (`Views/PlayerView.axaml:257–258`, `ViewModels/PlayerViewModel.cs:124–129`, `ViewModels/PlayerViewModel.cs:549–567`, `Themes/DarkTheme.axaml:545–560`.)

Inherited Avalonia **11.2.3** behavior (dependency pin `InstantReplay.csproj:58–60`): left track press immediately jumps to pointer and enters dragging; pointer moves update continuously; release ends dragging. Horizontal mapping is `Minimum + clamp((pointerX-thumbWidth/2)/(trackWidth-thumbWidth),0,1)*(Maximum-Minimum)`. Thumb drag also updates through pointer position. Snap-to-tick defaults false. Focused Slider's unhandled, unmodified key handling includes Left/Down decrement SmallChange, Right/Up increment SmallChange, PageUp/PageDown ±LargeChange, Home/End endpoints; actual player-key-binding precedence must retain toolkit routing. These inherited actions apply to volume too; they are distinct from view-level ±10-second commands. ([Pinned Slider.cs lines 176–257 and 310–399](https://github.com/AvaloniaUI/Avalonia/blob/11.2.3/src/Avalonia.Controls/Slider.cs#L176), [Fluent Slider template lines 143–175](https://github.com/AvaloniaUI/Avalonia/blob/11.2.3/src/Avalonia.Themes.Fluent/Controls/Slider.xaml#L143).)

Keyboard bindings scoped to focused player: Space→PlayPause, Left→SeekBackward (−10 s), Right→SeekForward (+10 s), M→ToggleMute. Window tunnel handler consumes Escape only when fullscreen and exits it. No F/F11 toggle, Up/Down volume shortcut, Delete shortcut, or audio-track key is defined here. Player grabs focus on attach, after fullscreen layout update, and after left video/speaker press. Video left press handles the event and toggles playback; right press opens menu and does not toggle. There is no double-click fullscreen handler. (`Views/PlayerView.axaml:10–17`, `Views/PlayerView.axaml.cs:163–164`, `Views/PlayerView.axaml.cs:261–286`, `Views/PlayerView.axaml.cs:356–357`, `Views/MainWindow.axaml.cs:16–17`, `Views/MainWindow.axaml.cs:159–165`.)

### Replay sidebar and fullscreen

ReplayPanel windowed width 300, margin 16,0,0,0, SurfaceApp, BorderCard 1, radius 14. Grid rows `Auto,*,Auto`; loading ProgressBar at top uses Library.IsLoading, height/min-height 3, margin 1,1,1,0, Accent/Transparent/border 0. Heading margin 18,18,18,12: Player_Replays and TotalClips at opposite ends, mono 11 TextFaintest. (`Views/PlayerView.axaml:356–374`.)

ListBox margin 10,0, transparent/no border/padding, ItemsSource shared Clips, SelectedItem TwoWay ActiveSidebarClip, horizontal scrollbar Disabled. Local row padding 8, bottom margin 2, radius 9; hover SurfaceCard, selected AccentDim, background transition 120 ms. Shared theme also supplies transparent rest background and hand cursor; local row sizing supersedes older 6-pixel/8-radius/8-bottom-margin rules. The template ContentPresenter has a separately specified radius 8 in the global style, so do not assume it becomes 9 automatically. Each entire row Grid has a Transparent brush for context-menu hit-testing. Thumbnail 64×36, radius 7 and explicit matching rounded RectangleGeometry clip, `#16181E`, UniformToFill. Text stack left margin 11, spacing 2, vertical-center: DisplayName font 12 SemiBold TextHi ellipsis, ShortMeta mono 10 TextFaint. (`Views/PlayerView.axaml:53–72`, `Views/PlayerView.axaml:376–412`, `Themes/DarkTheme.axaml:601–625`.)

Footer margin 18,14, spacing 8; top BorderCard hairline height 1/bottom margin 2; three `*,Auto` rows: Player_Clips→TotalClips, Player_Duration→TotalDurationDisplay, Player_Used→TotalSizeDisplay. Labels 12.5 TextMuted, values mono 12.5 TextHi. (`Views/PlayerView.axaml:418–439`.)

Video and sidebar have the same three context actions/order/icons as cards. Video menu visible only with CurrentClip. Target resolution is menu DataContext ReplayClip for a sidebar item, otherwise CurrentClip. Clicking a menu on another sidebar item must target that item rather than whichever video happens to be playing. Delete does not implement an explicit stop/current-clip clear. (`Views/PlayerView.axaml:176–189`, `Views/PlayerView.axaml:384–397`, `Views/PlayerView.axaml.cs:297–310`, `ViewModels/PlayerViewModel.cs:465–469`.)

Fullscreen changes MainWindow to FullScreen and back to **Normal**, not previous maximized state; shell top bar and 78-pixel navigation rail collapse via IsFullscreen bindings. HeaderBlock is hidden. PlayerArea margin 0, VideoFrame radius 0 and border 0, so same-aspect video fills edge-to-edge. ReplayPanel moves from column 1 to column 0, right aligned, margin 0,12,12,118, width still 300; it overlays the video rather than reserving width. Windowed restores original margins/radius/border/column/stretch. (`Views/MainWindow.axaml:219–221`, `Views/MainWindow.axaml:326–328`, `Views/MainWindow.axaml.cs:129–132`, `Views/PlayerView.axaml.cs:318–344`.)

Controls start hidden on attach/fullscreen-layout update in **both** modes; replay panel starts visible windowed, hidden fullscreen. Hidden ControlsBar has opacity 0/Y +20 and no hit-testing; hidden replay panel opacity 0/X +24/no hit-testing; show returns translation 0/opacity 1. Both opacity/transform transition over 250 ms CubicEaseOut. Hidden elements still participate in layout. (`Views/PlayerView.axaml:101–131`, `Views/PlayerView.axaml.cs:63–73`, `Views/PlayerView.axaml.cs:346–350`.)

Windowed pointer movement reveals controls iff inside inclusive VideoFrame bounds or speed flyout is pinned; sidebar remains shown. Fullscreen reveal zones use PlayerArea coordinates: bottom iff `y >= height-140`; right iff `x >= width-280` **and not bottom**. Bottom owns bottom-right, so sidebar hides there; right zone is 280 even though panel is 300 wide. No explicit hide delay is used for overlays; movement recomputes immediately. Exiting PlayerArea hides controls unless pinned, and hides sidebar only fullscreen. Every move restores default cursor/restarts 2.5-second idle timer when fullscreen; expiry hides PlayerArea cursor independently of playback state/overlay visibility; exiting fullscreen stops timer and restores cursor. (`Views/PlayerView.axaml.cs:57–126`, `Views/PlayerView.axaml.cs:346–357`.)

### Localization contract

Use these exact keys, resolving through the existing localization area. Static UI DynamicResource strings can update with language resources; computed view-model strings are recomputed only on the events described below (no language-change handler in the listed models). English values are recorded for verification; other languages use their resource dictionaries rather than newly invented translations. (`Views/LibraryView.axaml:192–228`, `Views/LibraryView.axaml:240–273`, `Views/LibraryView.axaml:321–327`, `Views/LibraryView.axaml:456–478`, `Views/PlayerView.axaml:209`, `Views/PlayerView.axaml:268–345`, `Views/PlayerView.axaml:370–437`.)

| Keys | English values | Resource source |
|---|---|---|
| Library_Title; Library_Empty; Library_Delete; Library_OpenFolder; Library_ShowInFolder | Library; No replays yet. Start recording and press the hotkey to save a replay!; Delete; Open folder; Show in folder | `Assets/Langs/en.axaml:131–136` |
| Library_Search; Library_Favorites; Library_Desktop; Library_Today; Library_Yesterday | Search clips; Favorites; Desktop; Today; Yesterday | `Assets/Langs/en.axaml:138–142` |
| Library_NoResults; Library_ClipCount; Library_GroupCount; Library_ClearSelection | No clips match your search.; `{0} CLIPS`; `{0} clips`; Clear selection | `Assets/Langs/en.axaml:143–146` |
| Library_EmptyTitle; Library_Stats | No replays yet; `{0} clips · {1} free` | `Assets/Langs/en.axaml:194–197` |
| Editor_Edit | Edit | `Assets/Langs/en.axaml:67` |
| Player_Replays; Player_Clips; Player_Duration; Player_Placeholder; Player_Used | REPLAYS; Clips; Duration; Video player; Used | `Assets/Langs/en.axaml:149–152`, `Assets/Langs/en.axaml:195` |
| Tooltip_Stop; Tooltip_Rewind; Tooltip_PlayPause; Tooltip_Forward; Tooltip_Next | Stop; Back 10s; Play / Pause; Forward 10s; Next replay | `Assets/Langs/en.axaml:155–159` |
| Tooltip_Fullscreen; Tooltip_Volume | Fullscreen (ESC to exit); Volume | `Assets/Langs/en.axaml:160–161` |
| Time_SecShort; Time_MinShort; Time_HourMinShort | `{0} sec`; `{0} min`; `{0} h {1} min` | `Assets/Langs/en.axaml:186–188` |

`Library_Stats` is computed but not displayed by this Library markup; Library_Subtitle, Library_FreeSpace, Player_NowPlaying, and Player_ReplayBadge exist in localization resources but are not used by the current screens. `EDITED`, `GB`, `MB`, `KB`, `B`, and speed suffix `x` are literal. (`ViewModels/LibraryViewModel.cs:251–252`, `Views/LibraryView.axaml:189–204`, `Views/LibraryView.axaml:359`, `Models/ReplayClip.cs:59–65`, `ViewModels/PlayerViewModel.cs:65–67`, `ViewModels/PlayerViewModel.cs:172`, `Assets/Langs/en.axaml:132–137`, `Assets/Langs/en.axaml:153–154`.)

## 4. **State & logic**

### Clip model and public model surfaces

ReplayClip is shared by Library and Player. Observable fields: IsSelected=false, IsFavorite=false, Thumbnail=null, Duration=TimeSpan.Zero. Duration changes notify both DurationDisplay and ShortMeta. Plain properties: nullable Game/Exe; IsEdited=false; IsImage=false; FilePath and ThumbnailPath empty strings; CreatedDate default DateTime; FileSize=0. HasGame is `!string.IsNullOrWhiteSpace(Game)`. DisplayName is filename without extension. FileSizeDisplay uses binary thresholds: ≥1,073,741,824→one-decimal GB; ≥1,048,576→one-decimal MB; ≥1024→one-decimal KB; otherwise integer B. DurationDisplay is `h:mm:ss` for TotalHours≥1, otherwise `m:ss`; component h/m formatting is not total-hour/total-minute formatting. ShortMeta is `DurationDisplay + " · " + CreatedDate(dd.MM)`. (`Models/ReplayClip.cs:8–78`.)

LibraryViewModel initializes Title=`Library`, holds SettingsViewModel, and exposes Clips, GameChips, Groups (observable collections). SelectedClip is an observable nullable property but is unused by the grid's selection logic; do not replace multi-selection with it. Other observable defaults: IsLoading=false, SearchText empty, FavoritesOnly=false, HasNoResults=false, FreeSpaceDisplay/UsedSpaceDisplay=`—`, UsedSpacePercent=0; internal game filter and anchor initially null. GameChip contains immutable Label, FilterKey (`filterKey ?? label`), observable IsActive. ClipGroup has init-only Label/Sub/CountLabel (empty by default), Items observable collection. (`ViewModels/LibraryViewModel.cs:18–50`, `ViewModels/LibraryViewModel.cs:234–252`, `ViewModels/LibraryViewModel.cs:334–344`, `ViewModels/LibraryViewModel.cs:841–874`.)

Library generated command surface: SetGameFilter, ToggleFavoritesOnly, Refresh (async), PlayClip, RequestEditClip, ToggleFavorite, DeleteClip (async signature but no await in its body), SelectAll, ClearSelection, DeleteSelected (async), ShowInFolder, OpenLibraryFolder. Plain RequestEdit emits EditClipRequested unless IsImage; PlayClip emits PlayClipRequested for any non-null clip including screenshots. Null clip arguments no-op. (`ViewModels/LibraryViewModel.cs:55–68`, `ViewModels/LibraryViewModel.cs:163–175`, `ViewModels/LibraryViewModel.cs:361–362`, `ViewModels/LibraryViewModel.cs:670–837`.)

### Library scan, publication, and hydration algorithm

1. On construction, Task.Run performs EnforceStorageQuota, then posts UpdateFreeSpace to UI thread; construction does not scan/populate Clips. (`ViewModels/LibraryViewModel.cs:334–344`.)
2. Refresh cancels the previous CTS, creates/stores a new CTS, and sets IsLoading=true. It snapshots Settings.LibraryPath into libraryDir. Cancellation is a superseding-refresh mechanism; these files do not cancel simply on leaving LibraryView. (`ViewModels/LibraryViewModel.cs:351–384`, `Views/LibraryView.axaml.cs:13–34`.)
3. Phase 1 runs inside Task.Run(token). If the folder does not exist, create it and return an empty list/signature. Otherwise enumerate **top-level only** using DirectoryInfo.GetFiles for patterns in this order: `*.mp4`, `*.mkv`, `*.mov`, `*.avi`, `*.gif`, `*.png`, `*.jpg`, `*.jpeg`; globally OrderByDescending CreationTime. No LastWriteTime sort, recursive scan, wildcard-all-media decoder detection, or explicit secondary tie sort. Same-time ordering follows stable pattern/enumeration order. Use enumeration FileInfo size/time directly. (`ViewModels/LibraryViewModel.cs:184–204`, `ViewModels/LibraryViewModel.cs:384–400`.)
4. Before each clip, break if cancellation requested. Append to signature: `FullName + "|" + Length + "|" + LastWriteTimeUtc.Ticks + "\n"`, in scan order. This is the **literal concatenated signature**, not a digest. .NET Ticks are 100-ns units from 0001-01-01. Signature excludes CreationTime values themselves, JSON contents, duration, game, favorite, and thumbnail file timestamps. (`ViewModels/LibraryViewModel.cs:389–400`, `ViewModels/LibraryViewModel.cs:427`.)
5. Read metadata. IsEdited=`metadata.Edited || basename.Contains("-edit", OrdinalIgnoreCase)`. IsImage only for lower-invariant extension `.png`, `.jpg`, `.jpeg`. ThumbnailPath=image's own path for screenshots, hashed JPEG path for videos. Seed Thumbnail from decoded-cache lookup at that thumbnail path; no decoding/probing/generation in phase 1. Duration=positive cached DurationSeconds, else zero (even for an image if an unusual sidecar contains a duration). Set CreatedDate=FileInfo.CreationTime (local), FileSize=Length, Game/Exe from metadata, IsFavorite default false. (`ViewModels/LibraryViewModel.cs:395–425`.)
6. After await, discard results if token canceled or stored CTS is no longer this CTS. If published signature is non-null, matches, **and** Clips.Count==built.Count, keep original Clips/group/card instances and selection. Collect existing cards missing Thumbnail or video Duration≤0 and fire-and-forget hydration only for them; otherwise return without any hydration/housekeeping. Phase 1 still read every sidecar even though its newly built objects are discarded on this fast path. (`ViewModels/LibraryViewModel.cs:431–445`.)
7. Changed scan: on captured UI continuation, clear Clips, add built objects one by one, set anchor null, rebuild chips/groups/header count, save signature; fire-and-forget phase 2. Only OperationCanceledException is caught by Refresh itself; other scan failures propagate to its caller. Finally, only the latest CTS owner refreshes drive information, raises LibraryStats and selection-property notifications, and sets IsLoading=false; this does **not** wait for phase 2. CTS instances are not disposed here. (`ViewModels/LibraryViewModel.cs:448–471`.)
8. Phase 2 first runs background housekeeping: create thumbnail cache directory; recursively remove `libraryDir/.thumbnails` if present, catching/logging cleanup failure; enforce quota. This happens **after cards are published**, unlike the stale pre-refresh-quota comment. No automatic rescan/removal of quota-deleted cards follows inside this hydration call. (`ViewModels/LibraryViewModel.cs:218–229`, `ViewModels/LibraryViewModel.cs:481–493`.)
9. Parallelism=`clamp(Environment.ProcessorCount / 2, 2, 6)`, integer division. Iterate newest-first input list; await SemaphoreSlim before scheduling each Task.Run(token); worker awaits HydrateClipAsync then releases gate in finally; await all tasks. Semaphore is intentionally not disposed while canceled workers unwind. Cancellation is swallowed; other hydration errors log `[Library] hydrate failed: ...`. (`ViewModels/LibraryViewModel.cs:495–522`.)
10. Per card: if Thumbnail already exists and (Duration>0 or IsImage), return immediately. Screenshot branch opens original file read-only, DecodeToWidth(stream,640), and caches under original screenshot path/write-time key; exceptions log and leave blank card. No separate disk preview or FFmpeg call for screenshots. (`ViewModels/LibraryViewModel.cs:530–553`.)
11. Video branch: if Duration≤0, FFProbe.AnalyseAsync(filePath), use mediaInfo.Duration, and persist only positive durations with SetDuration. Probe exceptions are caught/logged; generation can still proceed using duration zero. Check cancellation after probe. (`ViewModels/LibraryViewModel.cs:555–575`.)
12. Generate thumbnail iff absent or thumbnail local LastWriteTime is **strictly older** than original local LastWriteTime. Snapshot time is **2 s if duration.TotalSeconds >3**, otherwise **0 s** (exactly 3 s→0). Exact application call: `await FFMpeg.SnapshotAsync(filePath, thumbPath, new System.Drawing.Size(320,180), snapshotTime);`. Output path ends `.jpg`. No custom JPEG quality, filter, process timeout, hardware decode flag, or cancellation token is passed by this call site. If resulting file exists, decode full JPEG Bitmap from stream and cache it. Snapshot/decode failure logs and leaves preview absent. (`ViewModels/LibraryViewModel.cs:577–595`.)
13. The installed dependency is FFMpegCore 5.4.0 (`InstantReplay.csproj:66`). Its package provenance pins upstream commit `ed8f899d04a1f71c481e02b2123176d5cd097a4a`. The wrapper itself performs an additional FFProbe for SnapshotAsync, enables overwrite, selects primary video stream (fallback first/0), seeks on input, chooses image codec from `.jpg`, emits one frame, and resizes. Thus a first-time duration probe plus thumbnail generation can invoke ffprobe **twice**; “one probe ever” comments describe application-level duration caching, not all wrapper work. ([Pinned FFMpeg.cs, lines 38–75](https://github.com/rosenbjerg/FFMpegCore/blob/ed8f899d04a1f71c481e02b2123176d5cd097a4a/FFMpegCore/FFMpeg/FFMpeg.cs#L38), [Pinned SnapshotArgumentBuilder.cs, lines 8–50](https://github.com/rosenbjerg/FFMpegCore/blob/ed8f899d04a1f71c481e02b2123176d5cd097a4a/FFMpegCore/FFMpeg/SnapshotArgumentBuilder.cs#L8).) An equivalent Rust FFmpeg invocation must select that stream, input-seek to 0/2 s, output one JPEG and overwrite at 320×180; this is a semantic command description, not a claim that the C# call site contains a literal shell command. Preserve the call's behavior rather than assuming a default one-third-duration snapshot.
14. On completion, if canceled or both bitmap null and duration unchanged, return. Otherwise Post to UI thread; recheck cancellation, then set non-null Thumbnail and positive Duration. Do not dispose a canceled bitmap: it may already be in shared cache or displayed by another card. External FFProbe/Snapshot calls are not passed this refresh's token, so cancellation suppresses publication rather than reliably terminating their processes; cache/sidecar writes may finish after cancellation. (`ViewModels/LibraryViewModel.cs:598–609`, `ViewModels/LibraryViewModel.cs:564–589`.)

### Decoded-cache behavior

Static ConcurrentDictionary<string,Bitmap>. Key=`thumbnailPath + "|" + File.GetLastWriteTimeUtc(thumbnailPath).Ticks`; return null key if file absent or stat throws. For screenshot thumbnails this is the original image path. Paths in this memory key are not lowercased/normalized. Get returns existing Bitmap reference. Insert: obtain key; if null return; if cache count≥600, Clear **the entire dictionary**, then assign key→bitmap. Clear never disposes bitmaps; cards may still use them, and their lifetime follows references/GC. Cache lives across view-model refreshes; no LRU, disk-size quota, periodic disk eviction, or per-delete invalidation is implemented. (`ViewModels/LibraryViewModel.cs:612–652`.)

Disk thumbnail filename=`uppercase_hex(MD5(UTF8(videoPath.ToLowerInvariant()))) + ".jpg"`. Hash only supplied path string, not file bytes/size/write time. Typical scan paths are FullName; hash function itself does not call GetFullPath, canonicalize slashes, resolve links, or trim. Same name in different directories is distinct; changing library directory does not erase old cached thumbnails. (`ViewModels/LibraryViewModel.cs:655–665`.)

### Filters, grouping, and selection

RebuildGameChips uses all Clips, independently of search/favorites. Game names with HasGame=true: Distinct OrdinalIgnoreCase, sorted CurrentCultureIgnoreCase; representative casing comes from first encountered clip. Append Desktop last iff any !HasGame; Label=Library_Desktop, FilterKey literal `"\u0001desktop"` (leading U+0001). Drop stale active game filter if no matching game remains, or stale Desktop filter if no desktop remains. No All chip. Clicking same filter case-insensitively clears it to null; set every chip's IsActive by OrdinalIgnoreCase key comparison. (`ViewModels/LibraryViewModel.cs:55–94`.)

SearchText or FavoritesOnly changes clear all selection and regroup synchronously, without debounce. SetGameFilter also clears selection/regroups. Search null→empty, Trim; if nonempty match substring of DisplayName **or raw Game** OrdinalIgnoreCase (not Exe, extension, folder path, timestamp, or translated Desktop label). Filters combine with AND: favorites, category, then search. Raw whitespace Game is considered Desktop by HasGame but could still participate in search by its raw string. (`ViewModels/LibraryViewModel.cs:52–53`, `ViewModels/LibraryViewModel.cs:59–63`, `ViewModels/LibraryViewModel.cs:97–111`, `Models/ReplayClip.cs:28`.)

Group filtered list by CreatedDate.Date, OrderByDescending day, retain filtered source order within day. DateTime.Today is evaluated on regroup only. Today/Yesterday labels localized, subtitle `d MMM`; other days heading `d MMMM`, subtitle `dddd`. Uppercase heading with CurrentCulture; group count Library_GroupCount(bucket count). HasNoResults=Clips.Count>0 && Groups.Count==0. ClipCountLabel is Library_ClipCount(**all** Clips.Count), not filtered count. RebuildView rebuilds chips then groups and notifies ClipCountLabel. No midnight timer or explicit locale-change regroup. (`ViewModels/LibraryViewModel.cs:49–50`, `ViewModels/LibraryViewModel.cs:113–156`.)

ToggleFavorite changes IsFavorite immediately, writes sidecar favorite, then **regroups only**, so unfavoriting under FavoritesOnly removes the card; it does not clear selection, rebuild chips, or notify selection-derived properties. (`ViewModels/LibraryViewModel.cs:685–692`.)

HasSelection scans **all** Clips; IsNotSelecting is its inverse; DeleteSelectedLabel=`Localizer.Get("Library_Delete") + " (" + selectedCount + ")"`. NotifySelectionChanged raises these three properties. The view-model does not subscribe to every clip's IsSelected property; callers must invoke notification through supplied selection commands. (`ViewModels/LibraryViewModel.cs:728–745`.)

Pointer rules: right-button press returns without changing selection/focus; non-left returns. Left press: Shift takes precedence over Ctrl; Shift→SelectRange; Ctrl→ToggleSelectClip; otherwise if HasSelection→toggle; otherwise PlayClip. Focus card afterward. Circle click toggles regardless of modifier and focuses itself. Empty body press walks visual ancestors; if any Border has class clipcardr, do nothing; otherwise clear selection (no left-button restriction). (`Views/LibraryView.axaml.cs:58–95`.)

Toggle flips selected; updates anchor **only when toggled on**, so toggling anchor off leaves its anchor reference. Range uses flattened Groups.Items display order. If target not visible no-op; if no visible anchor, add target without clearing others and make it anchor. Else set visible cards selected iff between inclusive anchor/target indices; do not move anchor. Range loop does not clear any selected clip outside visible list. Ctrl+A selects all visible cards but does not clear hidden selections or set anchor. ClearSelection clears every Clips item and anchor. DeleteSelected snapshots all selected clips (including hidden ones), deletes each sequentially, then nulls anchor. Filters normally clear selection; unfavoriting is an exception. (`ViewModels/LibraryViewModel.cs:747–799`.)

### File operations and drive/quota state

DeleteClip(null) no-op; otherwise synchronously: if source exists File.Delete(source); if ThumbnailPath exists File.Delete(thumbnail); metadata Delete(cache+legacy); Clips.Remove; RebuildView. All in one try; any exception logs `Failed to delete clip: ...` and skips remaining steps. NotifySelectionChanged still runs after catch. No confirmation, recycle bin, undo, player-stop coordination, free-space refresh, LibraryStats notification, signature update, or cache-bitmap disposal. For screenshots ThumbnailPath==FilePath, second existence check normally finds nothing. If source absent, removal/cache cleanup still proceeds. (`ViewModels/LibraryViewModel.cs:700–721`.)

ShowInFolder requires non-null clip **and existing source**, then Process.Start(`explorer.exe`, `/select,"{FilePath}"`); catches/logs errors. OpenLibraryFolder uses ProcessStartInfo(FileName=Settings.LibraryPath, UseShellExecute=true), without a preceding exists check/create; catches/logs. No rename/move/copy/import/export file command is provided here. (`ViewModels/LibraryViewModel.cs:801–837`.)

UpdateFreeSpace resolves Path.GetPathRoot(Path.GetFullPath(Settings.LibraryPath)); empty root→return. DriveInfo.AvailableFreeSpace/2^30→freeGb, TotalSize/2^30→totalGb, used=total−free. FreeSpaceDisplay is `F0 GB` if freeGb≥100, else `F1 GB`; UsedSpacePercent=`100*(TotalSize-AvailableFreeSpace)/TotalSize`; UsedSpaceDisplay=`usedGb:F1 + " / " + totalGb:F0 + " GB"`; width=`clamp(percent,0,100)/100*120`. Numeric interpolation follows current culture. Failure leaves previous values and logs; no periodic free-space timer. Refresh finally explicitly notifies LibraryStats=`Library_Stats(Clips.Count,FreeSpaceDisplay)`; startup UpdateFreeSpace alone does not notify that derived string. (`ViewModels/LibraryViewModel.cs:234–275`, `ViewModels/LibraryViewModel.cs:461–469`.)

Quota does nothing unless Settings.AutoCleanupEnabled; maxBytes=`(long)SelectedStorageLimit.Gb*1024*1024*1024`; ≤0→return; missing library→return. Enumerate only VideoPatterns in top-level folder, create FileInfo objects, sort CreationTime ascending, sum Length. If total>limit: oldest first, until total≤limit; per item capture size, permanently delete media, delete metadata, delete hashed thumbnail if present, subtract captured size, log integer MB (`size/1_048_576`). No protection for favorites/active playback; screenshots excluded from both sum and deletion. Per-file failures log/continue, outer failure logs. If thumbnail deletion fails after source deletion, total is not decremented and cleanup can delete additional files. Runs on construction and phase-2 housekeeping, not every unchanged no-pending refresh. (`ViewModels/LibraryViewModel.cs:278–344`, `ViewModels/LibraryViewModel.cs:436–445`, `ViewModels/LibraryViewModel.cs:487–493`.)

### Player state, threading, and lifecycle

Defaults: Title=`Player`; _libVLC/_mediaPlayer/Player=null; CurrentClip/ActiveSidebarClip/StillImage=null; IsViewingImage/IsPlaying/IsClipLoading/IsMuted=false; Position/PositionSeconds/DurationSeconds=0; TimeDisplay=`0:00 / 0:00`; Volume=80; PlaybackSpeed=1.0. VideoRenderer instance is created with the view-model, but engine/control worker/position timer are lazy. Clips aliases Library.Clips (same objects, **not Groups/filter result**); Library property exposes the shared VM. (`ViewModels/PlayerViewModel.cs:26–169`, `ViewModels/PlayerViewModel.cs:250–263`.)

Constructor subscribes only to Clips.CollectionChanged for sidebar stats. Handler always posts UI notifications for TotalClips, TotalDurationDisplay, TotalSizeDisplay. It does **not** subscribe to individual Duration changes; duration hydration can leave total-duration footer stale until a collection change/rebinding. TotalClips=Clips.Count including screenshots. TotalSize: ≥2^30 bytes→F1 GB; otherwise F0 MB (even tiny files become 0 MB). TotalDuration sums TimeSpan ticks: ≥1 hour→Time_HourMinShort((int)TotalHours,Minutes), ≥1 minute→Time_MinShort((int)TotalMinutes), else Time_SecShort(Seconds); discards smaller units in each case. (`ViewModels/PlayerViewModel.cs:53–69`, `ViewModels/PlayerViewModel.cs:255–285`.)

EnsureVlc runs at beginning of LoadAndPlay, returns if _libVLC already non-null, catches initialization failures and logs. Active implementation synchronously Core.Initialize, creates its **own** LibVLC with `--no-video-title-show`, `--no-osd`, creates MediaPlayer, attaches renderer before playback, exposes Player, starts background thread `Lag.VlcControl`, creates stopped System.Timers.Timer interval 100 ms and wires events. It does not use VlcEngine. If library/engine creation succeeds only partly, retry guard is still _libVLC!=null; partial failures may prevent full recovery. Screenshot loading also invokes EnsureVlc first and returns if engine/player unavailable. (`ViewModels/PlayerViewModel.cs:288–363`, `ViewModels/PlayerViewModel.cs:368–371`.)

BlockingCollection<Action> serializes all queued **control** calls on one background worker. For each op catch/log `[Player] VLC op failed: ...`, continue; outer catch swallows shutdown/completed-queue exceptions. Enqueue no-ops once disposed and catches queue.Add failures. Creation and periodic native getters are exceptions to the “every call on worker” comment: creation occurs in EnsureVlc and snapshots are read from the timer thread. No UI-thread seek/stop/media swap should block the Slint port. (`ViewModels/PlayerViewModel.cs:31–36`, `ViewModels/PlayerViewModel.cs:219–239`, `ViewModels/PlayerViewModel.cs:299–317`, `ViewModels/PlayerViewModel.cs:549–556`.)

User Position change ignores internal-update guard; stores latest float clamp(value,0,1) under pending lock, RequestFlush. Volume change stores clamp(value,0,100); IsMuted stores latest bool. These callbacks clamp the **native pending write**, not the bound property itself. RequestFlush ignores disposed; only one queued flush at a time. Flush loops: atomically take-and-clear pending volume, mute, position; if all absent or player null, clear queued flag and return. Apply volume then mute then seek (seek only if IsSeekable), catching each native call separately; repeat until drained. Intermediate values overwritten while worker is busy are dropped. No fixed throttle interval; at most one flush/seek operation executing at a time. (`ViewModels/PlayerViewModel.cs:124–156`, `ViewModels/PlayerViewModel.cs:180–217`.)

SpeedOptions exactly `[2.0,1.5,1.0,0.5,0.25,0.1]`, ordered fast→slow. SpeedLabel exact 1→`1x`, otherwise current-culture `0.##` + `x`. Speed change notifies SpeedLabel and queues ApplyRate(value). ApplyRate only calls SetRate(float), swallowing errors; no clamping, audio disable/selection, reinit, or deep-slow-motion special path. References to 0.05x, automatic 1x during seek, and “drop audio” are stale comments; no 0.05 preset and OnPositionChanged does not modify speed. (`ViewModels/PlayerViewModel.cs:163–178`, `ViewModels/PlayerViewModel.cs:213–215`, `ViewModels/PlayerViewModel.cs:241–248`, `ViewModels/PlayerViewModel.cs:333–335`.)

LoadAndPlay name is misleading: it prepares media; **does not enqueue Play for videos**. After EnsureVlc succeeds, reset speed=1, set CurrentClip, set ActiveSidebarClip while `_suppressSidebarAutoPlay=true`, then clear suppression. A user ActiveSidebarClip change non-null and not suppressed invokes LoadAndPlay then StartPlayback. PlayClip command does the same. MainViewModel's library play handler calls LoadAndPlay then switches CurrentView to Player; PlayerView attach performs StartPlayback. (`ViewModels/PlayerViewModel.cs:74–81`, `ViewModels/PlayerViewModel.cs:368–382`, `ViewModels/PlayerViewModel.cs:453–462`, `ViewModels/MainViewModel.cs:277–281`, `Views/PlayerView.axaml.cs:128–145`.)

Screenshot branch queues StopPlayback, synchronously opens original and decodes full-resolution Bitmap, swaps StillImage and defers old image disposal. Failure logs and can leave old still image visible. Then IsViewingImage=true, IsPlaying=false, IsClipLoading=false, TimeDisplay empty; returns. It does not reset Position/PositionSeconds/DurationSeconds synchronously. StartPlayback and PlayPause no-op in image mode; SeekForward/Backward lack this guard (keyboard can queue time changes on retained media), mute can still toggle via keyboard. Center-play visibility remains !IsPlaying. Switching to video clears IsViewingImage/StillImage and defers old still disposal. (`ViewModels/PlayerViewModel.cs:384–414`, `ViewModels/PlayerViewModel.cs:492–502`, `ViewModels/PlayerViewModel.cs:531–542`, `Views/PlayerView.axaml:241`.)

Video branch: detect `.gif` case-insensitively, IsClipLoading=true; queue operation reads live player/lib instance, normalizes native rate 1.0, creates Media(new Uri(path)); GIF adds `:demux=avformat`, `:input-repeat=65535`; **every** media adds `:avcodec-hw=none`. Save old Media, assign new Media, dispose local new wrapper (player retains native ref), dispose old wrapper. No explicit Stop before assignment; no preemptive cancellation of queued older clip loads; no file-exists guard; load sets no initial PositionSeconds/DurationSeconds or cleared renderer bitmap. View starts playback separately when attached; sidebar starts immediately. (`ViewModels/PlayerViewModel.cs:416–454`.)

### Playback events and transport state

| Trigger | UI state / effect | Source |
|---|---|---|
| Renderer.FrameRendered | If IsClipLoading, set false; callback already UI-dispatched | `ViewModels/PlayerViewModel.cs:319–324` |
| Native Playing | Post UI: IsPlaying=true, start 100 ms timer; set pendingVolume=Volume and flush; enqueue reapply current PlaybackSpeed. Does **not** explicitly reapply IsMuted or clear loading. | `ViewModels/PlayerViewModel.cs:326–336` |
| Native Paused | Post UI: IsPlaying=false, stop position timer | `ViewModels/PlayerViewModel.cs:337` |
| Native Stopped | Post UI: IsPlaying=false, IsClipLoading=false, stop timer; guarded Position=0; TimeDisplay=`0:00.000 / 0:00.000`. PositionSeconds/DurationSeconds are not reset. | `ViewModels/PlayerViewModel.cs:338–347` |
| Native EndReached | Post UI: IsPlaying=false, stop timer, guarded Position=1. No auto-next, no explicit loading reset, no final PositionSeconds/TimeDisplay update. | `ViewModels/PlayerViewModel.cs:348–357` |
| PlayPause | Null player/image→no-op; queue: if Ended, Stop then Play (restart); else if native IsPlaying, Pause; else Play | `ViewModels/PlayerViewModel.cs:498–513` |
| StartPlayback | Image→no-op; otherwise enqueue player.Play; safe null-conditional | `ViewModels/PlayerViewModel.cs:489–495` |
| Stop / StopPlayback | Enqueue player.Stop; UI resets arrive from Stopped | `ViewModels/PlayerViewModel.cs:516–528` |
| SeekForward | Enqueue native Time +=10,000 ms, catch; no explicit upper clamp or IsSeekable/image guard | `ViewModels/PlayerViewModel.cs:531–535` |
| SeekBackward | Enqueue native Time=max(0,Time−10,000 ms), catch; no IsSeekable/image guard | `ViewModels/PlayerViewModel.cs:538–542` |
| PlayNext | If collection empty return; currentIndex=Clips.IndexOf(CurrentClip) or −1; steps 1..Count modulo Count; play first !IsImage; all images→no-op | `ViewModels/PlayerViewModel.cs:471–487` |

Next uses unfiltered master order, wraps, includes GIFs as video, can replay the same clip if it is the only video, starts first video when current absent/stale, and is never triggered automatically at natural end. (`ViewModels/PlayerViewModel.cs:471–487`, `ViewModels/PlayerViewModel.cs:348–357`.)

100 ms position timer snapshots native Position, Time(ms), Length(ms) on timer thread then UI-posts: guarded Position; PositionSeconds=current.TotalSeconds; DurationSeconds=total.TotalSeconds; TimeDisplay=`FormatTime(current) / FormatTime(total)`. FormatTime includes milliseconds: `h:mm:ss.fff` if TotalHours≥1 else `m:ss.fff`. No getter exception handler or generation/current-clip identity guard surrounds this snapshot. It runs only while native Playing state has started it; paused seek is not actively polled. (`ViewModels/PlayerViewModel.cs:315–317`, `ViewModels/PlayerViewModel.cs:326–357`, `ViewModels/PlayerViewModel.cs:545–571`.)

Visible TimeText is **not bound to TimeDisplay**. View uses DispatcherTimer 33 ms (about 30.3 Hz, despite “display-rate” comment), initialized on attach from PositionSeconds and UtcNow; resyncs when PositionSeconds or IsPlaying changes. Tick: `t=lastTimeSec`; if IsPlaying add `(UtcNow-lastTimeAt).TotalSeconds * max(0.01,PlaybackSpeed)`; if total>0 clamp t to [0,total]; format nonnegative values as above, assign TimeText. Speed change alone does not reset interpolation anchor; paused snapshots/stop may leave stale visible times because only Position and TimeDisplay were reset. This timer is separate from the compositor and video decode fps. (`Views/PlayerView.axaml:254–256`, `Views/PlayerView.axaml.cs:23–27`, `Views/PlayerView.axaml.cs:143–145`, `Views/PlayerView.axaml.cs:214–241`.)

Attach subscribes BitmapChanged/FrameRendered/PropertyChanged, sets source, starts playback, starts clock; subscribes speed flyout and main fullscreen state, applies layout, focuses. On renderer events source is re-read since bitmap instance rotates. StillImage/IsViewingImage property changes also reselect source. Detach stops cursor/time/mute/pop timers, unsubscribes flyout and both VMs, queues StopPlayback, clears VideoImage.Source, nulls VM references. It does not dispose singleton VM/renderer or reset `_controlsPinned`/flash opacity. (`Views/PlayerView.axaml.cs:128–224`.)

Dispose is idempotent: _disposed=true, unsubscribe collection changes, stop/dispose native position timer; bypass Enqueue guard to add final Stop, CompleteAdding, join worker at most **2000 ms**, dispose queue, MediaPlayer, LibVLC, then VideoRenderer. No final StillImage disposal is present. Native calls still blocked past join timeout are a teardown hazard, not a desired UX. (`ViewModels/PlayerViewModel.cs:573–591`.)

### Video renderer and lifetime invariants

Attach stores player and roots all callback delegates for their entire native lifetime; SetVideoFormatCallbacks(format,cleanup), SetVideoCallbacks(lock,null,display), before Play. Format callback requests fourcc `RV32` (32-bit BGRA), source coded width/height unchanged; pitch=`(width*4+31)&~31u` (32-byte row alignment). Query player.Size(0) for visible width/height; if either zero, throws, or exceeds coded dimensions, use coded dimensions. No separate x/y crop offset or sample-aspect-ratio correction is implemented in this bitmap renderer. (`Services/VlcVideoRenderer.cs:30–46`, `Services/VlcVideoRenderer.cs:61–86`.)

Under _sync: free prior HGlobal, save coded/visible dimensions/pitch, allocate pitch*codedHeight, RtlZeroMemory entire fresh native buffer to prevent garbage/gray flash. Return pitches=pitch, lines=codedHeight, buffer-count=1. Post UI recreation: if disposed or current visible dimensions no longer equal captured ones, abort; defer disposal of each old bitmap; allocate three WriteableBitmaps at visible PixelSize, DPI 96,96, Bgra8888/Opaque; slots write=0,ready=1,display=2,hasReady=false; raise BitmapChanged outside lock. (`Services/VlcVideoRenderer.cs:88–127`.)

Video lock callback writes native buffer pointer to planes under _sync, returns null picture ID; cleanup frees native buffer under same lock. Display callback locks _sync; reject disposed/null target/null native buffer or target-size mismatch. Lock write bitmap: if destination RowBytes==native pitch **and** visibleWidth==codedWidth, copy pitch*visibleHeight contiguously; else for y<visibleHeight copy min(destination.RowBytes,visibleWidth*4) bytes from native y*pitch to destination y*RowBytes. This copies only the upper-left visible rectangle, excluding coded right/bottom macroblock padding (example comment: 2580×1080 visible versus 2592×1088 coded). Swap write↔ready and set hasReady. (`Services/VlcVideoRenderer.cs:23–26`, `Services/VlcVideoRenderer.cs:130–179`.)

Coalesce presentation using Interlocked.CompareExchange pending flag 0→1. At most one pending UI Render-priority post at a time; post first resets pending flag, then locks, aborts if disposed, swaps display↔ready if hasReady, clears hasReady; raises FrameRendered. Multiple decode frames before a UI present overwrite ready with newest complete frame; no unbounded frame queue. Bitmap getter returns display slot under lock; view must re-read every frame, not merely invalidate old source. Never intentionally write the currently designated display bitmap. (`Services/VlcVideoRenderer.cs:48–59`, `Services/VlcVideoRenderer.cs:176–197`, `Views/PlayerView.axaml.cs:197–201`.)

Dispose locks/idempotently marks disposed, frees HGlobal, defers all three bitmap disposals and nulls slots. DeferredDispose.Later(null) no-op; default delay **3000 ms**; arm a one-shot DispatcherTimer on UI thread (direct if already UI, otherwise Post), callback catches all Dispose exceptions. It is an elapsed-time mitigation, not a compositor completion fence. Rust must ensure textures/buffers remain alive until the renderer finishes using them; it need not imitate unsafe managed resource races. (`Services/VlcVideoRenderer.cs:201–222`, `Core/DeferredDispose.cs:6–19`.)

### Render cadence / fps

DynamicRenderTimer is a background render/composition timer, not capture fps, playback rate, the 100 ms position timer, or the 33 ms label timer. Constructor clamps requested Hz to **30..360**; Hz setter clamps similarly and logs only changes. SetMonitor atomically stores HMONITOR as long. AddTick adds under lock and lazily starts a single background AboveNormal thread named `Lag render timer`; RemoveTick removes under lock. Thread runs forever, even with no subscribers; no shutdown/disposal path. RunsInBackground facade returns true; DispatchProxy forwards add_Tick/remove_Tick/get_RunsInBackground, default null. (`Services/DynamicRenderTimer.cs:20–78`, `Services/DynamicRenderTimer.cs:167–187`.)

Loop Stopwatch starts once; bound monitor initial −1, desired initially 0. On desired-monitor change dispose previous DXGI Output, resolve new output, set bound monitor, reset next clock deadline, log vblank or fallback. Resolve: zero monitor→null; create Factory1, enumerate all adapters/GetOutput until exception ends each output loop, retain matching Description.MonitorHandle, dispose unmatched outputs/adapters/factory. COM refcount keeps returned output usable. Lookup failure logs then null. (`Services/DynamicRenderTimer.cs:81–101`, `Services/DynamicRenderTimer.cs:137–163`.)

With output: blocking WaitForVerticalBlank; success invokes tick(Stopwatch.Elapsed), continues loop; Hz does not cap vblank cadence. Failure logs, disposes output, sets null/resets next deadline and uses clock fallback **until monitor handle changes** (same-monitor Hz changes do not retry DXGI). No tick exception handler protects subscriber invocation. (`Services/DynamicRenderTimer.cs:103–133`.)

Fallback each loop: interval=Stopwatch.Frequency/_hz; next+=interval; if now−next>4*interval reset next=now; sleep floor integer milliseconds of remaining time if >0, then tick(elapsed). No spin to deadline or frame-count stepping. Program holds timeBeginPeriod(1) for approximate millisecond sleep granularity. (`Services/DynamicRenderTimer.cs:126–133`, `Program.cs:18`, `Program.cs:68`.)

At Avalonia AfterSetup, Program chooses fastest enumerated display refresh, clamped **60..240** initially (default/failure floor 60); creates timer and reflectively binds its IRenderTimer facade through AvaloniaLocator. Binding failure logs/stays at stock 60 Hz. MainWindow retunes at Opened, PositionChanged, and posted WindowState/IsVisible changes: hidden/minimized→monitor 0, Hz=60 to retain smooth HUD/toasts without GPU vblank wait; visible with valid HWND→Hz=reported refresh if >0, monitor=nearest window monitor. The runtime timer's 30..360 clamp applies to retuned values. Monitor lookup uses MonitorFromWindow(DEFAULTTONEAREST), GetMonitorInfo, EnumDisplaySettings current mode; no recording-fps setting drives this. (`Program.cs:191–213`, `Program.cs:230–269`, `Views/MainWindow.axaml.cs:19–30`, `Views/MainWindow.axaml.cs:99–126`, `Services/HardwareDetector.cs:132–165`.)

### VlcEngine.cs — unfinished intent, not active behavior

WIP static service proposes one process-lifetime LibVLC shared by player/editor, initialized off UI thread and optionally prewarmed on window/library opening to avoid plugin-probing startup freezes. Gate protects Task<LibVLC>; IsReady means IsCompletedSuccessfully; Current returns ready result else null without starting/waiting; Prewarm fire-and-forgets GetAsync; GetAsync creates or retries null/faulted/canceled task with Task.Factory.StartNew(Create,None,LongRunning,Default). Create times Core.Initialize/new LibVLC with same two global options, logs success/failure, rethrows failure. Nobody should dispose the proposed shared instance. (`Services/VlcEngine.cs:9–62`.)

**There are no callers of VlcEngine in the inspected application entry files/view-model/view/service/core sources.** PlayerViewModel still owns/creates/disposes its own LibVLC synchronously. Do not require prewarm events or shared engine as current user-visible parity. For Rust, asynchronous initialization without UI freeze is a reasonable implementation improvement, but label it as such; no original ready-state UI or prewarm trigger is wired. (`ViewModels/PlayerViewModel.cs:293–313`, `ViewModels/PlayerViewModel.cs:589–590`; WIP intended call description `Services/VlcEngine.cs:15–18`.)

## 5. **Persistence & formats**

| File/location | Read/write/delete contract | Source |
|---|---|---|
| Settings.LibraryPath (default Windows MyVideos special folder + `Lag`) | Scan all eight patterns at top level; create missing directory during scan; read file size, creation/local write/UTC write times; permanently delete on manual/quota commands; playback reads original file | `ViewModels/SettingsViewModel.cs:1501`, `ViewModels/SettingsViewModel.cs:1920`, `ViewModels/LibraryViewModel.cs:184–204`, `ViewModels/LibraryViewModel.cs:384–425`, `ViewModels/LibraryViewModel.cs:700–710` |
| `%LocalAppData%\Lag\thumbnails\{HASH}.jpg` | Directory created in hydration; video snapshot JPEG; mtime invalidation as above; removed on manual/quota delete; no central index/size eviction | `ViewModels/LibraryViewModel.cs:206–212`, `ViewModels/LibraryViewModel.cs:490`, `ViewModels/LibraryViewModel.cs:579–589`, `ViewModels/LibraryViewModel.cs:660–664` |
| `{LibraryPath}\.thumbnails\` legacy directory | If exists, recursively remove during hydration; failure debug-only; thumbnails regenerate in new location | `ViewModels/LibraryViewModel.cs:214–229` |
| Original `.png/.jpg/.jpeg` | Screenshot is its own ThumbnailPath; downscale decode to width 640 for grid; full-resolution decode for player; no converted cache image | `ViewModels/LibraryViewModel.cs:412–415`, `ViewModels/LibraryViewModel.cs:540–547`, `ViewModels/PlayerViewModel.cs:390–394` |
| `%LocalAppData%\Lag\metadata\{HASH}.json` | Metadata read/write/delete, preferred over legacy; lazily create directory on Write | `Services/ClipMetadata.cs:40–50`, `Services/ClipMetadata.cs:58–109` |
| `{clipPath}.json` legacy sidecar | Full filename including media extension + `.json`, e.g. `Replay.mp4.json`; migrate on read, remove after successful cached write; delete alongside cached metadata | `Services/ClipMetadata.cs:53–86`, `Services/ClipMetadata.cs:105–109` |

Both persistent hashes are the same uppercase 32-character MD5 hex of UTF-8 bytes of **lower-invariant supplied clip path**, no content hash, salt, timestamp, version, separator normalization, or original-path field in JSON. Extension distinguishes metadata `.json` from thumbnail `.jpg`, and directories differ. Windows LocalApplicationData/MyVideos special-folder resolution must be used rather than hardcoding a specific username. No actual hidden-file attribute is set by these methods; “hidden cache” means location outside user's visible library folder. (`Services/ClipMetadata.cs:42–54`, `ViewModels/LibraryViewModel.cs:210–212`, `ViewModels/LibraryViewModel.cs:660–664`.)

Metadata schema uses System.Text.Json default **case-sensitive PascalCase property names**, nullable strings/double and booleans; WriteIndented=true, no naming policy/default-value/null omission options. Representative newly created full document:

```json
{
  "Game": null,
  "Exe": null,
  "Favorite": false,
  "Edited": false,
  "DurationSeconds": null
}
```

Property order follows declaration order above. Game is friendly app/game label; Exe is captured process executable for icon lookup (no icon lookup is actually used in these cards); Favorite is star; Edited editor-produced/overwritten marker; DurationSeconds double is seconds (fractional permitted), null until probed. Missing properties default null/false; unknown properties are ignored by default serializer behavior; wrong types/malformed JSON cause Read to return null. No schema version or migration other than location migration, no stored CreatedDate/FileSize/path/thumbnail/selection/playback-position/volume/speed. (`Services/ClipMetadata.cs:14–40`, `Services/ClipMetadata.cs:62–75`; display usage `ViewModels/LibraryViewModel.cs:405–424`.)

Read: if cached file exists, read/deserialize it and return immediately (including JSON `null` result); a malformed/unreadable cache is caught and returns null **without trying legacy**. Only if no cached file exists, read legacy; non-null metadata→Write(new cache) and return metadata. Write creates cache directory, overwrites whole JSON via File.WriteAllText, then best-effort removes legacy; failures debug-log. No temp-file/atomic replacement, cross-process lock, file-content validation, or transaction. (`Services/ClipMetadata.cs:58–88`.)

Update reads existing metadata or new default object, invokes supplied mutation, then writes. SetDuration stores TimeSpan.TotalSeconds; SetFavorite and SetEdited mutate one field. Delete separately swallows errors for cache and legacy deletions. Although the store comment says “never throws,” Update's arbitrary mutation runs outside a try and can throw; fixed setter mutations here do not deliberately throw. Concurrent read-modify-write updates can overwrite one another. Moving/renaming media changes hash and loses association unless another area explicitly migrates metadata; the obsolete model comment promising move-survival is false for the active cache. (`Services/ClipMetadata.cs:7–12`, `Services/ClipMetadata.cs:91–109`.)

Decoded cache, filters, selection/anchor, folder signature, sidebar selection/current clip, player volume/mute/speed/position, fullscreen overlay state, and timer state are in-memory only in this area. No settings persistence of those player choices occurs here. Engine and editor may write the same metadata schema through cross-area interfaces below. (`ViewModels/LibraryViewModel.cs:28–47`, `ViewModels/LibraryViewModel.cs:359`, `ViewModels/LibraryViewModel.cs:621–652`, `ViewModels/PlayerViewModel.cs:74–189`.)

## 6. **Native/external dependencies**

### Exact VLC option inventory and libmpv mapping

These are **all** explicit VLC startup/media options in the requested files. Colons denote VLC per-media options, not part of mpv option names. Rust equivalents below are port design suggestions; verify against the bundled libmpv version. The source behavior is normative even where backend event models differ.

| C# option / site | Existing intent and actual scope | Suggested libmpv setting |
|---|---|---|
| `--no-video-title-show` (`ViewModels/PlayerViewModel.cs:302`; WIP `Services/VlcEngine.cs:55`) | Suppress VLC title over video | `osd-playing-msg=""` |
| `--no-osd` (same sites) | App draws all controls/feedback | `osd-level=0`; `osc=no` for app-owned controls |
| `:demux=avformat` (`ViewModels/PlayerViewModel.cs:440`) | GIF only: animate all frames rather than VLC image demuxer's first frame | GIF load with `demuxer=lavf` |
| `:input-repeat=65535` (`ViewModels/PlayerViewModel.cs:441`) | GIF only: finite 65,535 input repeats | GIF `loop-file=65535`; ordinary media `loop-file=no` |
| `:avcodec-hw=none` (`ViewModels/PlayerViewModel.cs:446`) | All video media: software decode to avoid d3d11va audio stalls/green swap | `hwdec=no` |

The option mappings are inferred from [official mpv option definitions](https://github.com/mpv-player/mpv/blob/master/DOCS/man/options.rst). [mpv's loop documentation](https://mpv.io/manual/stable/#options-loop-file) counts repetitions as seeks back to start, excluding initial playthrough. Preserve finite repetition rather than silently substituting infinite loop. The old comments claiming hardware enabled per-media do not describe the actual `none` option. (`ViewModels/PlayerViewModel.cs:300–302`, `ViewModels/PlayerViewModel.cs:443–446`, `Services/VlcEngine.cs:53–55`.)

There is no specified VLC file/network cache size, `--vout`, `--aout`, `--avcodec-threads`, subtitles policy, pitch policy, forced fps, deinterlace flag, audio language, audio track ID, or hardware-acceleration selection setting in the requested files. Defaults come from the engine; do not misrepresent imagined options as source parity. (`ViewModels/PlayerViewModel.cs:299–307`, `ViewModels/PlayerViewModel.cs:435–451`, `Services/VlcEngine.cs:52–57`.)

### Native API behavior to reproduce

| C# dependency / call | Purpose and Rust boundary |
|---|---|
| LibVLCSharp Core.Initialize; LibVLC; MediaPlayer; Media(Uri) | Active player owns lazy instance/player/media; Rust owns libmpv handle/render context and media lifetime; async worker/event dispatch must keep UI responsive. (`ViewModels/PlayerViewModel.cs:293–317`, `ViewModels/PlayerViewModel.cs:435–451`.) |
| Play/Pause/Stop, VLCState.Ended | Suggested mpv: `loadfile`, `pause`, `stop`; explicitly reload or seek/restart at EOF and preserve current clip UI. (`ViewModels/PlayerViewModel.cs:492–528`.) |
| Position 0..1, Time/Length milliseconds, IsSeekable | Suggested mpv: `percent-pos/100`, `time-pos`, `duration` seconds, `seekable`; slider→absolute-percent seek, buttons→±10-second relative seek. Keep coalescing. (`ViewModels/PlayerViewModel.cs:124–129`, `ViewModels/PlayerViewModel.cs:215`, `ViewModels/PlayerViewModel.cs:531–566`.) |
| Volume 0..100; Mute bool; SetRate(float) | Suggested mpv: `volume`, `mute`, `speed`. Preserve app's 100 ceiling and independent mute. (`ViewModels/PlayerViewModel.cs:138–177`, `ViewModels/PlayerViewModel.cs:211–215`, `ViewModels/PlayerViewModel.cs:245–248`.) |
| Playing/Paused/Stopped/EndReached + FrameRendered | Map explicit app state using backend events/properties; EOF is distinct from explicit stop, and a media-ready event is distinct from first presented frame. (`ViewModels/PlayerViewModel.cs:319–357`.) |
| No track calls in Player | Keep default audio-track choice; no player selector, no track reinitialization for slow motion. mpv track APIs belong only to a future/editor interface. (`ViewModels/PlayerViewModel.cs:241–248`, `Views/PlayerView.axaml:264–347`.) |

The suggested commands/properties are supported by the [official mpv command/property interface](https://github.com/mpv-player/mpv/blob/master/DOCS/man/input.rst). Slint must own the listed keyboard/pointer bindings; do not expose mpv's unrelated default shortcuts, built-in controller, or an independent native video window. Backend `keep-open`/EOF configuration is an implementation choice: reproduce explicit end/restart behavior and chosen frame retention without converting EOF into automatic Next. (`Views/PlayerView.axaml:12–17`, `Views/PlayerView.axaml:174–192`, `ViewModels/PlayerViewModel.cs:348–357`, `ViewModels/PlayerViewModel.cs:508–512`.)

**Renderer suggestion:** use libmpv render API integrated with Slint's rendering lifecycle; software `sw` rendering is the closest model to current bitmaps, with bgr0 output converted/marked opaque for the UI. Set software size/format/stride/pointer, keep buffers alive through use, and coalesce update callbacks into UI/render requests. GPU render integration is also possible if overlays, visible geometry, input, frame lifetime, and cadence remain equivalent. mpv's render API controls its own callback/thread/context rules; do not transplant VLC lock/display callbacks verbatim. ([Official render API header](https://github.com/mpv-player/mpv/blob/master/include/mpv/render.h).) Source requirement remains ordinary UI composition, visible-region correctness, and no native-host overlay/input problems. (`Services/VlcVideoRenderer.cs:12–26`, `Services/VlcVideoRenderer.cs:75–127`, `Services/VlcVideoRenderer.cs:148–197`.)

Do not equate `Playing`/`file-loaded` with first presented video frame. A new backend generation identifier can prevent old-media events or renderer callbacks from updating a newer clip; this is a safe robustness improvement when visible behavior stays unchanged. Original renderer only checks current dimensions on format post and Player event handlers have no generation tags. (`Services/VlcVideoRenderer.cs:107–125`, `ViewModels/PlayerViewModel.cs:319–357`.)

### FFmpeg, OS, and UI dependencies

- FFMpegCore 5.4.0 uses FFProbe.AnalyseAsync for Duration and FFMpeg.SnapshotAsync for preview. Use the same bundled FFmpeg 7.1 CLI/DLL set for deterministic format support. App sets process current directory to `<AppContext.BaseDirectory>\ffmpeg`, SetDllDirectory and AddDllDirectory, prepends that directory plus `;` to PATH, configures FFOptions.BinaryFolder there, and initializes native FFmpeg interop. No area-specific LAG_* binary-path override exists. (`InstantReplay.csproj:35–38`, `InstantReplay.csproj:66`, `App.axaml.cs:34–50`, `ViewModels/LibraryViewModel.cs:564–582`.)
- A schematic equivalent thumbnail command is `ffmpeg -ss <00:00:00 or 00:00:02> -i <clip> -map 0:<selected-video-stream-index> -c:v mjpeg -vframes 1 -s 320x180 -y <hashed-cache>.jpg`. This expresses the verified wrapper builder's semantics, **not byte-for-byte generated argument spelling/order**; when already 320×180 the builder omits resize. GIF thumbnails are JPEG stills; GIF playback remains animated. (`ViewModels/LibraryViewModel.cs:581–582`; [pinned builder lines 29–50 and 74–101](https://github.com/rosenbjerg/FFMpegCore/blob/ed8f899d04a1f71c481e02b2123176d5cd097a4a/FFMpegCore/FFMpeg/SnapshotArgumentBuilder.cs#L29).)
- Windows shell: `explorer.exe /select,"path"` and shell execution of folder path; use std::process::Command with equivalent quoting/argument semantics or windows-rs ShellExecuteW. File deletion is permanent standard filesystem deletion, no IFileOperation/recycle-bin behavior. (`ViewModels/LibraryViewModel.cs:706–710`, `ViewModels/LibraryViewModel.cs:811`, `ViewModels/LibraryViewModel.cs:827–832`.)
- DriveInfo corresponds to Windows volume/free-space query (suggest windows-rs GetDiskFreeSpaceExW with available-to-caller bytes); resolve LocalAppData/Videos known folders using Windows known-folder APIs. Preserve binary byte units despite labels GB/MB. (`ViewModels/LibraryViewModel.cs:261–270`, `Services/ClipMetadata.cs:42–43`, `ViewModels/SettingsViewModel.cs:1501`.)
- MD5.HashData + UTF8 + invariant lower + uppercase hex: use deterministic Rust hashing/encoding with Windows/.NET-compatible path case handling; do not replace hash algorithm just because these are caches (compatibility with existing metadata is required). (`Services/ClipMetadata.cs:46–50`, `ViewModels/LibraryViewModel.cs:660–664`.)
- Renderer HGlobal allocation/free plus kernel32 `RtlMoveMemory`/`RtlZeroMemory`: Rust allocation/zero/copy with checked lengths can replace these calls; retain row alignment/stride and visible cropping semantics, avoiding use-after-free. (`Services/VlcVideoRenderer.cs:81–99`, `Services/VlcVideoRenderer.cs:142–172`, `Services/VlcVideoRenderer.cs:201–206`.)
- SharpDX.DXGI Factory1/Adapter1/Output.Description.MonitorHandle/WaitForVerticalBlank map to windows-rs DXGI factory/adapter/output enumeration and IDXGIOutput::WaitForVBlank. MonitorFromWindow/GetMonitorInfo/EnumDisplaySettings and winmm timeBeginPeriod(1) supply selection/fallback timing. Slint's compositor can replace the Avalonia reflection/DispatchProxy machinery if observable monitor-following cadence and fallback remain correct. (`Services/DynamicRenderTimer.cs:31–36`, `Services/DynamicRenderTimer.cs:108`, `Services/DynamicRenderTimer.cs:139–155`, `Services/HardwareDetector.cs:132–176`, `Program.cs:68`, `Program.cs:235–247`.)
- CommunityToolkit generated properties/commands and Avalonia Dispatcher/DispatcherTimer/Bitmap replace with Slint model notifications, UI-loop invocation, timers and image handles. Images must update when hydrated and per presented frame. Material.Icons supplies the glyph names listed above. These files do not call COM audio APIs, NAudio, SharpHook, capture/encoder APIs, or hardware-monitor refresh probes directly except via the cited timer interfaces. (`Models/ReplayClip.cs:8–50`, `ViewModels/LibraryViewModel.cs:602–609`, `Views/PlayerView.axaml.cs:128–145`, `Services/DynamicRenderTimer.cs:139–155`.)

## 7. **Cross-area interfaces**

| Boundary | Exact contract | Source |
|---|---|---|
| Settings → Library | LibraryPath, AutoCleanupEnabled, SelectedStorageLimit.Gb; live HotkeyParts for empty-library keycaps. Library does not own settings serialization. | `ViewModels/LibraryViewModel.cs:179`, `ViewModels/LibraryViewModel.cs:261–290`, `Views/LibraryView.axaml:480–490` |
| Library → Main/player | PlayClipRequested(sender,ReplayClip); Main calls Player.LoadAndPlay then CurrentView=Player; attach starts prepared media. | `ViewModels/LibraryViewModel.cs:163`, `ViewModels/LibraryViewModel.cs:670–675`, `ViewModels/MainViewModel.cs:277–281`, `Views/PlayerView.axaml.cs:141` |
| Library → Main/editor | EditClipRequested via RequestEdit, rejects images; Main OpenEditor checks images again, Editor.OpenClip then CurrentView=Editor. | `ViewModels/LibraryViewModel.cs:169–175`, `ViewModels/MainViewModel.cs:283–284`, `ViewModels/MainViewModel.cs:377–390` |
| Main navigation → Library | NavigateToLibrary changes view immediately, stops player if leaving it, fire-and-forgets Refresh; NavigateToPlayer/Editor also background-refresh shared Clips. Navigation commands stay enabled rather than awaiting scan. | `ViewModels/MainViewModel.cs:338–367` |
| Recorder save → metadata/Library | ReplaySaved writes new ClipMetadata before refresh: Game=LastSavedGameTag.Name or current target Name only if IsGame, Exe=carried Exe or target Exe. Then RefreshCommand.ExecuteAsync(null). Save status/toast belongs to other area. | `ViewModels/MainViewModel.cs:196–220` |
| Library.Clips → Player | Same observable collection/references; full master list, no game/favorite/search filter; collection changes trigger sidebar stat notifications. | `ViewModels/PlayerViewModel.cs:38–68`, `ViewModels/PlayerViewModel.cs:255–273` |
| Library.Clips → editor picker | CollectionChanged invokes RebuildEditableClips; videos-only picker excludes images. | `ViewModels/MainViewModel.cs:286–288`, `ViewModels/MainViewModel.cs:393–395` |
| Player context menus → Library | RequestEditClip→Library.RequestEdit; ShowClipInFolder→ShowInFolderCommand; DeleteReplay→DeleteClipCommand. Does not add separate player-specific confirmation/error UI. | `ViewModels/PlayerViewModel.cs:465–469` |
| Player edit control → Main | Window OpenEditorFromPlayer uses Player.CurrentClip then OpenEditor. | `Views/PlayerView.axaml:317–322`, `ViewModels/MainViewModel.cs:385–390` |
| Player fullscreen → MainWindow | Main.IsFullscreen toggle observed by Window state/layout; player subscribes to property change; Escape handled at window tunnel. | `ViewModels/MainViewModel.cs:370–375`, `Views/MainWindow.axaml.cs:67–79`, `Views/MainWindow.axaml.cs:159–165`, `Views/PlayerView.axaml.cs:312–357` |
| Window hide/navigation → Player | MainWindow closing-to-tray stops Player and Editor before Hide; PlayerView detach also stops. Main navigation to Settings/Library/Editor explicitly stops when CurrentView==Player. | `Views/MainWindow.axaml.cs:43–53`, `Views/PlayerView.axaml.cs:167–194`, `ViewModels/MainViewModel.cs:332–367` |
| Program/Window → timer | Program.RenderTimer nullable singleton; bind once, retune for monitor/window state; no dependency on selected recording monitor/fps. | `Program.cs:216–249`, `Views/MainWindow.axaml.cs:102–126` |
| Renderer → view + VM | BitmapChanged/FrameRendered UI events; view reselects image source, VM ends loading on FrameRendered. Bitmap reference rotates. | `Services/VlcVideoRenderer.cs:48–59`, `ViewModels/PlayerViewModel.cs:324`, `Views/PlayerView.axaml.cs:136–139`, `Views/PlayerView.axaml.cs:197–201` |
| Metadata → other producers | Public Read/Write/Update/SetDuration/SetFavorite/SetEdited/Delete share same hashed per-path schema; editor may tag edits, recorder tags game/exe. | `Services/ClipMetadata.cs:58–109`, `ViewModels/MainViewModel.cs:208–212` |

## 8. **Self-tests / diagnostics**

No self-test command, LAG_* environment variable, custom diagnostic overlay, dropped-frame counter, or in-area benchmark is implemented in the twelve requested files. Program has capture-area LAG_* probes, but they are not player/library tests and must not be invented here. (`ViewModels/LibraryViewModel.cs:1–876`, `ViewModels/PlayerViewModel.cs:1–593`, `Services/VlcEngine.cs:1–65`, `Services/DynamicRenderTimer.cs:1–189`; external contrast `Program.cs:70–135`.)

Expected diagnostic line formats (interpolated fields in braces; Debug lines are not guaranteed to appear in release console/session logs):

| Sink | Exact line format | Source |
|---|---|---|
| Debug | `[Library] Legacy thumbnail cleanup failed: {ex.Message}` | `ViewModels/LibraryViewModel.cs:228` |
| Debug | `[Storage] Free-space check failed: {ex.Message}` | `ViewModels/LibraryViewModel.cs:274` |
| Console | `[Storage] Quota cleanup: deleted oldest clip '{file.Name}' ({size / 1_048_576} MB).` | `ViewModels/LibraryViewModel.cs:320` |
| Debug | `[Storage] Failed to delete '{file.Name}': {ex.Message}` | `ViewModels/LibraryViewModel.cs:324` |
| Debug | `[Storage] Quota enforcement failed: {ex.Message}` | `ViewModels/LibraryViewModel.cs:330` |
| Debug | `[Library] hydrate failed: {ex.Message}` | `ViewModels/LibraryViewModel.cs:521` |
| Debug | `[Library] Failed to load screenshot {filePath}: {ex.Message}` | `ViewModels/LibraryViewModel.cs:551` |
| Debug | `[FFProbe] Failed to get duration for {filePath}: {ex.Message}` | `ViewModels/LibraryViewModel.cs:571` |
| Debug | `[FFMpeg] Failed to generate thumbnail for {filePath}: {ex.Message}` | `ViewModels/LibraryViewModel.cs:594` |
| Debug | `Failed to delete clip: {ex.Message}` | `ViewModels/LibraryViewModel.cs:717` |
| Debug | `Failed to show clip in folder: {ex.Message}` | `ViewModels/LibraryViewModel.cs:815` |
| Debug | `Failed to open folder: {ex.Message}` | `ViewModels/LibraryViewModel.cs:836` |
| Debug | `[ClipMeta] write failed: {ex.Message}` | `Services/ClipMetadata.cs:88` |
| Debug | `[Player] VLC op failed: {ex.Message}` | `ViewModels/PlayerViewModel.cs:228` |
| Debug | `VLC init failed: {ex.Message}` | `ViewModels/PlayerViewModel.cs:361` |
| Debug | `[Player] Failed to load screenshot: {ex.Message}` | `ViewModels/PlayerViewModel.cs:398` |
| Console | `[VlcVideoRenderer] format {width}x{height} (visible {visW}x{visH}), pitch {pitch}.` | `Services/VlcVideoRenderer.cs:105` |
| Console, WIP only | `[VlcEngine] ready in {sw.ElapsedMilliseconds} ms (background).` | `Services/VlcEngine.cs:56` |
| Console, WIP only | `[VlcEngine] init failed after {sw.ElapsedMilliseconds} ms: {ex.Message}` | `Services/VlcEngine.cs:61` |
| Console | `[RenderTimer] retuned to {v} Hz (window's monitor).` | `Services/DynamicRenderTimer.cs:51` |
| Console | `[RenderTimer] vsync-locked to the window's monitor.` | `Services/DynamicRenderTimer.cs:99` |
| Console | `[RenderTimer] no DXGI output resolved — clock pacing.` | `Services/DynamicRenderTimer.cs:100` |
| Console | `[RenderTimer] WaitForVBlank failed ({ex.Message}) — clock pacing.` | `Services/DynamicRenderTimer.cs:114` |
| Console | `[RenderTimer] DXGI output lookup failed: {ex.Message}` | `Services/DynamicRenderTimer.cs:161` |
| Console, external hook | `[RenderTimer] Bound DynamicRenderTimer @ {hz} Hz (fastest display; follows the window's monitor).` | `Program.cs:265` |
| Console, external hook | `[RenderTimer] ~{ticks / 2} ticks/s (current target {timer.Hz}).` | `Program.cs:261` |
| Console, external hook | `[RenderTimer] rebind failed, staying at 60 Hz: {ex}` | `Program.cs:269` |

Program's one-shot verification adds a Tick handler, counts ticks during stopwatch **8000≤ms<10000**, after 10 seconds prints integer ticks/2 once when count>0, then assigns int.MinValue. It checks timer ticks, not decoded/presented video frames; the extra subscriber itself starts/keeps timer thread alive, so the comment's assertion that this proves compositor adoption is stronger than the measurement. No env var is required. (`Program.cs:251–265`, `Services/DynamicRenderTimer.cs:60–78`.)

## 9. **Known bugs / quirks to preserve or fix**

“Preserve” means current externally observable behavior is the 1:1 baseline; changing it requires an explicit product decision. “Fix internally” means Rust should avoid unsafe lifetime/deadlock/race mechanisms while retaining the visible contract, rather than deliberately reproducing a crash. No implementation changes are made by this specification.

| Classification | Finding and exact consequence | Evidence |
|---|---|---|
| Preserve | No sort UI, recursive search, explicit refresh button, rename, confirmation/recycle bin, or player track switch. Do not turn the requested subject labels into extra features. | `Views/LibraryView.axaml:184–501`, `Views/PlayerView.axaml:264–347`, `ViewModels/LibraryViewModel.cs:184–204`, `ViewModels/LibraryViewModel.cs:700–710` |
| Preserve | Signature ignores sidecar-only changes; a same-media refresh discards freshly read metadata, keeping old Favorite/Game/Edited/positive Duration. Creation-time changes that leave scan order unchanged also do not affect signature. | `ViewModels/LibraryViewModel.cs:398–445` |
| Preserve | Metadata duration does not invalidate on changed media bytes/write time; overwritten/reencoded media can retain old duration until another area updates/deletes metadata. | `ViewModels/LibraryViewModel.cs:405–417`, `ViewModels/LibraryViewModel.cs:559–567` |
| Preserve | Memory-cache hit with positive duration returns before checking source mtime against disk JPEG. After a media overwrite, an unchanged old JPEG key can supply stale preview and suppress regeneration even though cards were rebuilt. | `ViewModels/LibraryViewModel.cs:412–415`, `ViewModels/LibraryViewModel.cs:536–538`, `ViewModels/LibraryViewModel.cs:579–582` |
| Preserve | Cached malformed metadata prevents fallback to otherwise valid legacy JSON. A path move loses cached metadata association; next-to-file/move-survival comment is obsolete. | `Services/ClipMetadata.cs:7–12`, `Services/ClipMetadata.cs:46–75` |
| Preserve | Housekeeping quota is after publication, so deleted media can remain as cards until a later changed refresh; unchanged/no-pending refresh bypasses housekeeping. Screenshots don't count; favorites can be deleted. Header usage is whole-drive, quota is video files only. | `ViewModels/LibraryViewModel.cs:261–270`, `ViewModels/LibraryViewModel.cs:298–319`, `ViewModels/LibraryViewModel.cs:436–458`, `ViewModels/LibraryViewModel.cs:487–493` |
| Preserve | Delete does not refresh drive usage; no stop/clear of currently playing clip. On source-deleted/thumbnail-failure, card/metadata may remain because remaining operations are skipped. | `ViewModels/LibraryViewModel.cs:704–720`, `ViewModels/PlayerViewModel.cs:465–469` |
| Preserve | Unfavorite while selected/FavoritesOnly can leave a hidden selected clip included in bulk delete; no NotifySelectionChanged is invoked by ToggleFavorite, so overlay/header derived notifications may lag. | `ViewModels/LibraryViewModel.cs:685–692`, `ViewModels/LibraryViewModel.cs:730–745`, `ViewModels/LibraryViewModel.cs:794–798` |
| Preserve | Anchor updates on selection-on only; Ctrl+A leaves anchor unchanged; range replaces visible selection only. | `ViewModels/LibraryViewModel.cs:753–780` |
| Preserve | Sidebar total duration is not notified on per-card hydration; static/computed localized strings don't all refresh on locale change or midnight automatically. | `ViewModels/PlayerViewModel.cs:255–285`, `ViewModels/LibraryViewModel.cs:113–156` |
| Preserve | Player has 0.1x minimum preset, no 0.05x option, no seek normalization to 1x, no slow-motion audio disabling. Stale comments must not create such behavior. | `ViewModels/PlayerViewModel.cs:124–129`, `ViewModels/PlayerViewModel.cs:163–177`, `ViewModels/PlayerViewModel.cs:213–215`, `ViewModels/PlayerViewModel.cs:241–248` |
| Preserve | Center Play is visible on screenshots/no clip; image commands no-op only where explicitly guarded. Screenshot full decode failure can keep previous image; screenshot still requires VLC init. | `Views/PlayerView.axaml:237–244`, `ViewModels/PlayerViewModel.cs:370–403`, `ViewModels/PlayerViewModel.cs:494–502` |
| Preserve | Visible clock can remain stale after Stop, end or paused seek; reset targets TimeDisplay/Position, while TimeText reads PositionSeconds/DurationSeconds and stopped polling. No special drag suppression or paused-seek polling. | `ViewModels/PlayerViewModel.cs:337–357`, `ViewModels/PlayerViewModel.cs:549–566`, `Views/PlayerView.axaml.cs:214–241` |
| Preserve | Native Stopped event during old-media swap can clear new IsClipLoading; any renderer frame can clear it, because no clip-generation tagging. Error-opening video has no dedicated EncounteredError UI handler and can remain loading until frame/Stopped. | `ViewModels/PlayerViewModel.cs:319–357`, `ViewModels/PlayerViewModel.cs:420–451` |
| Preserve | Fullscreen exit restores Normal rather than prior Maximized. Bottom reveal zone overrides right zone; right zone is narrower than sidebar; idle cursor hide doesn't hide/pin overlays. | `Views/MainWindow.axaml.cs:130–132`, `Views/PlayerView.axaml.cs:83–108` |
| Preserve | Body attach callback isn't canceled on detach; `_controlsPinned`/mute overlay opacity aren't explicitly reset on detach. | `Views/LibraryView.axaml.cs:25–33`, `Views/PlayerView.axaml.cs:167–195` |
| Preserve | GIF repeat count is finite 65535, not infinite; Next wraps master list and can restart its only video; EOF does not Next. | `ViewModels/PlayerViewModel.cs:440–441`, `ViewModels/PlayerViewModel.cs:471–487`, `ViewModels/PlayerViewModel.cs:350–357` |
| Fix internally | Active player lazy initialization is synchronous and can freeze first open; WIP VlcEngine is unused. An async Rust init can remove freeze without inventing prewarm UI or changing startup/no-open resource expectations. | `ViewModels/PlayerViewModel.cs:258–262`, `ViewModels/PlayerViewModel.cs:293–313`, `Services/VlcEngine.cs:9–18` |
| Fix internally | Concurrent metadata read/modify/write is non-atomic and can lose favorites/duration/edited flags; preserve schema but use safe coordination if improving implementation. | `Services/ClipMetadata.cs:79–103` |
| Fix internally | 2-second worker join may expire while native control remains active, then objects are disposed. Avoid use-after-free and UI shutdown deadlock in Rust. | `ViewModels/PlayerViewModel.cs:583–591` |
| Fix internally | Decoder/presenter slots and 3-second deferred disposal mitigate native render crashes but a delay isn't a GPU fence; Rust resources need real lifetime safety. Format posts reject only mismatched dimensions, not media generation. | `Services/VlcVideoRenderer.cs:107–125`, `Services/VlcVideoRenderer.cs:148–197`, `Core/DeferredDispose.cs:6–19` |
| Preserve / flag for later | DXGI wait failure only retries after monitor handle change; no subscriber exception handling or loop shutdown. Rust needn't use a forever thread, but match cadence/fallback and report a deliberate recovery change if added. | `Services/DynamicRenderTimer.cs:88–133`, `Services/DynamicRenderTimer.cs:69–78` |
| Documentation correction | ReplayClip.IsImage comment says system viewer; actual app opens integrated screenshot player. Scope comments say MP4-only although all eight patterns are used. Card “fade-rise” is opacity-only, VideoFrame comment r16 is 12, clock “display-rate” timer is 33 ms. | `Models/ReplayClip.cs:30–31`, `ViewModels/LibraryViewModel.cs:184–204`, `ViewModels/LibraryViewModel.cs:674–675`, `Views/LibraryView.axaml:147–162`, `Views/PlayerView.axaml:165–169`, `Views/PlayerView.axaml.cs:23–25` |

## 10. **Parity checklist**

Each checkbox is one independently verifiable assertion. Together with exact UI/state/format tables above, this is the acceptance test. Backend implementation details may differ where Section 6 explicitly allows an equivalent and Section 9 marks internal safety fixes.

- [ ] Discover top-level MP4 files. (`ViewModels/LibraryViewModel.cs:184–200`.)
- [ ] Discover top-level MKV files. (`ViewModels/LibraryViewModel.cs:184–200`.)
- [ ] Discover top-level MOV files. (`ViewModels/LibraryViewModel.cs:184–200`.)
- [ ] Discover top-level AVI files. (`ViewModels/LibraryViewModel.cs:184–200`.)
- [ ] Discover GIF as animated video rather than screenshot. (`ViewModels/LibraryViewModel.cs:184–204`.)
- [ ] Discover PNG as screenshot. (`ViewModels/LibraryViewModel.cs:187–204`.)
- [ ] Discover JPG as screenshot. (`ViewModels/LibraryViewModel.cs:187–204`.)
- [ ] Discover JPEG as screenshot. (`ViewModels/LibraryViewModel.cs:187–204`.)
- [ ] Ignore JSON/cache/unrecognized files in the library listing. (`ViewModels/LibraryViewModel.cs:184–200`.)
- [ ] Do not traverse library subfolders. (`ViewModels/LibraryViewModel.cs:197–200`.)
- [ ] Create a missing configured library directory during scan. (`ViewModels/LibraryViewModel.cs:387`.)
- [ ] Sort master media by local CreationTime descending. (`ViewModels/LibraryViewModel.cs:391`.)
- [ ] Derive card title from filename without extension. (`Models/ReplayClip.cs:69`.)
- [ ] Use FileInfo.CreationTime for card date and grouping. (`ViewModels/LibraryViewModel.cs:418`.)
- [ ] Publish card models without waiting for image decoding or FFmpeg/probe work. (`ViewModels/LibraryViewModel.cs:378–458`.)
- [ ] Cancel/stale-check a superseded refresh before publication. (`ViewModels/LibraryViewModel.cs:368–371`, `ViewModels/LibraryViewModel.cs:431–432`.)
- [ ] Let only newest refresh clear shared IsLoading. (`ViewModels/LibraryViewModel.cs:463–469`.)
- [ ] Build exact path/length/UTC-write-ticks/newline folder signature. (`ViewModels/LibraryViewModel.cs:398–400`.)
- [ ] Keep existing cards/selection when signature and count match. (`ViewModels/LibraryViewModel.cs:436–445`.)
- [ ] Resume only missing thumbnails/durations on unchanged-folder fast path. (`ViewModels/LibraryViewModel.cs:441–444`.)
- [ ] Rebuild chips/groups once changed scan publishes. (`ViewModels/LibraryViewModel.cs:448–455`.)
- [ ] End scan loading before background hydration completes. (`ViewModels/LibraryViewModel.cs:457–469`.)
- [ ] Schedule hydration newest-first with concurrency clamp(cpuCount/2,2,6). (`ViewModels/LibraryViewModel.cs:497–516`.)
- [ ] Suppress canceled hydration's late UI updates. (`ViewModels/LibraryViewModel.cs:598–608`.)
- [ ] Cache positive probed duration in sidecar. (`ViewModels/LibraryViewModel.cs:560–567`.)
- [ ] Reprobe duration only when loaded duration≤0. (`ViewModels/LibraryViewModel.cs:559–572`.)
- [ ] Thumbnail snapshot is at 2 s when duration>3 s. (`ViewModels/LibraryViewModel.cs:581`.)
- [ ] Thumbnail snapshot is at 0 s when duration≤3 s, including exactly 3 s. (`ViewModels/LibraryViewModel.cs:581`.)
- [ ] Produce one 320×180 JPEG preview for video/GIF. (`ViewModels/LibraryViewModel.cs:582`, Section 4 pinned wrapper references.)
- [ ] Regenerate absent/on-disk-older thumbnail when hydration reaches that check. (`ViewModels/LibraryViewModel.cs:579–582`.)
- [ ] Preserve positive-duration/memory-thumbnail early-return behavior. (`ViewModels/LibraryViewModel.cs:536–538`.)
- [ ] Decode screenshot card preview to width 640. (`ViewModels/LibraryViewModel.cs:545–547`.)
- [ ] Never generate a separate disk thumbnail for screenshots. (`ViewModels/LibraryViewModel.cs:412`, `ViewModels/LibraryViewModel.cs:540–547`.)
- [ ] Video thumbnail cache location is LocalAppData/Lag/thumbnails. (`ViewModels/LibraryViewModel.cs:210–212`.)
- [ ] Persistent thumbnail basename is uppercase MD5 of UTF-8 invariant-lower path. (`ViewModels/LibraryViewModel.cs:660–664`.)
- [ ] Decoded-cache key includes thumbnail path and UTC-write ticks. (`ViewModels/LibraryViewModel.cs:631–644`.)
- [ ] At count≥600, clear decoded cache wholesale before insertion without disposing displayed images. (`ViewModels/LibraryViewModel.cs:647–652`.)
- [ ] Remove legacy library/.thumbnails recursively during hydration. (`ViewModels/LibraryViewModel.cs:218–229`, `ViewModels/LibraryViewModel.cs:491`.)
- [ ] Read/write exact five-property PascalCase metadata schema. (`Services/ClipMetadata.cs:14–40`.)
- [ ] Use null/false defaults in metadata. (`Services/ClipMetadata.cs:17–31`.)
- [ ] Store metadata under LocalAppData/Lag/metadata with same path hash and .json. (`Services/ClipMetadata.cs:42–50`.)
- [ ] Serialize metadata indented with null/default fields retained. (`Services/ClipMetadata.cs:40`, `Services/ClipMetadata.cs:84`.)
- [ ] Prefer cached JSON to legacy clipPath+.json. (`Services/ClipMetadata.cs:62–70`.)
- [ ] Migrate valid legacy JSON on first read. (`Services/ClipMetadata.cs:66–71`.)
- [ ] Remove legacy JSON after successful cached write. (`Services/ClipMetadata.cs:83–86`.)
- [ ] Return null on unreadable/malformed metadata without surfacing UI error. (`Services/ClipMetadata.cs:58–75`.)
- [ ] Do not fall back to legacy when a corrupt cache already exists. (`Services/ClipMetadata.cs:62–75`.)
- [ ] Delete both metadata locations on clip delete. (`Services/ClipMetadata.cs:105–109`.)
- [ ] Mark edited if metadata.Edited or basename contains -edit case-insensitively. (`ViewModels/LibraryViewModel.cs:405–407`.)
- [ ] Game chips derive from entire master library independent of filters. (`ViewModels/LibraryViewModel.cs:78–82`.)
- [ ] Distinct game names compare OrdinalIgnoreCase. (`ViewModels/LibraryViewModel.cs:79`.)
- [ ] Sort game chip labels CurrentCultureIgnoreCase. (`ViewModels/LibraryViewModel.cs:80`.)
- [ ] Append localized Desktop chip with U+0001desktop filter key only when desktop clips exist. (`ViewModels/LibraryViewModel.cs:72`, `ViewModels/LibraryViewModel.cs:92–93`.)
- [ ] Drop stale category filter after last corresponding clip disappears. (`ViewModels/LibraryViewModel.cs:84–87`.)
- [ ] Clicking active game chip returns to unfiltered category state. (`ViewModels/LibraryViewModel.cs:59`.)
- [ ] Trim search and match filename/game substring case-insensitively. (`ViewModels/LibraryViewModel.cs:99–111`.)
- [ ] Combine search, game/Desktop, and favorites predicates with AND. (`ViewModels/LibraryViewModel.cs:101–111`.)
- [ ] Clear selection on SearchText change. (`ViewModels/LibraryViewModel.cs:52`.)
- [ ] Clear selection on FavoritesOnly change. (`ViewModels/LibraryViewModel.cs:53`.)
- [ ] Clear selection on category command. (`ViewModels/LibraryViewModel.cs:62`.)
- [ ] Rebuild groups immediately without search debounce. (`ViewModels/LibraryViewModel.cs:52–53`.)
- [ ] Group by creation day descending, retaining source order within day. (`ViewModels/LibraryViewModel.cs:118–145`.)
- [ ] Use uppercase localized Today with d MMM subtitle. (`ViewModels/LibraryViewModel.cs:122–125`, `ViewModels/LibraryViewModel.cs:140`.)
- [ ] Use uppercase localized Yesterday with d MMM subtitle. (`ViewModels/LibraryViewModel.cs:127–130`, `ViewModels/LibraryViewModel.cs:140`.)
- [ ] Use culture-formatted d MMMM uppercase and dddd for older days. (`ViewModels/LibraryViewModel.cs:132–140`.)
- [ ] Header count shows all clips while day counts show filtered bucket counts. (`ViewModels/LibraryViewModel.cs:50`, `ViewModels/LibraryViewModel.cs:142`.)
- [ ] Distinguish empty library from nonempty/no-results state. (`ViewModels/LibraryViewModel.cs:148`, `Views/LibraryView.axaml:455–478`.)
- [ ] Render Library header/filter/grid dimensions and theme tokens from Section 3. (`Views/LibraryView.axaml:184–317`.)
- [ ] Scroll chip strip by 60 DIPs per wheel delta with clamping. (`Views/LibraryView.axaml.cs:36–44`.)
- [ ] Card preview is 248×140 UniformToFill/radius 13. (`Views/LibraryView.axaml:316–339`, `Themes/Tokens.axaml:19`.)
- [ ] Card ring is accent 2 pixels and remains visible on selected card. (`Views/LibraryView.axaml:66–79`, `Views/LibraryView.axaml:343–345`.)
- [ ] Show favorite badge iff IsFavorite. (`Views/LibraryView.axaml:347–352`.)
- [ ] Fade EDITED badge on hover or own selection. (`Views/LibraryView.axaml:111–120`.)
- [ ] Hide duration badge for screenshots. (`Views/LibraryView.axaml:364–367`.)
- [ ] Show card HH:mm plus optional dot/game name. (`Views/LibraryView.axaml:435–442`.)
- [ ] Hide all card hover action overlays while any clip is selected. (`Views/LibraryView.axaml:370–372`.)
- [ ] Hover Play opens both video and screenshot in integrated player. (`Views/LibraryView.axaml:384–389`, `ViewModels/LibraryViewModel.cs:670–675`.)
- [ ] Hover favorite toggle persists favorite immediately. (`ViewModels/LibraryViewModel.cs:687–692`.)
- [ ] Unfavorite under favorites-only removes card from displayed groups. (`ViewModels/LibraryViewModel.cs:692`.)
- [ ] Hide hover Edit on screenshots. (`Views/LibraryView.axaml:406`.)
- [ ] Selection circle stays above hover overlay. (`Views/LibraryView.axaml:418–428`.)
- [ ] Card opacity entrance is 300 ms CubicEaseOut. (`Views/LibraryView.axaml:154–157`.)
- [ ] Card hover lift is −3 DIPs in 160 ms CubicEaseOut. (`Views/LibraryView.axaml:149–161`.)
- [ ] Delay library body by 240 ms then fade/rise +12→0 over 220 ms. (`Views/LibraryView.axaml.cs:25–32`.)
- [ ] Allow scrolling last complete card title using content bottom margin, not viewport bottom padding. (`Views/LibraryView.axaml:285–296`.)
- [ ] Show both declared library IsLoading strips. (`Views/LibraryView.axaml:281–283`, `Views/LibraryView.axaml:499–501`.)
- [ ] Plain card left press with no selection plays clip. (`Views/LibraryView.axaml.cs:66–69`.)
- [ ] Plain card left press while selecting toggles membership. (`Views/LibraryView.axaml.cs:68`.)
- [ ] Ctrl-left toggles membership. (`Views/LibraryView.axaml.cs:67`.)
- [ ] Shift-left takes precedence over Ctrl and selects inclusive visible range. (`Views/LibraryView.axaml.cs:66`, `ViewModels/LibraryViewModel.cs:764–772`.)
- [ ] Selecting-on updates anchor; selecting-off leaves prior anchor. (`ViewModels/LibraryViewModel.cs:755–756`.)
- [ ] Shift with no valid anchor adds clicked clip without clearing others. (`ViewModels/LibraryViewModel.cs:768`.)
- [ ] Shift-range leaves anchor fixed for repeated range adjustment. (`ViewModels/LibraryViewModel.cs:772`.)
- [ ] Ctrl+A selects visible clips without assigning anchor. (`Views/LibraryView.axaml:180`, `ViewModels/LibraryViewModel.cs:777–780`.)
- [ ] Escape clears selection and anchor. (`Views/LibraryView.axaml:181`, `ViewModels/LibraryViewModel.cs:785–789`.)
- [ ] Empty-space press clears selection. (`Views/LibraryView.axaml.cs:85–95`.)
- [ ] Right-click card preserves existing selection. (`Views/LibraryView.axaml.cs:63`.)
- [ ] Show Delete(N)/Clear selection only when HasSelection. (`Views/LibraryView.axaml:209–223`.)
- [ ] Delete selected clips sequentially from a snapshot including hidden selected items. (`ViewModels/LibraryViewModel.cs:794–798`.)
- [ ] Context menu order is Edit, Show in folder, Delete. (`Views/LibraryView.axaml:319–331`.)
- [ ] Screenshot context Edit is present but does nothing. (`Views/LibraryView.axaml:321`, `ViewModels/LibraryViewModel.cs:174`.)
- [ ] Delete source, thumbnail, metadata, then remove/rebuild card view. (`ViewModels/LibraryViewModel.cs:704–713`.)
- [ ] Delete permanently without confirmation/recycle-bin flow. (`ViewModels/LibraryViewModel.cs:700–721`.)
- [ ] Missing source does not prevent removing card/caches. (`ViewModels/LibraryViewModel.cs:706–713`.)
- [ ] Show in folder requires existing source and selects it in Explorer. (`ViewModels/LibraryViewModel.cs:807–811`.)
- [ ] Open folder uses shell on configured LibraryPath. (`ViewModels/LibraryViewModel.cs:823–832`.)
- [ ] Drive usage is whole-drive available/total binary GB with exact rounding. (`ViewModels/LibraryViewModel.cs:261–270`.)
- [ ] Usage bar is 120 DIPs times clamped percentage. (`ViewModels/LibraryViewModel.cs:247`.)
- [ ] Quota is opt-in and ignores nonpositive limits. (`ViewModels/LibraryViewModel.cs:288–291`.)
- [ ] Quota counts only the five video patterns. (`ViewModels/LibraryViewModel.cs:298–303`.)
- [ ] Quota deletes oldest CreationTime first without exempting favorites. (`ViewModels/LibraryViewModel.cs:298–319`.)
- [ ] Quota removes metadata and video-thumbnail disk cache too. (`ViewModels/LibraryViewModel.cs:314–317`.)
- [ ] Quota runs at construction and phase-2 housekeeping as specified. (`ViewModels/LibraryViewModel.cs:340–343`, `ViewModels/LibraryViewModel.cs:488–493`.)
- [ ] Player sidebar uses unfiltered shared Library.Clips. (`ViewModels/PlayerViewModel.cs:47–51`.)
- [ ] Sidebar selection immediately loads/starts clip without recursion. (`ViewModels/PlayerViewModel.cs:77–81`, `ViewModels/PlayerViewModel.cs:379–382`.)
- [ ] Player engine is not initialized during VM construction. (`ViewModels/PlayerViewModel.cs:258–263`.)
- [ ] Active path uses global no-title/no-OSD settings. (`ViewModels/PlayerViewModel.cs:302`.)
- [ ] Do not treat uncalled WIP VlcEngine prewarm as shipped behavior. (`Services/VlcEngine.cs:31–43`, `ViewModels/PlayerViewModel.cs:293–313`.)
- [ ] Clip preparation queues media swap before playback starts. (`ViewModels/PlayerViewModel.cs:426–453`, `Views/PlayerView.axaml.cs:141`.)
- [ ] Reset selected playback speed to 1x for every new clip. (`ViewModels/PlayerViewModel.cs:373–375`.)
- [ ] Use software video decode for baseline parity. (`ViewModels/PlayerViewModel.cs:443–446`.)
- [ ] GIF uses animated FFmpeg demuxing. (`ViewModels/PlayerViewModel.cs:438–440`.)
- [ ] GIF repeats 65,535 times and normal videos do not inherit repeat. (`ViewModels/PlayerViewModel.cs:436–446`.)
- [ ] Serialize/coalesce controls off UI thread so slow seeks/swaps do not freeze UI. (`ViewModels/PlayerViewModel.cs:180–239`.)
- [ ] Release old media references after replacement. (`ViewModels/PlayerViewModel.cs:448–451`.)
- [ ] Screenshot opens full resolution inside player. (`ViewModels/PlayerViewModel.cs:384–403`.)
- [ ] Image mode hides timeline/transport/speed/edit/volume while retaining fullscreen. (`Views/PlayerView.axaml:246–347`.)
- [ ] Center Play visibility depends only on !IsPlaying, even for image/no clip. (`Views/PlayerView.axaml:237–244`.)
- [ ] PlayPause no-ops for image/no native player. (`ViewModels/PlayerViewModel.cs:502`.)
- [ ] Switching image→video drops still image with safe delayed lifetime. (`ViewModels/PlayerViewModel.cs:407–413`.)
- [ ] Render player title/meta and frame/sidebar layout exactly as Section 3. (`Views/PlayerView.axaml:139–169`, `Views/PlayerView.axaml:356–439`.)
- [ ] Video is uniformly scaled with letterboxing and high-quality interpolation. (`Views/PlayerView.axaml:190–192`.)
- [ ] Show clip loading until presented frame rather than merely media Playing. (`ViewModels/PlayerViewModel.cs:319–324`.)
- [ ] Playing state starts position polling and reapplies chosen volume/rate. (`ViewModels/PlayerViewModel.cs:326–335`.)
- [ ] Pausing stops native position polling. (`ViewModels/PlayerViewModel.cs:337`.)
- [ ] Stop resets normalized position to 0 through state notification. (`ViewModels/PlayerViewModel.cs:338–346`.)
- [ ] Natural EOF parks normalized seek position at 1 without auto-next. (`ViewModels/PlayerViewModel.cs:350–357`.)
- [ ] Play after EOF restarts from beginning. (`ViewModels/PlayerViewModel.cs:508–512`.)
- [ ] Stop, rewind, play/pause, forward, and next appear in specified order. (`Views/PlayerView.axaml:268–293`.)
- [ ] Rewind subtracts 10 s with lower clamp zero. (`ViewModels/PlayerViewModel.cs:540–542`.)
- [ ] Forward adds 10 s without extra application upper clamp. (`ViewModels/PlayerViewModel.cs:533–535`.)
- [ ] Seek slider is normalized 0–1 and TwoWay. (`Views/PlayerView.axaml:257–258`.)
- [ ] Track left press jumps immediately to pointer position. (Pinned Avalonia Slider.cs:328–356 linked in Section 3.)
- [ ] Pointer dragging continuously updates slider value without tick snapping. (Pinned Avalonia Slider.cs:310–356 linked in Section 3.)
- [ ] Retain focused-slider inherited keyboard behavior and original event routing. (Pinned Avalonia Slider.cs:207–257 linked in Section 3.)
- [ ] Coalesce slider changes to latest native seek and require seekable media. (`ViewModels/PlayerViewModel.cs:128`, `ViewModels/PlayerViewModel.cs:198–215`.)
- [ ] Do not automatically pause/resume or reset rate while dragging. (`ViewModels/PlayerViewModel.cs:124–129`, `Views/PlayerView.axaml:257–258`.)
- [ ] Preserve specified paused-seek/clock-update limitations unless explicitly changed. (`ViewModels/PlayerViewModel.cs:337`, `ViewModels/PlayerViewModel.cs:549–566`, `Views/PlayerView.axaml.cs:227–241`.)
- [ ] Speed menu contains exactly 2,1.5,1,0.5,0.25,0.1 in that order. (`ViewModels/PlayerViewModel.cs:165`.)
- [ ] Speed label uses 1x special case and otherwise 0.##x. (`ViewModels/PlayerViewModel.cs:172`.)
- [ ] Speed change only sets rate, without track switching. (`ViewModels/PlayerViewModel.cs:174–177`, `ViewModels/PlayerViewModel.cs:245–248`.)
- [ ] Speed flyout pins controls until closed. (`Views/PlayerView.axaml.cs:244–250`.)
- [ ] Volume defaults to 80. (`ViewModels/PlayerViewModel.cs:140`.)
- [ ] Volume slider is width 56 and ranges 0–100. (`Views/PlayerView.axaml:337–341`.)
- [ ] Mute leaves volume-slider value unchanged. (`ViewModels/PlayerViewModel.cs:148–161`.)
- [ ] Speaker icon uses VolumeHigh/TextMid or VolumeOff/DangerStop. (`Views/PlayerView.axaml:332–335`.)
- [ ] Mute/unmute flashes 96-pixel badge for 850 ms with 300 ms opacity transition. (`Views/PlayerView.axaml:216–228`, `Views/PlayerView.axaml.cs:34`, `Views/PlayerView.axaml.cs:290–294`.)
- [ ] Speaker press pops scale to 1.35 then restores after 130 ms. (`Views/PlayerView.axaml.cs:275–286`.)
- [ ] No player audio-track selector or extra mpv default track shortcut appears. (`Views/PlayerView.axaml:264–347`, `Views/PlayerView.axaml:12–17`.)
- [ ] Space toggles play/pause while player focused. (`Views/PlayerView.axaml:13`.)
- [ ] Left seeks backward 10 seconds while player focused. (`Views/PlayerView.axaml:14`.)
- [ ] Right seeks forward 10 seconds while player focused. (`Views/PlayerView.axaml:15`.)
- [ ] M toggles mute while player focused. (`Views/PlayerView.axaml:16`.)
- [ ] Left click on video toggles playback and restores keyboard focus. (`Views/PlayerView.axaml.cs:262–271`.)
- [ ] Right click on video does not toggle playback. (`Views/PlayerView.axaml.cs:265`, `Views/PlayerView.axaml:176–189`.)
- [ ] Next wraps master library order, skipping screenshots. (`ViewModels/PlayerViewModel.cs:475–485`.)
- [ ] Next with only one video may restart same video. (`ViewModels/PlayerViewModel.cs:478–485`.)
- [ ] Next with all screenshots does nothing. (`ViewModels/PlayerViewModel.cs:478–487`.)
- [ ] Player context menus delegate same file operations as Library. (`ViewModels/PlayerViewModel.cs:467–469`.)
- [ ] Sidebar context actions target right-clicked row, video menu targets current clip. (`Views/PlayerView.axaml.cs:307–310`.)
- [ ] Player delete does not explicitly clear/stop CurrentClip. (`ViewModels/PlayerViewModel.cs:469`.)
- [ ] Sidebar rows use 64×36 rounded thumbnail and DisplayName/ShortMeta. (`Views/PlayerView.axaml:398–411`.)
- [ ] Sidebar count includes screenshots. (`ViewModels/PlayerViewModel.cs:54`.)
- [ ] Summed duration uses localized truncated h/min/sec format. (`ViewModels/PlayerViewModel.cs:277–285`.)
- [ ] Sidebar size uses F1 GB at ≥2^30 bytes, otherwise F0 MB. (`ViewModels/PlayerViewModel.cs:64–67`.)
- [ ] Preserve collection-only stat notification behavior. (`ViewModels/PlayerViewModel.cs:255–273`.)
- [ ] Native time/length/position are sampled every 100 ms while playing. (`ViewModels/PlayerViewModel.cs:315–317`, `ViewModels/PlayerViewModel.cs:549–566`.)
- [ ] Visible millisecond clock interpolates every 33 ms using wall elapsed×max(0.01,speed). (`Views/PlayerView.axaml.cs:25`, `Views/PlayerView.axaml.cs:227–241`.)
- [ ] Clock clamps to total only when total>0. (`Views/PlayerView.axaml.cs:234`.)
- [ ] Fullscreen button toggles global window fullscreen flag. (`Views/PlayerView.axaml:342–346`, `ViewModels/MainViewModel.cs:375`.)
- [ ] Escape exits fullscreen via window tunnel handler. (`Views/MainWindow.axaml.cs:16–17`, `Views/MainWindow.axaml.cs:159–165`.)
- [ ] Exiting fullscreen restores WindowState.Normal. (`Views/MainWindow.axaml.cs:132`.)
- [ ] Fullscreen removes player margins, frame radius and frame border. (`Views/PlayerView.axaml.cs:323–327`.)
- [ ] Fullscreen hides clip header and shell topbar/rail. (`Views/PlayerView.axaml:151`, `Views/MainWindow.axaml:221`, `Views/MainWindow.axaml:328`.)
- [ ] Fullscreen sidebar overlays right with margin 0,12,12,118. (`Views/PlayerView.axaml.cs:333–337`.)
- [ ] Controls initially hidden in both modes. (`Views/PlayerView.axaml.cs:346–350`.)
- [ ] Windowed controls reveal only over video or while pinned. (`Views/PlayerView.axaml.cs:93–99`.)
- [ ] Fullscreen bottom 140-DIP zone reveals controls. (`Views/PlayerView.axaml.cs:58`, `Views/PlayerView.axaml.cs:105–107`.)
- [ ] Fullscreen right 280-DIP zone reveals sidebar except bottom zone. (`Views/PlayerView.axaml.cs:61`, `Views/PlayerView.axaml.cs:105–108`.)
- [ ] Hiding overlays disables their hit testing while retaining layout. (`Views/PlayerView.axaml.cs:63–73`.)
- [ ] Control hide uses opacity/Y+20 over 250 ms CubicEaseOut. (`Views/PlayerView.axaml:103–115`.)
- [ ] Sidebar hide uses opacity/X+24 over 250 ms CubicEaseOut. (`Views/PlayerView.axaml:119–131`.)
- [ ] Fullscreen cursor hides after 2.5 s idle and wakes on movement. (`Views/PlayerView.axaml.cs:21`, `Views/PlayerView.axaml.cs:75–91`.)
- [ ] Windowed cursor stays restored and idle timer stopped. (`Views/PlayerView.axaml.cs:353–354`.)
- [ ] Detach unsubscribes events, stops view timers and stops playback. (`Views/PlayerView.axaml.cs:167–194`.)
- [ ] Hide-to-tray stops playback even if view remains attached. (`Views/MainWindow.axaml.cs:43–53`.)
- [ ] UI video supports clipping/overlays/input without independent native-host window. (`Services/VlcVideoRenderer.cs:12–21`.)
- [ ] Decoded visible geometry excludes coded right/bottom padding. (`Services/VlcVideoRenderer.cs:83–86`, `Services/VlcVideoRenderer.cs:156–172`.)
- [ ] Frame memory is initialized before first presentation. (`Services/VlcVideoRenderer.cs:96–99`.)
- [ ] No decoder write corrupts a frame actively used by compositor. (`Services/VlcVideoRenderer.cs:17–21`, `Services/VlcVideoRenderer.cs:176–193`.)
- [ ] Coalesce video presentation requests instead of queueing every decode frame. (`Services/VlcVideoRenderer.cs:181–197`.)
- [ ] Re-read rotating presented bitmap/image source on every frame. (`Views/PlayerView.axaml.cs:199–201`.)
- [ ] Keep replaced UI resources alive through rendering; C# fallback delay is 3000 ms. (`Core/DeferredDispose.cs:14–19`.)
- [ ] Runtime fallback render Hz clamps 30–360. (`Services/DynamicRenderTimer.cs:39–50`.)
- [ ] Initial target is fastest display clamped 60–240. (`Program.cs:204–213`.)
- [ ] Visible window composition follows its monitor's real vblank. (`Services/DynamicRenderTimer.cs:90–122`, `Views/MainWindow.axaml.cs:124–126`.)
- [ ] Dragging between monitors rebinds output/cadence. (`Views/MainWindow.axaml.cs:22–23`, `Services/DynamicRenderTimer.cs:91–100`.)
- [ ] Hidden/minimized window falls back to 60 Hz clock without monitor-vblank wait. (`Views/MainWindow.axaml.cs:112–116`.)
- [ ] Vblank failure falls back to clock and logs the failure. (`Services/DynamicRenderTimer.cs:111–117`.)
- [ ] Clock fallback resynchronizes after >4 intervals of stall. (`Services/DynamicRenderTimer.cs:128–133`.)
- [ ] Do not conflate compositor Hz, media fps, speed, 100-ms polling, or 33-ms label timer. (`Services/DynamicRenderTimer.cs:108–133`, `ViewModels/PlayerViewModel.cs:315–317`, `Views/PlayerView.axaml.cs:25`.)
- [ ] Retain documented diagnostic conditions/formats with backend naming adaptation clearly identified. (`Services/DynamicRenderTimer.cs:51–161`, `Services/VlcVideoRenderer.cs:105`, Section 8.)
- [ ] Preserve source behavioral quirks explicitly marked in Section 9 or record an approved divergence. (Section 9 source citations.)
