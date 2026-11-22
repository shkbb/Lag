# Lag theme API v1

_Generated from `ui/api/` - do not edit._ A theme imports these from `"@lag/api.slint"`:

```slint
import { MainVM, LibraryVM, Tr } from "@lag/api.slint";
```

- **in property**: Lag sets it, the theme shows it.
- **in-out property**: either side may change it (e.g. the current page).
- **callback**: the theme calls it, Lag does the work (save a replay, open a clip...). A `pure callback` is answered by Lag (`Tr.t(key, Tr.rev)` = the translated text).

Lag's own building blocks can be used too: `@lag/components/...` (buttons, cards, settings rows, the clip card...), `@lag/theme/tokens.slint` (colours, sizes) and `@lag/theme/icons.slint`.

**A theme holds only what it changes.** Lag puts it together over its own interface: a file the theme has (same path as in Lag's `ui\`, e.g. `theme/tokens.slint` or `views/library.slint`) replaces Lag's, every other file is Lag's, the newest one (a Lag update reaches the parts the theme did not change). `api/` is always Lag's. The quickest start: Settings > Interface > **New theme**, then **Edit** (the first change in a file copies it into the theme) or put a changed copy of one of Lag's files into the theme folder and press **Preview**.

## Globals

### Tr

Localisation. Markup: Tr.t("Btn_SaveReplay", Tr.rev). Rust implements `t` with lag_core::i18n and bumps `rev` on a language change so every bound string re-evaluates. Keys = the C# resource keys.

| Name | Kind | Type | Notes |
|---|---|---|---|
| `rev` | in property | `int` |  |
| `t` | pure callback | `(string, int) -> string` |  |

### MainVM

| Name | Kind | Type | Notes |
|---|---|---|---|
| `current-page` | in-out property | `Page` |  |
| `theme-fade-image` | in property | `image` | theme switch cross-fade: a snapshot of the old look fades out over the new one (owner, 2026-11-17) |
| `theme-fade-active` | in property | `bool` |  |
| `theme-fade-shown` | in property | `bool` |  |
| `is-recording` | in property | `bool` |  |
| `is-paused` | in property | `bool` |  |
| `is-saving` | in property | `bool` |  |
| `is-starting` | in property | `bool` | Start pressed, engine not up yet (the start takes a moment): the button shows it is working. |
| `status-text` | in property | `string` |  |
| `buffer-status` | in property | `string` |  |
| `buffer-fill-percent` | in property | `float` | 0..100 |
| `is-fullscreen` | in-out property | `bool` |  |
| `has-capture-target` | in property | `bool` |  |
| `capture-target-is-game` | in property | `bool` |  |
| `capture-target-name` | in property | `string` |  |
| `toggle-recording` | callback | `()` |  |
| `save-replay` | callback | `()` |  |
| `toggle-pause-recording` | callback | `()` |  |
| `navigate` | callback | `(Page)` |  |
| `toggle-fullscreen` | callback | `()` |  |
| `take-screenshot` | callback | `()` |  |
| `push-to-talk` | callback | `(bool)` | true = key down (mic open), false = released |
| `show-main-window` | callback | `()` | tray "Open", second-instance activation |
| `minimize-window` | callback | `()` | window chrome |
| `maximize-window` | callback | `()` |  |
| `close-window` | callback | `()` |  |
| `begin-move-drag` | callback | `()` |  |
| `occluder-changed` | callback | `(bool, float, float, float, float)` | Main-window UI that can float over the player's native video surface (window coords, logical px). The video gets that rectangle cut out so the UI stays visible (airspace). |

### WindowMetrics

Current main-window size, kept up to date by AppWindow. Used by full-window popups (dropdowns) that handle outside clicks themselves.

| Name | Kind | Type | Notes |
|---|---|---|---|
| `width` | in-out property | `length` |  |
| `height` | in-out property | `length` |  |

### SettingsVM

| Name | Kind | Type | Notes |
|---|---|---|---|
| `webdav-open` | in-out property | `bool` | Settings > Library > Add storage > WebDAV |
| `webdav-busy` | in property | `bool` |  |
| `webdav-error` | in property | `string` | i18n key of the last problem ("" = none) |
| `webdav-connect` | callback | `(string  url , string  login , string  password )` |  |
| `tab` | in-out property | `int` |  |
| `groups` | in property | `[SettingsGroup]` |  |
| `presets` | in property | `[PresetData]` |  |
| `pending` | in property | `bool` |  |
| `cleanup-hint` | in property | `bool` |  |
| `cleanup-text` | in property | `string` |  |
| `warning` | in-out property | `bool` |  |
| `warning-summary` | in property | `string` |  |
| `overlay-boxes` | in property | `[OverlayBoxData]` |  |
| `overlay-enabled` | in property | `bool` |  |
| `mic-level` | in property | `float` |  |
| `output-level` | in property | `float` |  |
| `gate-threshold` | in property | `float` |  |
| `gate-open` | in property | `bool` |  |
| `hear-yourself` | in property | `bool` |  |
| `capture-id` | in property | `string` |  |
| `hotkeys-suppressed` | in property | `bool` |  |
| `language-flags` | in property | `[image]` |  |
| `select-tab` | callback | `(int)` |  |
| `choose` | callback | `(string, int)` |  |
| `toggle` | callback | `(string, bool)` |  |
| `number` | callback | `(string, float)` |  |
| `text` | callback | `(string, string)` |  |
| `action` | callback | `(string)` |  |
| `apply-preset` | callback | `(int)` |  |
| `commit-overlay` | callback | `(int, float, float, float)` |  |
| `acknowledge` | callback | `()` |  |
| `captured` | callback | `(string, string)` | Shell hook seam uses C# KeyCode and ModifierMask names. Escape clears the binding. |
| `update-status` | callback | `(string)` |  |
| `theme-mode` | in property | `string` |  |

### LibraryVM

| Name | Kind | Type | Notes |
|---|---|---|---|
| `is-loading` | in property | `bool` |  |
| `clip-count-label` | in property | `string` |  |
| `used-space-display` | in property | `string` |  |
| `used-space-bar-width` | in property | `float` |  |
| `has-clips` | in property | `bool` |  |
| `has-no-results` | in property | `bool` |  |
| `has-selection` | in property | `bool` |  |
| `delete-selected-label` | in property | `string` |  |
| `search-text` | in-out property | `string` |  |
| `favorites-only` | in property | `bool` |  |
| `game-chips` | in property | `[GameChipData]` |  |
| `hotkey-parts` | in property | `[string]` |  |
| `total-content-height` | in property | `float` |  |
| `visible-rows` | in property | `[LibraryRowData]` |  |
| `cloud-connected` | in property | `bool` | a cloud is connected: the cloud actions are offered |
| `delete-dialog-open` | in-out property | `bool` | Delete (docs/spec/cloud.md §12): asked when a clip is also, or only, in the cloud |
| `delete-dialog-mixed` | in property | `bool` | some of the clips are both here and in the cloud (offer "this PC" / "cloud" / "everywhere") |
| `delete-dialog-cloud-only` | in property | `bool` | every clip is only in the cloud (deleting is for good) |
| `search-changed` | callback | `(string)` |  |
| `set-game-filter` | callback | `(string)` |  |
| `toggle-favorites-only` | callback | `()` |  |
| `open-library-folder` | callback | `()` |  |
| `delete-selected` | callback | `()` |  |
| `clear-selection` | callback | `()` |  |
| `select-all` | callback | `()` |  |
| `card-pressed` | callback | `(int  clip-index , bool  shift , bool  ctrl )` |  |
| `card-play` | callback | `(int  clip-index )` |  |
| `card-favorite` | callback | `(int  clip-index )` |  |
| `card-edit` | callback | `(int  clip-index )` |  |
| `card-select-circle` | callback | `(int  clip-index )` |  |
| `context-edit` | callback | `(string  file-path )` |  |
| `context-show-in-folder` | callback | `(string  file-path )` |  |
| `context-delete` | callback | `(string  file-path )` |  |
| `context-upload` | callback | `(string  file-path )` |  |
| `context-save-local` | callback | `(string  file-path )` |  |
| `context-open-cloud` | callback | `(string  file-path )` |  |
| `context-retry` | callback | `()` |  |
| `delete-choose` | callback | `(int, bool)` | 0 = this PC, 1 = the cloud, 2 = everywhere; remember = don't ask again |
| `delete-cancel` | callback | `()` |  |
| `viewport-changed` | callback | `(float  scroll-y , float  width , float  height )` |  |

### PlayerVM

| Name | Kind | Type | Notes |
|---|---|---|---|
| `image` | in property | `image` |  |
| `has-clip` | in property | `bool` |  |
| `is-playing` | in property | `bool` |  |
| `is-loading` | in property | `bool` |  |
| `is-image` | in property | `bool` |  |
| `path` | in property | `string` |  |
| `title` | in property | `string` |  |
| `meta` | in property | `string` |  |
| `time-text` | in property | `string` |  |
| `position` | in-out property | `float` |  |
| `dragging` | in-out property | `bool` |  |
| `volume` | in-out property | `float` |  |
| `muted` | in property | `bool` |  |
| `speed` | in property | `float` |  |
| `audio-track` | in property | `int` |  |
| `audio-track-count` | in property | `int` |  |
| `clips` | in property | `[PlayerClip]` |  |
| `total-duration` | in property | `string` |  |
| `total-size` | in property | `string` |  |
| `error` | in property | `string` |  |
| `mute-flash` | in-out property | `bool` |  |
| `toggle` | callback | `()` |  |
| `stop` | callback | `()` |  |
| `seek` | callback | `(float, bool)` |  |
| `skip` | callback | `(float)` |  |
| `next` | callback | `()` |  |
| `play-clip` | callback | `(string)` |  |
| `edit` | callback | `(string)` |  |
| `volume-changed` | callback | `(float)` |  |
| `mute` | callback | `()` |  |
| `speed-changed` | callback | `(float)` |  |
| `audio-changed` | callback | `(int)` |  |
| `frame-step` | callback | `()` |  |
| `active-changed` | callback | `(bool)` |  |
| `fullscreen-changed` | callback | `(bool)` |  |
| `video-rect-changed` | callback | `(float, float, float, float)` |  |
| `pointer-left-video` | callback | `()` |  |
| `video-pointer` | callback | `(float, float)` | fullscreen: mouse moved over the video (the native surface passes hit-testing to this window) |
| `cursor-hidden` | in-out property | `bool` | fullscreen, overlay hidden: cursor hidden after idle |
| `synthetic-move` | callback | `() -> bool` | see PlayerOverlayVM.synthetic-move |

### EditorVM

| Name | Kind | Type | Notes |
|---|---|---|---|
| `has-clip` | in property | `bool` | Header & sidebar clips |
| `current-clip-path` | in property | `string` |  |
| `clip-name` | in property | `string` |  |
| `clips` | in property | `[EditorClipItem]` |  |
| `video-image` | in property | `image` | Preview & playback |
| `native-video` | in property | `bool` | set by Rust once the native video surface exists: the stage stays empty, the overlay window draws the rest |
| `stage-rect-changed` | callback | `(float, float, float, float)` | the stage's rect in the window (logical px): Rust places the native surface + overlay window there |
| `focus-serial` | in property | `int` | bumped by Rust when a click in the overlay window must give keyboard focus back to the page |
| `editing-text-index` | in-out property | `int` | caption being edited on the video (-1 = none); the Text tab's field takes the keyboard meanwhile |
| `text-focus-serial` | in-out property | `int` |  |
| `is-playing` | in property | `bool` |  |
| `position` | in-out property | `float` |  |
| `time-display` | in property | `string` |  |
| `duration-sec` | in property | `float` |  |
| `trim-start-sec` | in-out property | `float` |  |
| `trim-end-sec` | in-out property | `float` |  |
| `trim-start-display` | in property | `string` |  |
| `trim-end-display` | in property | `string` |  |
| `trim-duration-display` | in property | `string` |  |
| `playhead-fraction` | in property | `float` |  |
| `is-video-selected` | in-out property | `bool` |  |
| `timeline-thumbs` | in property | `[image]` |  |
| `cut-bands` | in property | `[EditorCutBand]` |  |
| `is-marking-cut` | in property | `bool` |  |
| `audio-tracks` | in property | `[EditorAudioTrackData]` |  |
| `audio-follows-video` | in-out property | `bool` |  |
| `set-audio-follows-video` | callback | `(bool)` |  |
| `display-source-aspect-raw` | in property | `float` | Stage reframe / crop / transform |
| `display-frame-aspect` | in property | `float` |  |
| `crop-tool-active` | in-out property | `bool` |  |
| `reframe-selected` | in-out property | `bool` |  |
| `edit-nx` | in property | `float` |  |
| `edit-ny` | in property | `float` |  |
| `edit-nw` | in property | `float` |  |
| `edit-nh` | in property | `float` |  |
| `selected-tools-tab` | in-out property | `int` | Tools panel |
| `aspect-options` | in property | `[ComboBoxItemData]` |  |
| `selected-aspect-index` | in-out property | `int` |  |
| `speed-options` | in property | `[SegmentedOption]` |  |
| `selected-speed-index` | in-out property | `int` |  |
| `speed-text` | in-out property | `string` |  |
| `filter-options` | in property | `[EditorFilterData]` |  |
| `selected-filter-index` | in-out property | `int` |  |
| `color-brightness` | in-out property | `int` |  |
| `color-contrast` | in-out property | `int` |  |
| `color-saturation` | in-out property | `int` |  |
| `texts` | in property | `[EditorTextOverlayData]` |  |
| `has-texts` | in property | `bool` |  |
| `has-selected-text` | in property | `bool` |  |
| `selected-text-content` | in-out property | `string` |  |
| `selected-text-font-percent` | in-out property | `int` |  |
| `text-colors` | in property | `[EditorColorSwatch]` |  |
| `{ hex` | field | `"#FFFFFF", swatch: #FFFFFF }` |  |
| `{ hex` | field | `"#000000", swatch: #000000 }` |  |
| `{ hex` | field | `"#FFD43B", swatch: #FFD43B }` |  |
| `{ hex` | field | `"#FF6B6B", swatch: #FF6B6B }` |  |
| `{ hex` | field | `"#4DABF7", swatch: #4DABF7 }` |  |
| `{ hex` | field | `"#51CF66", swatch: #51CF66 }` |  |
| `is-exporting` | in property | `bool` | Export |
| `export-progress` | in property | `int` |  |
| `export-status` | in property | `string` |  |
| `export-resolution-options` | in property | `[ComboBoxItemData]` |  |
| `selected-export-resolution-index` | in-out property | `int` |  |
| `export-format-options` | in property | `[ComboBoxItemData]` |  |
| `selected-export-format-index` | in-out property | `int` |  |
| `export-as-gif` | in-out property | `bool` |  |
| `export-flyout-open` | in-out property | `bool` |  |
| `open-clip` | callback | `(string)` | Callbacks (UI -> Rust) |
| `import-clip` | callback | `()` |  |
| `merge-mode` | in-out property | `bool` | clip merging: picking mode in the sidebar, picked count, start |
| `merge-count` | in property | `int` |  |
| `toggle-merge-mode` | callback | `()` |  |
| `merge-pick` | callback | `(string)` |  |
| `merge-clips` | callback | `()` |  |
| `montage-segments` | in property | `[EditorSegmentData]` | montage: the strip's segments, shown when there is more than one; total length and the montage clock |
| `montage-active` | in property | `bool` |  |
| `montage-total` | in property | `string` |  |
| `montage-time` | in property | `string` |  |
| `select-segment` | callback | `(int)` |  |
| `segment-dropped` | callback | `(int, length, length)` |  |
| `split-segment` | callback | `()` |  |
| `remove-segment` | callback | `()` |  |
| `play-montage` | callback | `()` |  |
| `add-to-montage` | callback | `(string)` |  |
| `add-montage-file` | callback | `()` |  |
| `strip-resized` | callback | `(length)` |  |
| `back` | callback | `()` |  |
| `active-changed` | callback | `(bool)` |  |
| `play-pause` | callback | `()` |  |
| `undo` | callback | `()` |  |
| `push-undo` | callback | `()` |  |
| `set-trim-start-here` | callback | `()` |  |
| `set-trim-end-here` | callback | `()` |  |
| `toggle-cut` | callback | `()` |  |
| `reset-trim` | callback | `()` |  |
| `remove-cut-at` | callback | `(float)` |  |
| `timeline-drag` | callback | `(int, float, float)` |  |
| `timeline-drag-end` | callback | `()` |  |
| `toggle-video-selected` | callback | `()` |  |
| `toggle-audio-selected` | callback | `(int)` |  |
| `set-audio-muted` | callback | `(int, bool)` |  |
| `set-audio-volume` | callback | `(int, int)` |  |
| `set-audio-range` | callback | `(int, float, float)` |  |
| `select-aspect` | callback | `(int)` |  |
| `toggle-crop-tool` | callback | `()` |  |
| `right-click-reset` | callback | `()` |  |
| `begin-overlay-drag` | callback | `()` |  |
| `drag-overlay` | callback | `(int, float, float, float, float, float, float, float, float)` |  |
| `rotate-left` | callback | `()` |  |
| `rotate-right` | callback | `()` |  |
| `toggle-flip-h` | callback | `()` |  |
| `toggle-flip-v` | callback | `()` |  |
| `select-speed-preset` | callback | `(int)` |  |
| `speed-text-edited` | callback | `(string)` |  |
| `select-filter` | callback | `(int)` |  |
| `color-changed` | callback | `(int, int, int)` |  |
| `reset-color` | callback | `()` |  |
| `add-text` | callback | `()` |  |
| `select-text` | callback | `(int)` |  |
| `update-selected-text` | callback | `(string)` |  |
| `update-selected-text-size` | callback | `(int)` |  |
| `set-text-color` | callback | `(string)` |  |
| `move-text` | callback | `(int, float, float)` |  |
| `remove-selected-text` | callback | `()` |  |
| `select-export-resolution` | callback | `(int)` |  |
| `select-export-format` | callback | `(int)` |  |
| `set-export-gif` | callback | `(bool)` |  |
| `export-clip` | callback | `()` |  |
| `cancel-export` | callback | `()` |  |

### PlayerOverlayVM

| Name | Kind | Type | Notes |
|---|---|---|---|
| `has-clip` | in property | `bool` |  |
| `is-playing` | in property | `bool` |  |
| `is-loading` | in property | `bool` |  |
| `is-image` | in property | `bool` |  |
| `is-fullscreen` | in property | `bool` |  |
| `path` | in property | `string` |  |
| `time-text` | in property | `string` |  |
| `position` | in-out property | `float` |  |
| `dragging` | in-out property | `bool` |  |
| `volume` | in-out property | `float` |  |
| `muted` | in property | `bool` |  |
| `speed` | in property | `float` |  |
| `smooth-slowmo` | in property | `bool` |  |
| `audio-track` | in property | `int` |  |
| `audio-track-count` | in property | `int` |  |
| `error` | in property | `string` |  |
| `mute-flash` | in-out property | `bool` |  |
| `controls` | in-out property | `bool` |  |
| `cursor-idle` | in-out property | `bool` |  |
| `sidebar` | in-out property | `bool` |  |
| `clips` | in property | `[PlayerClip]` |  |
| `total-duration` | in property | `string` |  |
| `total-size` | in property | `string` |  |
| `library-loading` | in property | `bool` |  |
| `toggle` | callback | `()` |  |
| `stop` | callback | `()` |  |
| `seek` | callback | `(float, bool)` |  |
| `skip` | callback | `(float)` |  |
| `next` | callback | `()` |  |
| `play-clip` | callback | `(string)` |  |
| `edit` | callback | `(string)` |  |
| `show-in-folder` | callback | `(string)` |  |
| `delete-clip` | callback | `(string)` |  |
| `volume-changed` | callback | `(float)` |  |
| `mute` | callback | `()` |  |
| `speed-changed` | callback | `(float)` |  |
| `smooth-slowmo-changed` | callback | `(bool)` |  |
| `audio-changed` | callback | `(int)` |  |
| `toggle-fullscreen` | callback | `()` |  |
| `exit-fullscreen` | callback | `()` |  |
| `dragging-changed` | callback | `(bool)` |  |
| `pinned-changed` | callback | `(bool)` |  |
| `synthetic-move` | callback | `() -> bool` | true when the OS cursor did not actually move since the last pointer event (Windows sends a fake WM_MOUSEMOVE whenever a window under the cursor is shown/hidden/moved): such events must not wake the UI |

### EditorOverlayVM

| Name | Kind | Type | Notes |
|---|---|---|---|
| `has-clip` | in property | `bool` |  |
| `display-source-aspect-raw` | in property | `float` |  |
| `display-frame-aspect` | in property | `float` |  |
| `crop-tool-active` | in property | `bool` |  |
| `reframe-selected` | in property | `bool` |  |
| `edit-nx` | in property | `float` |  |
| `edit-ny` | in property | `float` |  |
| `edit-nw` | in property | `float` |  |
| `edit-nh` | in property | `float` |  |
| `texts` | in property | `[EditorTextOverlayData]` |  |
| `focus-keys` | callback | `()` |  |
| `play-pause` | callback | `()` |  |
| `deselect-reframe` | callback | `()` |  |
| `select-reframe` | callback | `()` |  |
| `right-click-reset` | callback | `()` |  |
| `begin-overlay-drag` | callback | `()` |  |
| `drag-overlay` | callback | `(int, float, float, float, float, float, float, float, float)` |  |
| `select-text` | callback | `(int)` |  |
| `move-text` | callback | `(int, float, float)` |  |
| `push-undo` | callback | `()` |  |
| `editing-index` | in-out property | `int` |  |
| `text-edit-started` | callback | `(int)` |  |
| `text-edit-finished` | callback | `()` |  |
| `deselect-text` | callback | `()` |  |

### AutoScroll

Middle-button autoscroll, as in a browser (owner, 2026-11-21: "hold the wheel and scroll fast in Settings"). Rust (shell/autoscroll.rs) sees the middle button at window level, before any button under the cursor, and fills this global in window coordinates; the scroll view under the press point claims it and scrolls itself.

| Name | Kind | Type | Notes |
|---|---|---|---|
| `active` | in-out property | `bool` |  |
| `origin-x` | in-out property | `length` |  |
| `origin-y` | in-out property | `length` |  |
| `mouse-x` | in-out property | `length` |  |
| `mouse-y` | in-out property | `length` |  |
| `owner-area` | in-out property | `float` | the claiming view: the smallest live, scrollable one that contains the origin (0 = nobody yet) |
| `owner-x` | in-out property | `length` |  |
| `owner-y` | in-out property | `length` |  |
| `pending` | in-out property | `float` | distance (px, + = down) Rust has measured for the owner since it last scrolled; the owner consumes it |

## Data types

### ComboBoxItemData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `text` | field | `string` |  |
| `tier` | field | `int` | 0 = normal, 1 = caution (#E5B84C), >= 2 = extreme (#E0564C) |

### SegmentedOption

| Name | Kind | Type | Notes |
|---|---|---|---|
| `text` | field | `string` |  |
| `tier` | field | `int` | 0 = default, 1 = caution, >= 2 = extreme |

### OverlayBoxData

| Name | Type |
|---|---|
| `xf` | `float` |
| `yf` | `float` |
| `scale` | `float` |
| `shown` | `bool` |

### EditorClipItem

| Name | Kind | Type | Notes |
|---|---|---|---|
| `path` | field | `string` |  |
| `title` | field | `string` |  |
| `meta` | field | `string` |  |
| `thumbnail` | field | `image` |  |
| `has-thumbnail` | field | `bool` |  |
| `merge-number` | field | `int` | position in the clip-merge order (1-based), 0 = not picked |

### EditorSegmentData

one block of the montage strip

| Name | Kind | Type | Notes |
|---|---|---|---|
| `id` | field | `int` |  |
| `title` | field | `string` |  |
| `duration` | field | `string` |  |
| `x` | field | `length` | block position and width in the strip (computed in Rust: montage::block_widths) |
| `w` | field | `length` |  |
| `thumbnail` | field | `image` |  |
| `has-thumbnail` | field | `bool` |  |
| `selected` | field | `bool` |  |

### EditorAudioTrackData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `stream-index` | field | `int` |  |
| `name` | field | `string` |  |
| `volume` | field | `int` |  |
| `is-muted` | field | `bool` |  |
| `is-selected` | field | `bool` |  |
| `start-sec` | field | `float` |  |
| `end-sec` | field | `float` |  |
| `clip-duration-sec` | field | `float` |  |

### EditorFilterData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `name` | field | `string` |  |
| `preview` | field | `image` |  |
| `has-preview` | field | `bool` |  |

### EditorCutBand

| Name | Kind | Type | Notes |
|---|---|---|---|
| `start-sec` | field | `float` |  |
| `end-sec` | field | `float` |  |
| `is-pending` | field | `bool` |  |
| `cut-index` | field | `int` | -1 for pending |

### EditorTextOverlayData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `text` | field | `string` |  |
| `nx` | field | `float` |  |
| `ny` | field | `float` |  |
| `font-percent` | field | `int` |  |
| `text-color` | field | `color` |  |
| `hex` | field | `string` |  |
| `selected` | field | `bool` |  |

### EditorColorSwatch

| Name | Kind | Type | Notes |
|---|---|---|---|
| `hex` | field | `string` |  |
| `swatch` | field | `color` |  |

### LibraryCardData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `clip-index` | field | `int` |  |
| `file-path` | field | `string` |  |
| `title` | field | `string` |  |
| `time-text` | field | `string` |  |
| `game-name` | field | `string` |  |
| `duration-text` | field | `string` |  |
| `thumbnail` | field | `image` |  |
| `has-thumbnail` | field | `bool` |  |
| `is-image` | field | `bool` |  |
| `is-favorite` | field | `bool` |  |
| `is-edited` | field | `bool` |  |
| `is-selected` | field | `bool` |  |
| `cloud-state` | field | `int` | cloud (docs/spec/cloud.md §5): 0 only here, 1 here + cloud, 2 only in the cloud, 3 uploading / queued, 4 downloading, 5 failed; progress 0..1, below 0 = waiting |
| `cloud-progress` | field | `float` |  |
| `cloud-only` | field | `bool` |  |

### SettingsRowData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `id` | field | `string, label: string, description: string, kind: int` |  |
| `options` | field | `[ComboBoxItemData], segments: [SegmentedOption], selected: int` |  |
| `checked` | field | `bool, value: float, maximum: float, display: string` |  |
| `enabled` | field | `bool, visible: bool, divider: bool, custom: bool, auto-mode: bool` |  |
| `with-toggle` | field | `bool, toggle-id: string, parts: [string]` |  |
| `icon` | field | `image` | audio app rows (kind 4): the app's own icon (C# AppAudioItem.Icon) |

### SettingsGroup

| Name | Type |
|---|---|
| `title` | `string` |
| `rows` | `[SettingsRowData]` |

### PresetData

| Name | Type |
|---|---|
| `title` | `string` |
| `description` | `string` |
| `summary` | `string` |
| `active` | `bool` |

### GameChipData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `label` | field | `string` |  |
| `filter-key` | field | `string` |  |
| `is-active` | field | `bool` |  |

### LibraryRowData

| Name | Kind | Type | Notes |
|---|---|---|---|
| `kind` | field | `int` | 0 = group header, 1 = card row |
| `y` | field | `float` |  |
| `height` | field | `float` |  |
| `header-label` | field | `string` |  |
| `header-sub` | field | `string` |  |
| `header-count` | field | `string` |  |
| `cards` | field | `[LibraryCardData]` |  |

### PlayerClip

| Name | Kind | Type | Notes |
|---|---|---|---|
| `path` | field | `string, title: string, meta: string, header-meta: string, thumbnail: image, is-image: bool` |  |

### Page

MainViewModel (ViewModels/MainViewModel.cs, spec shell.md §4.2). Rust owns the logic.

| Name | Type |
|---|---|
| `settings` | `` |
| `library` | `` |
| `player` | `` |
| `editor` | `` |
