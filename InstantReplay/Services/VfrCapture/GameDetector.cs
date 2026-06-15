using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace Lag.Services.VfrCapture;

/// <summary>One game-window candidate the detector picked, with the score that won it.</summary>
public sealed record GameCandidate(IntPtr Hwnd, uint Pid, string Exe, string GameName, int Score, bool Fullscreen);

/// <summary>
/// Decides which window (if any) is a game worth recording — a POSITIVE detector, not a denylist.
/// Modelled on Medal: every visible top-level window is scored from weighted signals and the
/// highest wins, instead of blocking a hand-maintained list of "not games".
///
/// Our signals (Medal uses a CDN game DB for "PreferredType"; we approximate it with a small
/// built-in known-games table PLUS a real-world heuristic Medal can lean on its DB to skip):
///   • Known game     — process name in <see cref="KnownGames"/>, or a game-engine window class
///                      (UnrealWindow / SDL_app / Unity…). Strong "this is a game" signal.
///   • Fullscreen     — the window covers the whole recorded monitor. By far the most reliable
///                      cross-title signal: real games run fullscreen/borderless, tools don't.
///   • Focused        — it's the foreground window right now.
/// A window only qualifies as a game if it is EITHER a known game OR fullscreen-on-monitor; the
/// score then just ranks qualifiers. A tiny stable shell/self exclude drops OS surfaces outright
/// (Medal does the same — its own overlay scores "Avoid"); everything else gets a fair hearing.
/// </summary>
public sealed class GameDetector
{
    // Weights mirror the magnitudes seen in Medal's logs (PreferredType≈46k, Focused≈18k); the
    // fullscreen weight is ours, sitting between them as a primary positive signal.
    private const int ScoreKnownGame = 46000;
    private const int ScoreFullscreen = 30000;
    private const int ScoreFocused = 18000;

    /// <summary>3D-engine utilization (%) above which a fullscreen process counts as a game. Calibrated
    /// on real hardware: CS2 in-match ≈ 64%, an active Chromium/Electron UI ≈ 16%, background ≈ 2–4%.
    /// The wide gap between "background app" and "game" makes 30% a safe divider.</summary>
    private const double Gpu3DGamePercent = 30.0;

    private readonly GpuUsageProbe _gpu = new();
    private bool _steamGameRunning;   // refreshed per FindBestGame pass: a Steam game is live (RunningAppID != 0)

    /// <summary>Process name (lower-case, no ".exe") → friendly game name. Not exhaustive — it's a
    /// fast path for naming + catching windowed/borderless games that don't cover the monitor;
    /// the fullscreen heuristic catches unknown fullscreen titles regardless.</summary>
    private static readonly Dictionary<string, string> KnownGames = new(StringComparer.OrdinalIgnoreCase)
    {
        ["cs2"] = "Counter-Strike 2", ["csgo"] = "CS:GO",
        ["dota2"] = "Dota 2", ["deadlock"] = "Deadlock",
        ["valorant-win64-shipping"] = "VALORANT", ["valorant"] = "VALORANT",
        ["r5apex"] = "Apex Legends", ["r5apexdx12"] = "Apex Legends",
        ["fortniteclient-win64-shipping"] = "Fortnite",
        ["gta5"] = "GTA V", ["gtav"] = "GTA V", ["gta-v"] = "GTA V",
        ["rocketleague"] = "Rocket League",
        ["league of legends"] = "League of Legends", ["leagueoflegends"] = "League of Legends",
        ["overwatch"] = "Overwatch 2",
        ["tslgame"] = "PUBG: BATTLEGROUNDS",
        ["rainbowsix"] = "Rainbow Six Siege", ["rainbowsix_vulkan"] = "Rainbow Six Siege",
        ["destiny2"] = "Destiny 2",
        ["bf2042"] = "Battlefield 2042", ["bf1"] = "Battlefield 1", ["bfv"] = "Battlefield V",
        ["eldenring"] = "Elden Ring",
        ["marvel-win64-shipping"] = "Marvel Rivals",
        ["discovery"] = "THE FINALS",
        ["helldivers2"] = "Helldivers 2",
        ["palworld-win64-shipping"] = "Palworld", ["palworld"] = "Palworld",
        ["rust"] = "Rust", ["dayz"] = "DayZ",
        ["warthunder"] = "War Thunder", ["aces"] = "War Thunder",
        ["modernwarfare"] = "Call of Duty", ["cod"] = "Call of Duty",
        ["minecraft.windows"] = "Minecraft",
        ["hl2"] = "Half-Life 2", ["portal2"] = "Portal 2",
    };

