using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using NAudio.CoreAudioApi;
using NAudio.CoreAudioApi.Interfaces;

namespace Lag.Services;

/// <summary>
/// Enumerates applications that are CURRENTLY playing audio, via WASAPI audio sessions on the
/// default render endpoint — the "record audio from specific apps" picker. Apps appear
/// as soon as they produce sound (so Windows system processes that never play audio don't clutter
/// the list). Each entry carries a friendly name and the app's icon extracted from its executable.
/// </summary>
public static class AudioSessionService
{
    /// <summary>One application with an active audio session: matching exe, friendly name, icon PNG.</summary>
    public readonly record struct AudioApp(string ExeName, string DisplayName, byte[]? IconPng);

    /// <summary>
    /// Returns the distinct processes with an ACTIVE audio session right now (our own process and the
    /// system idle/sounds session at pid 0 excluded). The exe path is resolved with
    /// QueryFullProcessImageName (lower access rights than Process.MainModule, so it works for
    /// hardened apps like Firefox where MainModule throws — that was the missing-icon bug).
    /// Safe to call from a background thread; per-process failures are swallowed.
    /// </summary>
    public static List<AudioApp> GetActiveAudioApps()
    {
        var found = new Dictionary<string, AudioApp>(StringComparer.OrdinalIgnoreCase);

        try
        {
            using var enumerator = new MMDeviceEnumerator();
            using var device = enumerator.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);
            var sessions = device.AudioSessionManager.Sessions;
            int selfPid = Environment.ProcessId;

            for (int i = 0; i < sessions.Count; i++)
            {
                var session = sessions[i];
                if (session.State != AudioSessionState.AudioSessionStateActive) continue;

                int pid = (int)session.GetProcessID;
                if (pid == 0 || pid == selfPid) continue;

                try
                {
                    string procName;
                    using (var p = Process.GetProcessById(pid)) procName = p.ProcessName;
                    string exe = procName + ".exe";
                    if (found.ContainsKey(exe)) continue;

                    string display = procName;
                    byte[]? icon = null;
                    string? path = GetProcessImagePath(pid);
                    if (!string.IsNullOrEmpty(path))
                    {
                        try
                        {
                            var fvi = FileVersionInfo.GetVersionInfo(path);
                            if (!string.IsNullOrWhiteSpace(fvi.FileDescription)) display = fvi.FileDescription!.Trim();
                        }
                        catch { }
                        icon = ExtractIconPng(path);
                    }

                    found[exe] = new AudioApp(exe, display, icon);
                }
                catch { /* process exited between enumeration and lookup — skip */ }
            }
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[AudioSessionService] Session enumeration failed: {ex.Message}");
        }

        return found.Values.OrderBy(a => a.DisplayName, StringComparer.OrdinalIgnoreCase).ToList();
    }

    /// <summary>Resolves a PID's full exe path via QueryFullProcessImageName (works on hardened
    /// processes where Process.MainModule is access-denied). Null if it can't be opened.</summary>
    private static string? GetProcessImagePath(int pid)
    {
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (h == IntPtr.Zero) return null;
        try
        {
            var sb = new StringBuilder(1024);
            int size = sb.Capacity;
            return QueryFullProcessImageNameW(h, 0, sb, ref size) ? sb.ToString() : null;
        }
        finally { CloseHandle(h); }
    }

    /// <summary>Extracts the executable's associated icon as PNG bytes for display in the UI.</summary>
    private static byte[]? ExtractIconPng(string exePath)
    {
        try
        {
            using var ico = System.Drawing.Icon.ExtractAssociatedIcon(exePath);
            if (ico == null) return null;
            using var bmp = ico.ToBitmap();
            using var ms = new MemoryStream();
            bmp.Save(ms, System.Drawing.Imaging.ImageFormat.Png);
            return ms.ToArray();
        }
        catch { return null; }
    }

    private const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inherit, int pid);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern bool QueryFullProcessImageNameW(IntPtr proc, uint flags, StringBuilder name, ref int size);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);
}
