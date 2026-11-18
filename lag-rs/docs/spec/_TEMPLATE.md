# Spec template (for agents extracting the 1:1 port spec)

Context: Lag is a Windows instant-replay recorder written in C# / .NET 8 / Avalonia 11 (source in
`E:\project\InstantReplay\InstantReplay\`). It is being rewritten 1:1 in Rust (Slint UI, libmpv player,
windows-rs, same FFmpeg 7.1 DLLs). Your job is to READ the C# files of one area and write a spec so
precise that someone who never sees the C# can rebuild the area with identical look and behaviour.

Rules:
- READ ONLY in `InstantReplay/`. Write exactly ONE file: the output path given in your task. Touch nothing else.
- Do not summarise vaguely. Prefer exact values copied from source: sizes, margins, colours (hex), font sizes,
  corner radii, animation durations/easings, timer intervals, thresholds, default values, string resource keys,
  file paths, JSON keys, env vars, log line formats.
- Cite source as `File.cs:line` for every non-trivial fact.
- If something is dead code / unreachable, say so explicitly (don't spec it as a feature).
- English is fine for the spec text.

Sections (use these headings):
1. **Scope** — files covered (with line counts).
2. **Features** — numbered list of every user-visible feature/behaviour in this area.
3. **UI** — per screen/control: layout tree, exact styling, states (hover/pressed/disabled/selected),
   animations, bindings, keyboard shortcuts, context menus, tooltips, localisation keys used.
4. **State & logic** — view-model properties/commands, state machines, threading (which thread does what),
   timers, debounces, error handling, edge cases.
5. **Persistence & formats** — every file read/written: path, format, schema, versioning/migration.
6. **Native/external dependencies** — WinAPI/COM/FFmpeg/VLC/NAudio/SharpHook calls and what they're used for,
   with the Rust equivalent you'd suggest (windows-rs API, mpv property, etc.).
7. **Cross-area interfaces** — what this area calls in other areas and what calls into it.
8. **Self-tests / diagnostics** — env vars (LAG_*), what they do, expected output.
9. **Known bugs / quirks to preserve or fix** — note anything that looks like a bug; mark whether the
   current behaviour must be preserved.
10. **Parity checklist** — a flat `- [ ]` list of concrete, verifiable items (one behaviour each) that the
    Rust port must pass. Aim for completeness; this is the acceptance test.