    /// <summary>Window classes used by game engines — a strong "this is a game" signal even for a
    /// process we don't list. Kept conservative to avoid false positives.</summary>
    private static readonly HashSet<string> GameEngineClasses = new(StringComparer.OrdinalIgnoreCase)
    {
        "UnrealWindow",        // Unreal Engine (PUBG, Fortnite, Rivals…)
        "SDL_app",             // SDL games (CS2, Dota 2, Deadlock, Valve titles)
        "riotWindowClass",     // Riot (VALORANT, LoL)
        "CryENGINE",           // CryEngine
        "Valve001",            // legacy Source
        "Sledgehammer Oasis",  // Call of Duty
    };

    /// <summary>The ONLY hard exclude — stable OS shell + our own process. Not a game denylist:
    /// browsers/chat/etc. are simply never fullscreen-on-monitor with a game class, so the scoring
    /// drops them without us naming them.</summary>
    private static readonly HashSet<string> ShellExclude = new(StringComparer.OrdinalIgnoreCase)
    {
        "explorer.exe", "dwm.exe", "applicationframehost.exe", "searchhost.exe",
        "shellexperiencehost.exe", "startmenuexperiencehost.exe", "textinputhost.exe",
        "lockapp.exe", "sihost.exe", "ctfmon.exe", "lag.exe",
    };

    /// <summary>Non-games that DO load the GPU 3D engine (browsers composite + WebGL, Electron apps,
    /// OBS, media players, launchers) and so would otherwise fool the 3D heuristic when fullscreen.
    /// This is NOT a game blocklist — it's the short, stable set of apps that look GPU-busy but aren't
    /// games. Anything not here is judged purely on its signals (name / fullscreen+3D / Steam).</summary>
    private static readonly HashSet<string> NonGameGpuExclude = new(StringComparer.OrdinalIgnoreCase)
    {
        // Browsers (GPU compositing, WebGL, accelerated video → real 3D-engine load, esp. fullscreen YouTube)
        "chrome.exe", "msedge.exe", "firefox.exe", "librewolf.exe", "brave.exe",
        "opera.exe", "opera_gx.exe", "vivaldi.exe", "browser.exe", "arc.exe",
        // Electron / Chromium-shell apps
        "discord.exe", "discordcanary.exe", "discordptb.exe", "slack.exe", "ms-teams.exe", "teams.exe",
        "spotify.exe", "code.exe", "claude.exe", "notion.exe", "whatsapp.exe", "telegram.exe",
        // Capture / streaming tools (would record themselves recording)
        "obs64.exe", "obs32.exe", "obs.exe", "streamlabs obs.exe", "xsplit.core.exe",
        // Media players (GPU-assisted rendering)
        "vlc.exe", "mpc-hc64.exe", "mpc-hc.exe", "mpc-be64.exe", "potplayermini64.exe",
        "potplayer64.exe", "mpv.exe", "wmplayer.exe",
        // Launchers / clients (the original Steam bug; these wrap CEF in game-engine window classes)
        "steam.exe", "steamwebhelper.exe", "epicgameslauncher.exe", "battle.net.exe",
        "galaxyclient.exe", "eadesktop.exe", "ealauncher.exe", "riotclientux.exe", "riotclientservices.exe",
    };

    /// <summary>Scans all visible top-level windows, scores each, and returns the best game window
    /// — or null if nothing qualifies. <paramref name="monitorDeviceId"/> limits "fullscreen" to a
    /// specific display (e.g. "\\.\DISPLAY2"); null/empty = the primary monitor.</summary>
    public GameCandidate? FindBestGame(string? monitorDeviceId)
    {
        GameCandidate? best = null;
        IntPtr foreground = GetForegroundWindow();

        // Refresh the hardware signals once per pass (cheap), then score every window against them.
        _gpu.Sample();
        _steamGameRunning = SteamGameRunning();

        EnumWindows((hwnd, _) =>
        {
            var c = Evaluate(hwnd, monitorDeviceId, foreground);
            if (c != null && (best == null || c.Score > best.Score)) best = c;
            return true; // keep enumerating
        }, IntPtr.Zero);

        return best;
    }

