using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using NAudio.CoreAudioApi;
using NAudio.CoreAudioApi.Interfaces;

namespace Lag.Services;

/// <summary>
/// Enumerates applications that are CURRENTLY playing audio, via WASAPI audio sessions on the
/// default render endpoint. Used by the Medal-style "record audio from specific apps" picker —
/// apps appear in the list as soon as they start producing sound.
/// </summary>
public static class AudioSessionService
{
    /// <summary>One application with at least one active audio session.</summary>
    public readonly record struct AudioApp(string ExeName, string DisplayName);

    /// <summary>
    /// Returns the distinct list of processes with an ACTIVE audio session right now.
    /// Our own process and the system idle/sounds session (pid 0) are excluded.
    /// Safe to call from a background thread; failures return an empty list.
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
                if (session.State != AudioSessionState.AudioSessionStateActive)
                    continue;

                int pid = (int)session.GetProcessID;
                if (pid == 0 || pid == selfPid)
                    continue;

                try
                {
                    using var process = Process.GetProcessById(pid);
                    string exe = process.ProcessName + ".exe";
                    if (!found.ContainsKey(exe))
                        found[exe] = new AudioApp(exe, process.ProcessName);
                }
                catch
                {
                    // Process exited between enumeration and lookup — skip.
                }
            }
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[AudioSessionService] Session enumeration failed: {ex.Message}");
        }

        return found.Values.OrderBy(a => a.DisplayName, StringComparer.OrdinalIgnoreCase).ToList();
    }
}
