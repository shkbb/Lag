using SharpHook;
using SharpHook.Native;

namespace Lag.Services;

/// <summary>
/// Cross-platform global hotkey manager using SharpHook (libuiohook wrapper).
/// Captures keyboard input system-wide, including when games run in exclusive fullscreen.
/// 
/// Thread Safety: The hook callback fires on a background thread. Events are raised
/// directly — callers must marshal to UI thread if needed (e.g., via Dispatcher).
/// </summary>
public sealed class GlobalHotkeyManager : IDisposable
{
    // TaskPoolGlobalHook dispatches events on the thread pool instead of the hook thread
    // itself. With SimpleGlobalHook any slowness in our process (GC pause, CPU spike)
    // stalled the low-level hook callback — which Windows punishes by lagging the MOUSE
    // CURSOR system-wide. Never block the hook thread.
    private TaskPoolGlobalHook? _hook;
    private readonly object _lock = new();
    private bool _disposed;

    /// <summary>
    /// The modifier keys required for the hotkey (e.g., Alt, Ctrl, Shift).
    /// </summary>
    public ModifierMask RequiredModifiers { get; set; } = ModifierMask.LeftAlt;

    /// <summary>
    /// The primary key code for the hotkey (e.g., F10).
    /// </summary>
    public KeyCode RequiredKey { get; set; } = KeyCode.VcF10;

    /// <summary>
    /// Fired when the configured hotkey combination is pressed.
    /// Warning: This event fires on a background thread.
    /// </summary>
    public event EventHandler? HotkeyPressed;

    /// <summary>
    /// Fired when the manager is in "capture" mode and any key is pressed.
    /// Used by the Settings UI to let the user bind a new hotkey.
    /// Returns the captured key and modifiers; caller should then update
    /// <see cref="RequiredKey"/> and <see cref="RequiredModifiers"/>.
    /// </summary>
    public event EventHandler<HotkeyCapturedEventArgs>? HotkeyCaptured;

    /// <summary>
    /// When true, the next key press will be captured for rebinding
    /// instead of checking against the current hotkey.
    /// </summary>
    public bool IsCapturing { get; set; }

    // ───────────── Push-to-talk ─────────────

    /// <summary>Whether push-to-talk key tracking is active.</summary>
    public bool PttEnabled { get; set; }

    /// <summary>The push-to-talk key (held = mic live). Default: V.</summary>
    public KeyCode PttKey { get; set; } = KeyCode.VcV;

    /// <summary>Fired (possibly repeatedly, due to key auto-repeat) while the PTT key is pressed. Background thread!</summary>
    public event EventHandler? PttPressed;

    /// <summary>Fired when the PTT key is released. Background thread!</summary>
    public event EventHandler? PttReleased;

    /// <summary>
    /// Starts the global keyboard hook on a background thread.
    /// Must be called once during application startup.
    /// </summary>
    public async Task StartAsync()
    {
        lock (_lock)
        {
            if (_hook != null) return;
            // Keyboard events only: the old global mouse subscription (click-to-pause for the
            // native VLC window) is gone — video clicks are plain Avalonia events now.
            _hook = new TaskPoolGlobalHook();
            _hook.KeyPressed += OnKeyPressed;
            _hook.KeyReleased += OnKeyReleased;
        }

        // RunAsync blocks until the hook is disposed, so we fire-and-forget.
        await Task.Run(() => _hook.RunAsync());
    }

    /// <summary>
    /// Handles every key press event from the global hook.
    /// In capture mode, raises <see cref="HotkeyCaptured"/>.
    /// In normal mode, checks if the key combo matches the configured hotkey.
    /// </summary>
    private void OnKeyPressed(object? sender, KeyboardHookEventArgs e)
    {
        if (_disposed) return;

        var modifiers = e.RawEvent.Mask;
        var key = e.Data.KeyCode;

        // --- Capture mode: used by Settings to bind a new hotkey ---
        if (IsCapturing)
        {
            // Ignore standalone modifier presses (Shift/Ctrl/Alt/Win). Otherwise pressing Shift would
            // be captured as "Shift + LeftShift", because the modifier key itself arrives as the trigger.
            // We keep waiting until a real (non-modifier) key is pressed.
            if (IsStandaloneModifier(key))
                return;

            // A non-modifier key finalizes the hotkey immediately, using whatever modifiers are held.
            // This supports 1–3 key combinations:
            //   • single key   — e.g. F10 with NO modifiers (modifiers == None is perfectly valid);
            //   • 2-key combo  — e.g. Alt + F10;
            //   • 3-key combo  — e.g. Ctrl + Shift + R.
            IsCapturing = false;
            HotkeyCaptured?.Invoke(this, new HotkeyCapturedEventArgs(key, modifiers));
            return;
        }

        // --- Push-to-talk: key held → mic live (auto-repeat re-fires; unmute is idempotent) ---
        if (PttEnabled && key == PttKey)
        {
            PttPressed?.Invoke(this, EventArgs.Empty);
        }

        // --- Normal mode: check if the combo matches the configured hotkey ---
        if (key == RequiredKey && (modifiers & RequiredModifiers) == RequiredModifiers)
        {
            HotkeyPressed?.Invoke(this, EventArgs.Empty);
        }
    }

    /// <summary>Tracks the PTT key release to re-mute the microphone.</summary>
    private void OnKeyReleased(object? sender, KeyboardHookEventArgs e)
    {
        if (_disposed) return;

        if (PttEnabled && e.Data.KeyCode == PttKey)
        {
            PttReleased?.Invoke(this, EventArgs.Empty);
        }
    }

    /// <summary>
    /// True if the key is a bare modifier (Shift/Ctrl/Alt/Win, left or right). Such keys must not
    /// be accepted as the primary trigger of a hotkey combination.
    /// </summary>
    private static bool IsStandaloneModifier(KeyCode key) => key switch
    {
        KeyCode.VcLeftShift or KeyCode.VcRightShift
        or KeyCode.VcLeftControl or KeyCode.VcRightControl
        or KeyCode.VcLeftAlt or KeyCode.VcRightAlt
        or KeyCode.VcLeftMeta or KeyCode.VcRightMeta => true,
        _ => false
    };

    /// <summary>
    /// Stops the global hook and releases resources.
    /// </summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        lock (_lock)
        {
            if (_hook != null)
            {
                _hook.KeyPressed -= OnKeyPressed;
                _hook.KeyReleased -= OnKeyReleased;
                _hook.Dispose();
                _hook = null;
            }
        }
    }
}

/// <summary>
/// Event args carrying the captured key combination from the hotkey manager.
/// </summary>
public class HotkeyCapturedEventArgs : EventArgs
{
    public KeyCode Key { get; }
    public ModifierMask Modifiers { get; }

    public HotkeyCapturedEventArgs(KeyCode key, ModifierMask modifiers)
    {
        Key = key;
        Modifiers = modifiers;
    }
}
