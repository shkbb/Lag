using System;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;

namespace Lag.Services;

/// <summary>
/// Persistent file logging. The app already writes rich diagnostics via
/// <c>Console.WriteLine("[Component] …")</c> everywhere; this tees <see cref="Console.Out"/> and
/// <see cref="Console.Error"/> to a timestamped rolling .txt under Documents\Lag\Logs\ (and still to
/// the original console), so every line is captured to disk for diagnostics — no changes needed at
/// the hundreds of existing log call-sites. Logs live in Documents (not hidden AppData) so they're
/// easy to find and share for support.
/// </summary>
public sealed class FileLog : TextWriter
{
    private readonly TextWriter _console;
    private readonly StreamWriter _file;
    private readonly StringBuilder _line = new();
    private readonly object _gate = new();

    public override Encoding Encoding => Encoding.UTF8;

    private FileLog(TextWriter console, StreamWriter file) { _console = console; _file = file; }

    /// <summary>Folder holding the log files (Documents\Lag\Logs — discoverable, easy to share).</summary>
    public static string LogDir =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "Lag", "Logs");

    /// <summary>Cross-session crash log (every unhandled exception, newest appended), kept next to
    /// the per-launch logs so a single folder has everything support needs.</summary>
    public static string CrashLogPath => Path.Combine(LogDir, "crash.log");

    /// <summary>Path of the current session's log file (set after <see cref="Initialize"/>).</summary>
    public static string? CurrentLogPath { get; private set; }

    /// <summary>Begins capturing all console output to a fresh per-launch log file. Never throws —
    /// logging must not be able to break startup.</summary>
    public static void Initialize()
    {
        try
        {
            Directory.CreateDirectory(LogDir);
            PruneOldLogs(keep: 15);

            string path = Path.Combine(LogDir, $"Lag-{DateTime.Now:yyyy-MM-dd_HH-mm-ss}.txt");
            CurrentLogPath = path;
            var file = new StreamWriter(path, append: false) { AutoFlush = true };

            file.WriteLine($"===== Lag log — {DateTime.Now:yyyy-MM-dd HH:mm:ss} =====");
            file.WriteLine($"Lag {AssemblyVersion()} | OS {Environment.OSVersion} | 64-bit {Environment.Is64BitProcess} | CLR {Environment.Version} | machine {Environment.MachineName}");
            file.WriteLine(new string('-', 60));

            var tee = new FileLog(Console.Out, file);
            Console.SetOut(tee);
            Console.SetError(tee);
        }
        catch { /* swallow — never break startup over logging */ }
    }

    public override void Write(char value)
    {
        lock (_gate)
        {
            if (value == '\n') { FlushLine(_line.ToString()); _line.Clear(); }
            else if (value != '\r') _line.Append(value);
        }
    }

    public override void Write(string? value)
    {
        if (string.IsNullOrEmpty(value)) return;
        foreach (char c in value) Write(c);
    }

    public override void WriteLine(string? value)
    {
        lock (_gate)
        {
            if (_line.Length > 0) { _line.Append(value); FlushLine(_line.ToString()); _line.Clear(); }
            else FlushLine(value ?? string.Empty);
        }
    }

    private void FlushLine(string line)
    {
        string stamped = $"[{DateTime.Now:HH:mm:ss.fff}] {line}";
        try { _file.WriteLine(stamped); } catch { }
        try { _console.WriteLine(line); } catch { }
    }

    /// <summary>Best-effort assembly version string for the log header (the authoritative Velopack
    /// release version is logged later from the UI layer where it's available).</summary>
    private static string AssemblyVersion()
    {
        try
        {
            return typeof(FileLog).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
                   ?? typeof(FileLog).Assembly.GetName().Version?.ToString()
                   ?? "?";
        }
        catch { return "?"; }
    }

    /// <summary>Records an unhandled exception: appends it to the cross-session <see cref="CrashLogPath"/>
    /// AND echoes it to the current session log (via the console tee), so it's both aggregated and in
    /// context. Never throws — crash logging must never take the process down with it.</summary>
    public static void LogCrash(string source, Exception? ex)
    {
        string block = $"───── {DateTime.Now:yyyy-MM-dd HH:mm:ss} [{source}] Lag {AssemblyVersion()} ─────" +
                       $"{Environment.NewLine}{ex}{Environment.NewLine}";
        try { Directory.CreateDirectory(LogDir); File.AppendAllText(CrashLogPath, block + Environment.NewLine); } catch { }
        try { Console.Error.WriteLine($"[CRASH:{source}] {ex}"); } catch { }
    }

    /// <summary>Keeps only the newest <paramref name="keep"/> log files.</summary>
    private static void PruneOldLogs(int keep)
    {
        try
        {
            var old = new DirectoryInfo(LogDir).GetFiles("Lag-*.txt")
                .OrderByDescending(f => f.LastWriteTimeUtc).Skip(keep);
            foreach (var f in old) { try { f.Delete(); } catch { } }
        }
        catch { }
    }
}
