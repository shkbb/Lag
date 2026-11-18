# lag-core port report

Files written: `Cargo.toml`, `.gitignore`, `src/lib.rs`, `src/settings.rs`,
`src/netjson.rs`, `src/filelog.rs`, `src/metadata.rs`, `src/clip.rs`, `src/i18n.rs`,
`src/flags.json`, `src/presets.rs`, `src/hardware.rs`, `src/recorder_options.rs`,
`examples/convert_langs.rs`, this report, and the 17 `langs/{code}.json` dictionaries.
All edits and generated verification artifacts are inside `crates/lag-core/`.
No GUI was opened and no git commit was made.

## What's ported

| C# source | Rust implementation |
| --- | --- |
| `ViewModels/SettingsViewModel.cs` settings serialization and `Services/ClipMetadata.cs` JSON options | `netjson.rs`; `settings.rs` now shares the formatter. CRLF, two-space indentation, declaration order, default encoder escaping, UTF-16 surrogate escapes, shortest floats and .NET exponent spelling. Nested non-finite floats are rejected. |
| `Services/FileLog.cs` | `filelog.rs`: `log::Log`, known-folder Documents location, session naming/header, exact timestamp/message bytes, newest-15 retention, managed/native crash blocks and console tee. Bounded background writer, explicit flush barrier, draining shutdown guard, observable queue rejection count and writer errors. |
| `Services/ClipMetadata.cs` | `metadata.rs`: full schema, uppercase MD5 of the supplied invariant-lowercased UTF-8 path, cache priority, legacy migration, setters, update and deletion. Best-effort C# API plus optional error-returning writes. |
| `Models/ReplayClip.cs` | `clip.rs`: plain UI state, generic thumbnail field, signed TimeSpan ticks, local creation date, path/title and culture-aware size labels, exact component-based duration formatting. |
| `Core/Localizer.cs`, `Core/LanguageToFlagConverter.cs`, `App.axaml`, `App.axaml.cs`, language options in `SettingsViewModel.cs` | `i18n.rs`, `flags.json`, converter and embedded language JSON: live language switch, English/key fallback, string composite formatting, native display names, resource/country identities and exact flag drawing data. |
| `Services/PresetResolver.cs` | `presets.rs`: all tier rules, resolution/FPS/codec tables, bitrate estimates, RAM budgets, buffer/bitrate/storage rungs, summaries and recommendation logs. |
| `Services/HardwareDetector.cs`, `Services/HardwareProfiler.cs` | `hardware.rs`: DXGI adapters and outputs, GDI monitor order/bounds/refresh, PnP interface StableId, borrowed-window monitor lookup, WASAPI input/output endpoints, CPU registry/core count, RAM, library-drive free/total space and profile reporting. |
| `Services/RecorderOptions.cs` | `recorder_options.rs`: every session option/default and `AppAudioCapture`. |

The profiler consumes actual cached encoder-probe results as `EncoderCapability` data.
The C# profiler obtains those same results from the separate capture engine's
`VfrCapture/EncoderSelector.cs`; encoder opening belongs to that engine, rather than
being guessed from GPU names or duplicated in this core crate.

New crate-local dependencies: `chrono`, `md5`, and development-only `roxmltree`.
Existing shared dependencies use `workspace = true`; Win32 features are declared
only in this crate. No root manifest or lockfile was edited by this task.

## Tests and results

- `cargo test -p lag-core`: 26 passed, zero failures; doc tests passed.
- `cargo clippy -p lag-core --all-targets -- -D warnings`: clean.
- `cargo fmt -p lag-core -- --check`: clean.
- The Rust converter was run against the original C# AXAML files: 17 languages,
  235 keys each. Tests compare every key and exact decoded text against AXAML,
  assert English key coverage, compare every flag layer against `App.axaml`, and
  check all bundled composite-format templates.
- Preset tests cover all GPU families/tier branches, intent-by-tier tables,
  rungs/ties, resolution boundaries, codec fallback, CPU-only protection, panel
  caps, RAM floors/bitrate clamp and disk hints.
