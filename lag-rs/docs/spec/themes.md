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

## 12. Phase 1 design: the UI twin (started 2026-11-22)
Problem: Lag's Rust code reaches the UI through ~270 typed calls that `slint-build` generates at build time
(`ui.global::<MainVM>().set_is_recording(..)`, `on_save_replay(..)`, structs in models, the `Page` enum). The
interpreter's API is by name and `Value`. Rewriting 270 call sites by hand would be slow and error-prone.

Solution: `crates/lag-app/build_twin.rs` (build script) compiles `ui/app.slint` with the interpreter's compiler
(full type info, `slint-interpreter` "internal" feature, pinned `=1.18.1`) and GENERATES `$OUT_DIR/ui_twin.rs`: the
same typed API, method for method, on top of `slint_interpreter::ComponentInstance`. `src/ui_twin/rt.rs` is the
hand-written runtime: value conversions (numbers, strings, colours, brushes, images, models of structs both ways),
`TwinGlobal` / `TwinComponent` (`new`, `global`, `window`, `show`, `hide`, `run`, `as_weak`, `clone_strong`), a
`Weak` with `upgrade` / `upgrade_in_event_loop`, and `load(entry, api_dir)` which compiles a UI once and makes its
windows available. A name a theme lacks is logged once (`[Theme] ...`) and returns a default, never a crash.

Generated today: 9 globals, 6 windows, 17 structs, 1 enum, 716 methods (1261 lines). Test
`ui_twin::tests::the_twin_drives_the_interpreted_ui`: Lag's whole UI loaded at run time and driven through the twin
(typed properties, the `Page` enum, a UI callback reaching Rust with a typed argument, a pure callback Rust implements
for translations, a model of structs holding a model of structs, read back).

Next steps of phase 1:
1. Switch the app: `slint::include_modules!()` -> `pub use ui_twin::*`; `slint::ComponentHandle` / `slint::Weak`
   uses -> the twin's; `rt::load` at start-up from a `ui` folder shipped next to Lag.exe (dev: the source folder).
2. `--ui-check` / `--gpu-check` on the twin; every screenshot identical to the compiled build.
3. Measure start-up time and memory, compiled vs interpreted, on the owner's PC; decide on caching if needed.
4. Remove the compiled path (`slint-build`) once the twin is the only one.

