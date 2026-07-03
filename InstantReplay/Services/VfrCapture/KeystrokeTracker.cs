using System;
using System.Collections.Generic;
using SharpHook.Native;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Tracks currently-pressed keys / mouse buttons for the recording overlay. Fed by
/// GlobalHotkeyManager's existing SharpHook global hook (never a second hook — see the
/// system-wide cursor-lag lesson), read by OverlayCompositor on the capture thread.
///
/// Inactive (no overlay recording) it is a single volatile-bool check per event — free.
/// Entries linger briefly after release so fast taps stay readable, then fade out.
/// </summary>
public sealed class KeystrokeTracker
{
    public static KeystrokeTracker Instance { get; } = new();

    /// <summary>How long a released key stays visible (fading) in the overlay.</summary>
    public const double FadeMs = 450;

    private const int MaxEntries = 10;

    private readonly object _gate = new();
    private readonly List<Entry> _entries = new();
    private volatile bool _active;

    /// <summary>One badge: the label, when it went down, and when it was released (0 = still held).</summary>
    public sealed record Entry(string Label)
    {
        public long DownAtTicks { get; set; }
        public long UpAtTicks { get; set; }   // 0 while held
    }

    /// <summary>Enable while a session records with the keys overlay; disable on teardown.</summary>
    public bool Active
    {
        get => _active;
        set
        {
            _active = value;
            if (!value) { lock (_gate) _entries.Clear(); }
        }
    }

    // ── Feed (called by GlobalHotkeyManager on hook threads) ──

    public void OnKey(KeyCode key, bool down) { if (_active) Push(KeyLabel(key), down); }

    public void OnMouseButton(int button, bool down)
    {
        if (!_active) return;
        string label = button switch { 1 => "LMB", 2 => "RMB", 3 => "MMB", 4 => "M4", 5 => "M5", _ => $"M{button}" };
        Push(label, down);
    }

    private void Push(string? label, bool down)
    {
        if (label == null) return;
        long now = DateTime.UtcNow.Ticks;
        lock (_gate)
        {
            var e = _entries.Find(x => x.Label == label);
            if (down)
            {
                if (e == null)
                {
                    if (_entries.Count >= MaxEntries) _entries.RemoveAt(0);
                    _entries.Add(new Entry(label) { DownAtTicks = now });
                }
                else { e.DownAtTicks = now; e.UpAtTicks = 0; }   // re-press revives a fading badge
            }
            else if (e != null)
            {
                e.UpAtTicks = now;
            }
        }
    }

    /// <summary>Snapshot for rendering: (label, opacity 0..1), expired entries pruned.</summary>
    public List<(string Label, float Opacity)> Snapshot()
    {
        long now = DateTime.UtcNow.Ticks;
        var result = new List<(string, float)>();
        lock (_gate)
        {
            for (int i = _entries.Count - 1; i >= 0; i--)
            {
                var e = _entries[i];
                if (e.UpAtTicks == 0) { continue; }
                double upMs = (now - e.UpAtTicks) / (double)TimeSpan.TicksPerMillisecond;
                if (upMs > FadeMs) _entries.RemoveAt(i);
            }
            foreach (var e in _entries)
            {
                float opacity = 1f;
                if (e.UpAtTicks != 0)
                {
                    double upMs = (now - e.UpAtTicks) / (double)TimeSpan.TicksPerMillisecond;
                    opacity = (float)Math.Clamp(1.0 - upMs / FadeMs, 0, 1);
                }
                result.Add((e.Label, opacity));
            }
        }
        return result;
    }

    /// <summary>Compact ASCII labels; null = keys not worth a badge (unknown codes).</summary>
    private static string? KeyLabel(KeyCode key)
    {
        string name = key.ToString();          // "VcW", "VcLeftShift", "VcF10", ...
        if (!name.StartsWith("Vc")) return null;
        name = name[2..];

        return name switch
        {
            "LeftShift" or "RightShift" => "Shift",
            "LeftControl" or "RightControl" => "Ctrl",
            "LeftAlt" or "RightAlt" => "Alt",
            "LeftMeta" or "RightMeta" => "Win",
            "Space" => "Space",
            "Enter" => "Enter",
            "Backspace" => "Bksp",
            "Escape" => "Esc",
            "Tab" => "Tab",
            "CapsLock" => "Caps",
            "Up" => "↑",
            "Down" => "↓",
            "Left" => "←",
            "Right" => "→",
            "Comma" => ",",
            "Period" => ".",
            "Slash" => "/",
            "Backslash" => "\\",
            "Semicolon" => ";",
            "Quote" => "'",
            "Minus" => "-",
            "Equals" => "=",
            "OpenBracket" => "[",
            "CloseBracket" => "]",
            "Backquote" => "`",
            _ when name.Length == 1 => name,                          // letters
            _ when name.Length == 2 && name[0] == '1' => name,        // "10".. never happens; keep digits below
            _ when name.StartsWith("F") && name.Length <= 3 => name,  // F1..F12
            _ when name.Length == 1 || (name.Length == 2 && char.IsDigit(name[1])) => name,
            _ when name is "0" or "1" or "2" or "3" or "4" or "5" or "6" or "7" or "8" or "9" => name,
            _ => null,   // NumPad*, media keys, etc. — skip rather than clutter
        };
    }
}
