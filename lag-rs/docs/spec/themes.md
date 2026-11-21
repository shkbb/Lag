# Custom interface (themes) — plan

Status: PLAN + working spike (2026-11-22), owner's answers in §11. Next: phase 1.

## 1. What the owner wants
"Let people change the interface completely: move everything (buttons, indicators), remove, add, make their own —
a dedicated editor for it, and import / export of a theme file." Not just dragging buttons: the freedom people have
with Hyprland "rices" on Linux, "absolutely anything" — any layout, shapes, widgets, art, animations. Both a
**graphical editor** and a **theme written as code** in a file that can be imported and exported.

## 2. The idea in one paragraph
Lag's whole interface becomes a *theme*: a small package of `.slint` files (Slint's own declarative UI language) plus
pictures and fonts. Lag compiles the active theme **at run time** with `slint-interpreter` and talks to it only
through a fixed, documented **theme API** — state goes in (recording, buffer, clips, game, settings…), actions come
out (save replay, start / stop, play, delete…). Today's interface becomes the built-in theme "Lag" in exactly that
format (dogfooding): whatever the built-in UI can do, any theme can do. The graphical editor writes the same files.

## 3. Spike results (`crates/lag-app/examples/theme_spike.rs`, headless, release build, this PC)
| measured | result |
|---|---|
| compile Lag's WHOLE current UI (10.5k lines `.slint`) with the interpreter | **239 ms** |
| create the window's components | 16 ms |
| first frame / next frames (software renderer, 1360×840) | 3 ms / 1.0 ms avg |
| working set with the interpreted full UI | 141 MB (to compare with the compiled build in phase 1) |
| compile + first frame of a separate theme file | **3 ms + 4 ms** |
| a click on the theme's own button reached Lag's Rust code | yes (`MainVM.save-replay`) |
| Lag changed state → the theme followed (ring colour, fill, text) | yes |
| swap theme → built-in UI again, no restart | 245 ms |

Pictures of the spike: the built-in UI running interpreted, and `examples/themes/spike-rice.slint` — a layout with
nothing in common with Lag's (a buffer ring + a sticker-shaped button) driven by the same globals.

Found on the way: Slint lets Rust reach a global only if the compiled file EXPORTS it. Lag therefore wraps every
theme in a generated file that re-exports the API globals (`export { AppWindow } from "<theme>"; export { MainVM, … }
from "@lag/…"`): theme authors only import what they use. `@lag/…` is a library path Lag maps to its API files.

## 4. Theme API (versioned)
- `@lag/api.slint` — the globals: `MainVM` (shell: recording, buffer, status, capture target, navigation, window
  chrome), `LibraryVM`, `PlayerVM`, `EditorVM`, `SettingsVM`, `Tr` (translations), `Theme` (tokens: colours, fonts,
  radii, durations), `AutoScroll`, `WindowMetrics`. Exactly today's globals, cleaned up and documented.
- `@lag/kit.slint` — Lag's building blocks, free to use or ignore: buttons, chips, toggles, sliders, dropdowns,
  clip card, game chips, buffer bar, timeline, settings rows, **VideoSurface** (the player / editor video is a
  native mpv surface: a theme places this component and Lag puts the video under its rectangle).
- `theme.toml` manifest: name, author, version, `api = 1`, entry file, preview picture, optional variants
  (e.g. dark / light of the same theme).
- Versioning: `api = N`. Lag keeps every published API version working (old names stay as aliases); a theme for a
  newer API than the app is refused with a clear message.
- Reference docs generated from the API files (every property / callback with a sentence), shipped with Lag.

## 5. Three levels, one format
1. **Look** — tokens: colours, gradients, fonts, corner radii, wallpaper, animation speed. Graphical editor.
2. **Layout** — move / resize / hide / add any kit widget anywhere, per page. Graphical editor (drag on a live
   preview), saved as generated `.slint`.
3. **Code** — any `.slint`: own widgets, shapes (paths), art, animations, completely new pages. Text editor with
   live reload and errors shown at the line.