### Status 2026-11-22 (later)
- DONE step 1: the app runs on the twin. `slint::include_modules!()` and `slint-build` are gone; `main.rs`
  re-exports the twin (`pub use ui_twin::*`, `TwinComponent as ComponentHandle`, `crate::Weak`), 37 files changed
  only their imports. `ui_twin::load_builtin()` compiles `ui\app.slint` first thing in `main` (also for
  `--ui-check` / `--snapshot` / `--gpu-check`); a failure shows an error box and logs `[Startup] UI failed to load`.
  The in-game stats HUD (`StatsHud`, was a separate `slint!` macro) is part of the themed UI now.
  `tools/package.ps1` ships `ui\` (111 files, 3 MB) next to Lag.exe; the exe shrank 31.2 -> 27.8 MB.
- DONE step 2: `--ui-check` full (53) + languages (119) on the twin, compared pixel by pixel with the compiled
  build's shots at the same theme: every difference is data (cloud preview cards created "now" swap places, the
  disk usage grew, audio levels), no layout / colour / text / font difference. A packaged folder renders from its
  own `ui\`.
- Step 3 (measure on the owner's PC): the log line `[Startup] UI loaded from ... in N ms` on a real launch.
- Step 4 is done together with step 1 (no compiled path left).

## 13. Phase 2: theme packages (2026-11-22, first cut DONE)
- `ui/api.slint`: the theme API v1 = re-exports of the globals Lag drives (`Tr, MainVM, Page, WindowMetrics,
  SettingsVM, LibraryVM, PlayerVM, EditorVM, PlayerOverlayVM, EditorOverlayVM, AutoScroll`) and the data structs.
  Themes `import { ... } from "@lag/api.slint"`; Lag's building blocks are reachable as `@lag/components/...`,
  tokens as `@lag/theme/tokens.slint`.
- `src/themes.rs`: a theme = `%APPDATA%\Lag\themes\<id>\theme.toml` (`name`, `author`, `version`, `api`,
  `entry` = app.slint) + files. `activate(id)` loads Lag's own UI first, then the theme's windows over it (two
  passes: find the windows the theme exports, then compile a generated wrapper that also re-exports the API
  globals). A window the theme does not draw stays Lag's. Any failure -> Lag's own UI + `last_error()` (shown in
  Settings in orange, full text in the log). `api` newer than Lag's -> refused with a clear message.
- Safety (`validate`): only theme file types (.slint .toml .md .txt .png .jpg .jpeg .svg .gif .webp .ttf .otf),
  no links, every path in a `.slint` (`import ... from`, `import "font"`, `@image-url`) is `@lag/...` or inside
  the package (no `..`, no drive, no absolute), size limits (32 MB a file, 96 MB, 2000 files). `.lagtheme` = zip;
  `unpack` refuses zip-slip (`enclosed_name`). Tests: path rules, package check, export -> import round trip,
  a crafted `../../evil.slint` refused, a theme replacing only `AppWindow` while the API globals stay reachable.
- Settings > General: "Interface" (Lag (built-in) + installed themes), buttons Import... / Export... / Themes
  folder, and Restart when the chosen theme is not the running one (`shell::restart`: a new Lag waits up to 10 s
  for the old one's single-instance lock, `--after-restart`). Settings key `UiTheme` (None = Lag's own).
- The way back always exists: the tray menu stays Lag's (themes cannot replace `TrayMenuWindow` for now) and shows
  "Lag's own interface" while a custom theme runs (resets `UiTheme`, restarts); holding Shift while Lag starts
  uses Lag's own interface for that run.
- Example theme: `lag-rs/docs/themes/example-ring/` (a buffer ring + a sticker-shaped Save button). Verified: with
  it installed and chosen, Lag starts drawing it (`--snapshot`); `Tr.t` and `MainVM` work; Lag logs once that the
  theme lacks `theme-mode` (not an error).
- Not yet: live reload without a restart (needs a hot swap of the main window: re-run `views::init_all` on the
  new window, drop the old one's callbacks), the tray menu theming (owner wants it: phase 3, keeping a mandatory
  way back), API reference docs, more example themes.

## 14. Phase 3 (started 2026-11-22): theme preview with live reload — DONE
`Lag.exe --theme-preview <theme folder>` (Settings > General > Interface > Preview): a separate Lag window drawing
the theme over real data, without recording, tray, hotkeys or cloud, never saving settings (safe next to the
running Lag). It polls the folder (400 ms, size + time of every file); after a save it starts a fresh preview at
the same place (`--at x,y,w,h`), which, once drawn, takes the old window's place in the z-order and ends the old
process (`--replace pid,hwnd`): no gap, and it never falls behind other windows (Windows' foreground lock would
otherwise keep a new background process's window under the editor or browser). A theme that does not compile
shows `ThemeErrorWindow` (the compiler's "file:line: message", paths relative to the theme); the fix reloads it.
Verified in real windows: start -> edit (button colour changed) -> syntax error (error window) -> fix (theme back),
always exactly one preview process, none left after closing; the owner's settings untouched.

Next in phase 3: API globals in their own `ui/api/` files so a copy of Lag's own UI can be a starting theme
("New theme from Lag's"), a generated API reference for theme authors, more example themes, hot swap of the main
window in the running app (no restart), themable tray menu with a mandatory way back.

## 15. Phase 3, part 2 (2026-11-22): API split, "New theme", API reference — DONE
- Every global Lag drives and every struct they carry now lives in `ui/api/globals.slint` / `ui/api/types.slint`
  (moved out of the views / components; those files import + re-export them, so nothing else changed). Why: a
  theme that is a copy of Lag's UI would otherwise get its OWN copies of MainVM, SettingsVM... (a different file =
  a different global) and Lag's Rust code would drive Lag's, not the theme's: a dead UI.
- `themes::new_from_builtin(root, name)` (Settings > Interface > **New theme**): copies `ui\` (without `api/`,
  `api.slint`, the dev gallery) into `%APPDATA%\Lag\themes\<id>\`, rewrites every import that leads into `api/`
  to `@lag/api/...`, writes theme.toml, validates, selects it and opens the folder. Never over an existing theme
  (`-2`, `-3`...). Verified: the full ui-check (53 shots) of Lag running such a copy as its theme is pixel-identical
  to Lag's own UI except data (the theme name in the picker, the record dot's blink phase).
- Path rules are package-aware now: `../theme/tokens.slint` inside the package is fine, anything resolving above
  the package root, absolute, a drive or another `@library` is refused; comments are ignored when looking for
  imports.
- API reference: `build_twin::api_reference` reads `ui/api/*.slint` (comments as descriptions) at build time ->
  `docs/themes/API.md` (test `the_api_reference_is_current`, `LAG_UPDATE_API_DOC=1` refreshes it); Lag writes it as
  `API.md` into the themes folder (New theme / Themes folder).

## 16. Phase 4 (2026-11-22): graphical editor — in progress, ships together with themes in 2.2.0
Owner: "ми робимо і вносимо редактор в одне оновлення" — no themes release without the editor.
- `Lag.exe --theme-editor <theme folder>` (Settings > Interface > **Edit**): the theme runs live in the middle (a
  `ComponentFactory` into a `ComponentContainer`), with Lag's real data: a hidden "donor" AppWindow driven by Lag's
  own code (`never_save`) whose API globals are mirrored into the theme every 700 ms (the page stays the editor's).
- **Element tab.** A click picks the innermost element of the theme's files under the pointer, a second click on
  the same spot the one around it. Picking uses a vendored slint-interpreter 1.18.1 (`vendor/slint-interpreter`,
  `LAG PATCH`: `element_sources_at(x, y)`, one pass over what is on screen, ~25 ms on Lag's whole UI instead of
  ~75 s through the public per-element API). The panel shows where the element is written, "part of X: changes
  every X" for component bodies, the path to it, every property its type has with what the file says, and
  ↑ ↓ / duplicate (inner ids renamed) / delete / + shape / + text; dragging moves it (not inside layouts).
- **Palette tab.** Every `color` / `brush` property of a global in the theme's own files (Lag's copy: `Theme` in
  `theme/tokens.slint`, 150+ colours), grouped by the `// ── Section ──` comments. A mode switch (dark / light /
  GTA VI) sets the canvas' mode; a change goes only into the branch of `is-neon ? A : is-dark ? B : C` that mode
  shows, so a theme keeps its light and dark looks apart. "Same in every mode" puts one colour in place of the
  whole expression. References are followed for the swatch (`page-bg: surface-app`), gradients show their first
  colour; a colour that hangs on something other than the mode is refused with a hint. Element properties that
  name a palette colour (`Theme.on-accent`) show its swatch too.
- Every change is a text edit of the `.slint` file, compiled at once; a change that does not compile is put back
  and the compiler's message shown. Undo / redo keep whole-file texts.
- Checks without the mouse: `--editor-script <file>` (`pick X Y`, `set`, `reset`, `do OP`, `drag`, `tab`, `mode`,
  `allmodes`, `token NAME VALUE`, `tokens FILTER`, `undo`, `redo`, `wait`, `shot`, `quit`); with
  `--editor-shots <dir>` the editor runs in memory (software renderer, no window — safe while a game runs) and
  `shot NAME` writes PNGs.
- Still to do before 2.2.0: enum dropdowns, a colour picker, an image picker, then the full UI check.
