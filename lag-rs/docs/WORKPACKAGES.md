# UI work packages (lag-app) — who owns which files

Read first: `docs/AGENTS.md` (hard rules: quiet.exe for every cargo command, NO windows/GUI runs, no git),
`docs/ARCHITECTURE.md` (threads, VM bridge, no-focus windows), the spec of your area in `docs/spec/`.
Reference C# (read-only): `E:\project\InstantReplay\InstantReplay`. Design system to build with:
`crates/lag-app/ui/theme/*`, `crates/lag-app/ui/components/*` (see `ui/README.md`).

## Shared contracts — DO NOT EDIT (ask in your report if you need a change)
- `ui/globals.slint` (`Tr`, `MainVM`, `Page`), `src/app_ctx.rs` (settings store + events), `src/main.rs`,
  `src/views/mod.rs`, `ui/theme/*`, `ui/components/*` (you may ADD new component files under
  `ui/components/<your-area>/`), `crates/lag-capture`, `crates/lag-core` (except: settings package may add
  NEW localisation keys to `crates/lag-core/langs/*.json`, all 17 files, additive only).
- Localisation: every user-visible string goes through `Tr.t("<C# key>", Tr.rev)`; keys are the C# keys.
- Settings: read via `ctx.settings()`, change via `ctx.update_settings(|s| ...)` (debounced save), react via
  `ctx.on_settings_changed`. Cross-page: `ctx.play_clip / edit_clip / request_library_refresh` + `on_*`.
- MainVM callbacks (`invoke_save_replay()`, `invoke_toggle_recording()`, ...) are how tray/hotkeys trigger
  actions; shell-A implements them.

## Packages
| package | owns (only these files) |
|---|---|
| shell-A | `ui/app.slint` (chrome 1:1, keep the page-host contract), `ui/toast.slint`, `src/views/main_vm.rs`, `src/services/**`, `src/shell/toast.rs`, `src/shell/nofocus.rs` |
| shell-B | `src/shell/mod.rs` (`init`), `src/shell/tray.rs`, `src/shell/hotkeys.rs`, `src/shell/hud.rs`, `ui/hud.slint`, `src/shell/updater.rs`, `src/shell/autostart.rs` |
| settings | `ui/views/settings.slint`, `src/views/settings.rs`, `ui/components/settings/**`, new keys in `crates/lag-core/langs/*.json` |
| library | `ui/views/library.slint`, `src/views/library.rs`, `ui/components/library/**` |
| player | `crates/lag-player/**`, `ui/views/player.slint`, `src/views/player.rs` |
| editor | `ui/views/editor.slint`, `src/views/editor.rs`, `ui/components/editor/**` |

Cargo: you may add dependencies/features to `crates/lag-app/Cargo.toml` INSIDE the single existing `[dependencies]` table (append only, never remove/reorder others'). NEVER add a second `[dependencies]` or `[target.*.dependencies]` table: duplicate keys broke every cargo command once.

## Done means
`quiet.exe cargo build -p lag-app` and `quiet.exe cargo test -p lag-app` pass; clippy clean in your files;
every item of your spec's §10 parity checklist is either implemented or listed as open in your report;
C# bugs you fixed listed with file:line. Never launch the app.
