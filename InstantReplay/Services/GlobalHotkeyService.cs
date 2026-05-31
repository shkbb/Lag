using System;
using System.Runtime.InteropServices;
using System.Threading;
using SharpHook.Native;

namespace Lag.Services;

/// <summary>
/// A system-wide global hotkey service that uses Win32 P/Invoke to RegisterHotKey.
/// Creates a headless, message-only window on a background thread to reliably receive
/// WM_HOTKEY messages without relying on the Avalonia UI message loop.
/// </summary>
public sealed class GlobalHotkeyService : IDisposable
{
    private const int WM_HOTKEY = 0x0312;
    private const int MOD_ALT = 0x0001;
    private const int MOD_CONTROL = 0x0002;
    private const int MOD_SHIFT = 0x0004;
    private const int MOD_WIN = 0x0008;
    private const int MOD_NOREPEAT = 0x4000;
    private const uint WM_QUIT = 0x0012;

    private const int HOTKEY_ID = 9000;

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool RegisterHotKey(IntPtr hWnd, int id, uint fsModifiers, uint vk);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool UnregisterHotKey(IntPtr hWnd, int id);

    [DllImport("user32.dll")]
    private static extern int GetMessage(out MSG lpMsg, IntPtr hWnd, uint wMsgFilterMin, uint wMsgFilterMax);

    [DllImport("user32.dll")]
    private static extern bool TranslateMessage(ref MSG lpMsg);

    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessage(ref MSG lpMsg);

    [DllImport("user32.dll")]
    private static extern void PostQuitMessage(int nExitCode);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostThreadMessage(uint idThread, uint Msg, IntPtr wParam, IntPtr lParam);

    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();

    [StructLayout(LayoutKind.Sequential)]
    private struct MSG
    {
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public POINT pt;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT
    {
        public int x;
        public int y;
    }

    private Thread? _messageLoopThread;
    private uint _threadId;
    private bool _isDisposed;

    public event EventHandler? HotkeyPressed;

    public void Start(uint modifiers, uint virtualKey)
    {
        if (_messageLoopThread != null)
            return;

        _messageLoopThread = new Thread(() => MessageLoop(modifiers, virtualKey))
        {
            IsBackground = true,
            Name = "Win32HotkeyThread"
        };
        _messageLoopThread.Start();
    }

    /// <summary>
    /// Re-registers the global hotkey with a new combination, cleanly stopping the previous
    /// message-loop thread first. Safe to call repeatedly (e.g. each time the user rebinds).
    /// </summary>
    public void UpdateHotkey(uint modifiers, uint virtualKey)
    {
        if (_isDisposed) return;
        StopMessageLoop();
        Start(modifiers, virtualKey);
    }

    /// <summary>
    /// Converts a SharpHook modifier mask + key code into the Win32 (fsModifiers, vk) pair and
    /// re-registers the hotkey. Returns false if the key cannot be mapped to a virtual-key code.
    /// </summary>
    public bool UpdateHotkey(ModifierMask modifiers, KeyCode key)
    {
        uint vk = VirtualKeyFromKeyCode(key);
        if (vk == 0) return false;

        UpdateHotkey(ModifiersToWin32(modifiers), vk);
        return true;
    }

    /// <summary>Maps a SharpHook <see cref="ModifierMask"/> to Win32 MOD_* flags (e.g. MOD_CONTROL | MOD_SHIFT).</summary>
    public static uint ModifiersToWin32(ModifierMask modifiers)
    {
        uint result = 0;
        if (modifiers.HasFlag(ModifierMask.LeftCtrl) || modifiers.HasFlag(ModifierMask.RightCtrl)) result |= MOD_CONTROL;
        if (modifiers.HasFlag(ModifierMask.LeftShift) || modifiers.HasFlag(ModifierMask.RightShift)) result |= MOD_SHIFT;
        if (modifiers.HasFlag(ModifierMask.LeftAlt) || modifiers.HasFlag(ModifierMask.RightAlt)) result |= MOD_ALT;
        if (modifiers.HasFlag(ModifierMask.LeftMeta) || modifiers.HasFlag(ModifierMask.RightMeta)) result |= MOD_WIN;
        return result;
    }

