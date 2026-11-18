# Rules for every agent working on lag-rs (Claude, codex, agy)

Goal: rewrite Lag (C#, `E:\project\InstantReplay\InstantReplay\`) 1:1 in Rust. Same look, same behaviour,
same files on disk. Fix real bugs you find (list each in your report with the C# file:line), optimise where
it is safe, but NEVER change user-visible behaviour silently. "Nothing breaks" beats "clever".

## Hard rules
0. **Run EVERY cargo/test/example command through the quiet wrapper**, e.g.
   `E:\project\InstantReplay\lag-rs\tools\bin\quiet.exe cargo test -p lag-core`
   (bash: `/e/project/InstantReplay/lag-rs/tools/bin/quiet.exe cargo test -p lag-core`).
   It disables Windows crash dialogs for the command and all its children. Without it, a crashing test
   (access violation in FFI code) pops an "Application Error" window over the user's game. This already
   happened once (lag_core test binary, 2026-11-16). If a test crashes with STATUS_ACCESS_VIOLATION
   (exit 0xC0000005), that is a real memory-safety bug in your unsafe/FFI code: find and fix it.
1. **Never open a window or launch a GUI process** (no running Lag.exe, no Slint previews, no `cargo run` of
   lag-app). The user is playing games on this PC; a window steals focus. Headless tests/console only.
2. **No mouse/keyboard simulation, no git commits/pushes, no deleting files outside your package.**
3. Touch ONLY the paths your task names. The C# tree is read-only reference.
4. Shared dependency versions live in `lag-rs/Cargo.toml` `[workspace.dependencies]`; use
   `dep = { workspace = true, features = [...] }`. Do not edit the root Cargo.toml; if you need a new shared
   dependency, add it to your crate only and mention it in your report.
5. Toolchain is `stable-x86_64-pc-windows-gnu` (no MSVC). Don't add crates that require MSVC or a C++ build.

## Conventions
- Logging: the `log` crate facade (`log::info!`, `log::warn!`, `log::error!`). Keep the C# message text
  VERBATIM including prefixes like `[MicDenoiser]`, `[stall]`, `[VFR]` — the user greps logs with old tools.
  The backend (lag-core::filelog) formats lines exactly like the C# FileLog.
- FFI: `lag_ffi::ffmpeg` (FFmpeg 7.1, incl. avfilter) and `lag_ffi::mpv`. Win32/COM: `windows` crate.
- Every port module starts with a doc comment naming the C# source file(s) it mirrors.
- Spec for your area: `lag-rs/docs/spec/<area>.md` (acceptance checklist in §10). Verify facts against the
  C# source; the spec can be wrong. Note spec errors in your report.
- Tests: `cargo test -p <crate>` must pass. Port C# self-tests (LAG_* env vars) as `#[test]`s or
  `examples/` where they are headless. Tests needing a game/GPU capture must be `#[ignore]`.
- Runtime DLLs (FFmpeg, libmpv) are copied next to every test/example binary by lag-ffi/build.rs;
  no PATH setup is needed (depend on lag-ffi).
- `cargo clippy -p <crate>` clean of warnings you introduced; `cargo fmt -p <crate>`.

## Report (end of task)
Files written · what's ported (map C# file → Rust module) · tests and their results · bugs found/fixed
(C# file:line, what, why) · deviations from C# behaviour (should be none or justified) · open questions.
- Slint: a child without explicit `x`/`y` (outside a layout) is CENTRED in its parent. Fills, bars, thumbs, list rows: always set `x`/`y`.
- UI CHECK BEFORE ANY HANDOFF (owner rule 2026-11-16: "check the WHOLE UI, not just this problem"):
  `target\release\Lag.exe --ui-check tools\ui-check\full.txt <out-dir>` and `... languages.txt <out-dir>`
  with APPDATA pointed at a SCRATCH copy (clicks in the script change and save settings). Headless, no
  window, input goes only to the in-memory window. Look at EVERY screenshot. Add a scenario line for each
  new UI state you build. Commands: see `Snapshot::run_script` in crates/lag-app/src/snapshot.rs.
- Slint: `TouchArea.moved` fires ONLY while a button is held (drags). Hover logic (show controls on mouse
  move, wake timers) must use `pointer-event(e) => { if (e.kind == PointerEventKind.move) {...} }`.
- Slint + FemtoVG at fractional DPI (owner runs 125 %): NEVER use `transform-scale-*` / `transform-rotation` in the
  app UI. A transformed element makes the GPU renderer redraw its surrounding card/page shifted by a sub-pixel
  ("the grid moves and comes alive"; proven with `Lag.exe --gpu-check` real-window snapshots 2026-11-17).
  Animate width/height/opacity/colour instead, or compute geometry (components/chevron.slint, ui/splash.slint).
  `--gpu-check <script> <out>` (src/gpucheck.rs) = real window + GPU snapshots; ask the owner before running.