## 6. Packages, import / export, safety
- A theme = a folder in `%APPDATA%\Lag\themes\<name>\`; export / import = one `.lagtheme` file (zip of that folder).
- Live reload: Lag watches the active theme's folder and recompiles on save (a theme compiles in a few ms).
- Safety: `.slint` is declarative — no code execution, no file or network access. Lag still checks a package
  before loading it: imports only from `@lag/…` or inside the package, images / fonts only inside the package,
  size limits. A theme that fails to compile never breaks Lag: it falls back to the built-in theme and shows the
  error (file, line, message).
- Settings → General → Theme lists installed themes (+ variants); today's Dark / Light / System / GTA VI become
  variants of the built-in theme.

## 7. Graphical editor (in Lag)
Opens the current theme on a live preview with sample data (recording, clips, game): pick an element → its
properties (position, size, colours, font, radius, visibility, animation); drag / resize on the canvas; "add widget"
from the kit; per-page; token panel; a code tab with the generated `.slint` (editable — hand edits are kept);
undo / redo; save, export `.lagtheme`. Everything it does is written as normal theme files.

## 8. Phases (rough effort)
| phase | what | size |
|---|---|---|
| 0 | spike: interpreted full UI, theme file, state in / action out, swap (DONE) | done |
| 1 | **Dynamic binding**: Lag's Rust code talks to the UI by names through one `ui_bridge` layer instead of 270 typed `global::<X>()` calls; the built-in UI is embedded as source and compiled at start; `--ui-check` keeps working; behaviour identical. Measure start time / memory vs compiled. | large |
| 2 | **Theme packages**: folder + manifest, wrapper, `@lag/` paths, safety checks, fallback + error display, live reload, import / export, theme picker; built-in themes as variants | medium |
| 3 | **API + kit**: clean the globals into `@lag/api`, split reusable widgets into `@lag/kit`, VideoSurface, generated reference, 3–4 example themes that look nothing like Lag (proof of "anything") | medium |
| 4 | **Graphical editor** (§7) | large |
| 5 | (later) overlay windows (HUD, toasts, tray menu) themable too; optional online gallery | later |

The Linux port benefits: the interpreter and the theme system are platform-independent.

## 9. Limits and risks (honest)
- "Anything" = anything Slint can draw: rectangles, paths / vector shapes, images, text, gradients, clipping,
  opacity, rotation, drop shadows, property animations, states. No custom GPU shaders or backdrop blur.
- Start-up: +~0.25 s compile of the built-in UI (the splash already covers start-up); custom themes add a few ms.
- Typed Rust ↔ UI checks move from compile time to start time: the bridge verifies at load that a theme exposes the
  API it needs (missing pieces are reported, not crashes); tests load every shipped theme.
- Fractional DPI (owner at 125 %): `transform-scale` / rotation make FemtoVG redraw neighbours shifted by a
  sub-pixel. Fine for themes that want it, documented as a caveat; Lag's own theme keeps avoiding it.

## 10. Questions for the owner
1. Should a theme be able to replace EVERY page (settings, library, player, editor), or only the main shell +
   library at first? (Plan: every page, because the built-in UI itself becomes a theme.)
2. Today's Dark / Light / System / GTA VI → variants of the built-in theme in the new picker. OK?
3. Sharing: files only (import / export `.lagtheme`), or later a gallery inside Lag to browse and install themes?
4. Should themes also restyle the separate windows (in-game HUD, toasts, tray menu, player fullscreen overlay)?
5. Order: code + live reload first (fast, the editor builds on it), then the graphical editor — or editor first?

## 11. Owner's answers (2026-11-22)
1. A theme replaces **every** page.
2. Yes: Dark / Light / System / GTA VI become variants of the built-in theme. And **System is the main theme now**,
   GTA VI is no longer the default (done in 2.1.2: `SettingsData::introduce_system_theme`, once; users on GTA VI
   because of 2.0's switch go to System, later choices stick).
3. Sharing by **files only** for now (`.lagtheme`): there is no server or budget for a hosted gallery. Keep the
   format ready for a gallery later (manifest has name / author / version / preview).
4. **Yes**: themes restyle the separate windows too (in-game HUD, toasts, tray menu, fullscreen player overlay) —
   moved from "later" into phase 3.
5. **Code + live reload first**, the graphical editor after (phases as in §8).