    /// <summary>
    /// Maps a SharpHook <see cref="KeyCode"/> to a Win32 virtual-key code by parsing its name
    /// (letters, digits, F1–F24 and common named keys). Returns 0 when unmapped.
    /// </summary>
    public static uint VirtualKeyFromKeyCode(KeyCode key)
    {
        string name = key.ToString();
        if (name.StartsWith("Vc", StringComparison.Ordinal))
            name = name.Substring(2);

        // Single letter A–Z → VK 0x41–0x5A
        if (name.Length == 1 && char.IsLetter(name[0]))
            return char.ToUpperInvariant(name[0]);

        // Single digit 0–9 → VK 0x30–0x39
        if (name.Length == 1 && char.IsDigit(name[0]))
            return name[0];

        // Function keys F1–F24 → VK_F1 (0x70) onward
        if (name.Length >= 2 && name[0] == 'F' && int.TryParse(name.Substring(1), out int fn) && fn is >= 1 and <= 24)
            return (uint)(0x70 + (fn - 1));

        return name switch
        {
            "Space" => 0x20,
            "Enter" => 0x0D,
            "Escape" => 0x1B,
            "Tab" => 0x09,
            "Backspace" => 0x08,
            "Delete" => 0x2E,
            "Insert" => 0x2D,
            "Home" => 0x24,
            "End" => 0x23,
            "PageUp" => 0x21,
            "PageDown" => 0x22,
            "Up" => 0x26,
            "Down" => 0x28,
            "Left" => 0x25,
            "Right" => 0x27,
            "PrintScreen" => 0x2C,
            _ => 0
        };
    }

    /// <summary>Signals the message-loop thread to quit and waits for it to unregister and exit.</summary>
    private void StopMessageLoop()
    {
        var thread = _messageLoopThread;
        if (thread == null) return;

        if (_threadId != 0)
            PostThreadMessage(_threadId, WM_QUIT, IntPtr.Zero, IntPtr.Zero);

        thread.Join(1000);
        _messageLoopThread = null;
        _threadId = 0;
    }

    private void MessageLoop(uint modifiers, uint virtualKey)
    {
        // Capture the native thread id so UpdateHotkey/Dispose can post WM_QUIT to this loop.
        _threadId = GetCurrentThreadId();

        // hWnd = IntPtr.Zero means the hotkey is associated with the current thread
        if (!RegisterHotKey(IntPtr.Zero, HOTKEY_ID, modifiers | MOD_NOREPEAT, virtualKey))
        {
            Console.WriteLine($"[GlobalHotkeyService] Failed to register hotkey. Error: {Marshal.GetLastWin32Error()}");
            return;
        }

        Console.WriteLine($"[GlobalHotkeyService] Registered Win32 Hotkey: Mod={modifiers}, Key={virtualKey}");

        MSG msg;
        int retVal;
        // Standard Win32 message pump for the thread
        while ((retVal = GetMessage(out msg, IntPtr.Zero, 0, 0)) != 0)
        {
            if (retVal == -1)
            {
                Console.WriteLine("[GlobalHotkeyService] GetMessage error.");
                break;
            }

            if (msg.message == WM_HOTKEY && msg.wParam.ToInt32() == HOTKEY_ID)
            {
                HotkeyPressed?.Invoke(this, EventArgs.Empty);
            }

            TranslateMessage(ref msg);
            DispatchMessage(ref msg);
        }

        UnregisterHotKey(IntPtr.Zero, HOTKEY_ID);
        Console.WriteLine("[GlobalHotkeyService] Message loop exited and hotkey unregistered.");
    }

    public void Dispose()
    {
        if (_isDisposed) return;
        _isDisposed = true;

        // Post WM_QUIT to the loop thread so it unregisters the hotkey and exits cleanly.
        StopMessageLoop();
    }
}