    /// <summary>Scores a single window. Returns null if it isn't a recordable game.</summary>
    public GameCandidate? Evaluate(IntPtr hwnd, string? monitorDeviceId, IntPtr foreground)
    {
        if (hwnd == IntPtr.Zero || !IsWindowVisible(hwnd) || IsIconic(hwnd)) return null;

        GetWindowThreadProcessId(hwnd, out uint pid);
        if (pid == 0 || pid == (uint)Environment.ProcessId) return null;

        string exe;
        try { exe = System.Diagnostics.Process.GetProcessById((int)pid).ProcessName + ".exe"; }
        catch { return null; }
        // Hard excludes: OS shell + the short list of GPU-busy non-games (browsers, Electron, OBS,
        // players, launchers). These never count as a game, so a fullscreen YouTube or the Steam
        // client can't fool the heuristics below.
        if (ShellExclude.Contains(exe) || NonGameGpuExclude.Contains(exe)) return null;

        string proc = exe.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? exe[..^4] : exe;
        string cls = GetWindowClass(hwnd);

        bool knownByName = KnownGames.TryGetValue(proc, out var dbName);
        bool engineClass = GameEngineClasses.Contains(cls);
        bool fullscreen = IsFullscreenOnMonitor(hwnd, monitorDeviceId);
        bool gpu3D = _gpu.Get3D(pid) >= Gpu3DGamePercent;

        // "Is this a GAME?" — no curated DB, just real signals:
        //   • known by NAME            → a curated title; trusted in any window state (borderless/small).
        //   • fullscreen + 3D-heavy    → pegs the GPU 3D engine like a game, unlike a video/desktop app.
        //   • fullscreen + Steam-live  → Steam reports a running game (covers light Steam titles too).
        //   • fullscreen + engineClass → an SDL/Unreal window that is ALSO fullscreen (windowed = launcher).
        // Plain fullscreen ALONE no longer qualifies (a fullscreen browser/video isn't a game), and the
        // GPU-busy non-games were already excluded above.
        bool gameSignal = knownByName
                       || (fullscreen && (gpu3D || _steamGameRunning || engineClass));
        if (!gameSignal) return null;

        int score = 0;
        if (knownByName) score += ScoreKnownGame;
        if (fullscreen) score += ScoreFullscreen;
        if (hwnd == foreground) score += ScoreFocused;
        if (gpu3D) score += ScoreKnownGame;          // a 3D-pegging fullscreen window is as strong as a known title
        if (_steamGameRunning) score += ScoreFullscreen;

        string name = dbName ?? FriendlyFromTitle(hwnd, proc);
        return new GameCandidate(hwnd, pid, exe, name, score, fullscreen);
    }

    private static string FriendlyFromTitle(IntPtr hwnd, string fallback)
    {
        var sb = new StringBuilder(256);
        GetWindowTextW(hwnd, sb, sb.Capacity);
        string t = sb.ToString().Trim();
        return string.IsNullOrEmpty(t) ? fallback : t;
    }

    private static string GetWindowClass(IntPtr hwnd)
    {
        var sb = new StringBuilder(256);
        GetClassNameW(hwnd, sb, sb.Capacity);
        return sb.ToString();
    }

    /// <summary>True when the window covers the whole recorded monitor (fullscreen/borderless).
    /// Filtering by monitor stops a fullscreen video on a second display from qualifying.</summary>
    private static bool IsFullscreenOnMonitor(IntPtr hwnd, string? monitorDeviceId)
    {
        if (!GetWindowRect(hwnd, out var rect)) return false;
        IntPtr mon = MonitorFromWindow(hwnd, 2 /* NEAREST */);
        var info = new MonitorInfoEx { Size = (uint)Marshal.SizeOf<MonitorInfoEx>() };
        if (!GetMonitorInfoW(mon, ref info)) return false;

        bool onRecorded = string.IsNullOrEmpty(monitorDeviceId)
            ? (info.Flags & 1) != 0 /* MONITORINFOF_PRIMARY */
            : string.Equals(info.Device, monitorDeviceId, StringComparison.OrdinalIgnoreCase);
        if (!onRecorded) return false;

        return rect.Left <= info.Monitor.Left && rect.Top <= info.Monitor.Top
            && rect.Right >= info.Monitor.Right && rect.Bottom >= info.Monitor.Bottom;
    }

    /// <summary>True when Steam reports a game currently running
    /// (HKCU\Software\Valve\Steam\RunningAppID != 0). Zero-maintenance: Valve keeps the list, not us —
    /// this alone covers the bulk of PC games regardless of our small built-in name table.</summary>
    private static bool SteamGameRunning()
    {
        try
        {
            using var k = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(@"Software\Valve\Steam");
            return k?.GetValue("RunningAppID") is int appId && appId != 0;
        }
        catch { return false; }
    }

    // ───────────────────────────── Win32 ─────────────────────────────

    private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] private static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr hWnd, out Win32Rect rect);
    [DllImport("user32.dll")] private static extern IntPtr MonitorFromWindow(IntPtr hWnd, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern bool GetMonitorInfoW(IntPtr hMonitor, ref MonitorInfoEx info);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassNameW(IntPtr hWnd, StringBuilder text, int maxCount);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int maxCount);

    [StructLayout(LayoutKind.Sequential)]
    private struct Win32Rect { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct MonitorInfoEx
    {
        public uint Size;
        public Win32Rect Monitor;
        public Win32Rect Work;
        public uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string Device;
    }
}