- Logging tests cover exact bytes, facade filtering, queue saturation without
  waiting, crash append format, retention, writer errors, explicit flush and
  draining Drop/shutdown. Read-only hardware probes passed headlessly.
- A headless .NET 8 oracle independently confirmed float-format goldens and
  `C:\Clips\Replay.mp4` -> `E70506024595A2FBC13F5ECC2858A8C3`.
- Real log inspected: `Documents\Lag\Logs\Lag-2026-11-15_22-15-04.txt`.
  Its bytes confirm UTF-8 without BOM, CRLF and `[HH:mm:ss.fff] message`.

Verification environment: stable `x86_64-pc-windows-gnu`. The default installed
GNU import/link tools could not produce a runnable full test executable here.
Verification used the toolchain's bundled LLVM linker and a temporary Rust
import-library helper, entirely under ignored `target/`; no MSVC or C++ build
was introduced. FFmpeg/mpv DLL directories were included on PATH for the initial
workspace checks; this crate itself has no FFmpeg/mpv DLL dependency.

An initial workspace test run passed 23 tests. Concurrent changes to the shared
workspace subsequently left its manifest/lockfile inconsistent, so final checks
used `--manifest-path crates/lag-core/target/verification/Cargo.toml --offline`.
That isolated workspace contains exact copies of this package and the original
AXAML references. SHA-256 comparisons confirmed all 30 source/data/manifest files
are identical to the delivered package. Only its private lockfile was updated.

The real AppData settings and metadata locations exist but their contents are
denied by the sandbox. The existing settings test and added live-sidecar/hash
checks therefore returned without exercising those files. Their availability
checks must not be interpreted as successful real-data byte comparisons.

## Bugs found/fixed

1. `Services/FileLog.cs:50,90`: `AutoFlush = true` and `_file.WriteLine` perform
   file writes on logging callers, including capture/hot threads. Moved writes
   and console output to the bounded worker. Accepted messages keep their
   original caller-time timestamps and unchanged message text.
2. `Services/FileLog.cs:46,48-50,145`: pruning to 15 before creating the next
   session leaves 16 logs. Prune after creation and retain 15 including the
   current session, as explicitly requested. `crash.log` is never rotated.

The existing Rust-only formatter also had incomplete scientific/f32/negative-zero
handling. Fixed against .NET 8 goldens while extracting it; existing settings
tests continue to pass.

## Deviations from C# behavior

- As requested, logging uses the Rust facade and a bounded asynchronous writer.
  A full/stopped queue rejects the newest message rather than blocking a game
  thread; `write_line`/crash methods return acceptance and `dropped_messages()`
  exposes the count. `flush()` and guard shutdown are intentionally blocking
  barriers for already accepted messages. Retain the guard until application exit.
- Header runtime/build identity is supplied through `SessionInfo`; defaults
  identify this Rust package/runtime rather than falsely reporting a CLR version.
  Header layout and configurable C# identity values retain the exact format.
- English fallback for a missing selected-language key is explicitly requested;
  the C# resource resolver itself otherwise falls directly to the key. All 17
  current dictionaries are complete, so existing translated strings are unchanged.
- Composite formatting accepts preformatted string arguments, which covers every
  bundled template; callers format numeric values in their culture before passing
  them, rather than relying on CLR object formatting.
- The newest-15 retention correction above implements the requested limit.

Preserved C# quirks include nearest-rung snapping even when it exceeds the RAM
estimate, 180 seconds snapping to 120, old GPU family caps, unchanged clip-path
separators before hashing, corrupt-cache precedence over legacy data, and duration
hours wrapping at 24. No cleanup setting is automatically enabled.

Spec discrepancy: `shell.md` summarizes retention as 15, while the C# ordering
actually keeps 16 after opening a new session. The implementation follows the
task's explicit newest-15 requirement. No other spec errors required a change.

## Open questions

No implementation questions remain. Live AppData compatibility and the hash of
an actual existing clip against its actual sidecar filename still need execution
with read access to those files; the exhaustive tests are present. Final ordinary
workspace invocation also needs the concurrently maintained root lockfile to
agree with the current workspace manifest.
