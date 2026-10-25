using System;
using Windows.Media.Core;
using Windows.Media.Playback;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Asks the Windows media stack — the one the built-in Media Player app uses — which audio track it
/// picks by default for a clip (LAG_MFPROBE=file1;file2;...). Our clips carry an "All" mix plus
/// per-source tracks; libVLC plays "All", and this shows whether Windows does too.
/// </summary>
public static class PlaybackTrackProbe
{
    public static void Run()
    {
        var files = (Environment.GetEnvironmentVariable("LAG_MFPROBE") ?? "").Split(';', StringSplitOptions.RemoveEmptyEntries);
        foreach (var path in files)
        {
            Console.WriteLine($"[MfProbe] {System.IO.Path.GetFileName(path)}");
            try
            {
                var src = MediaSource.CreateFromUri(new Uri(path));
                src.OpenAsync().AsTask().GetAwaiter().GetResult();
                var item = new MediaPlaybackItem(src);
                var tracks = item.AudioTracks;
                Console.WriteLine($"[MfProbe]   audio tracks: {tracks.Count}, selected by default: index {tracks.SelectedIndex}");
                for (int i = 0; i < tracks.Count; i++)
                {
                    var t = tracks[i];
                    string enc = "";
                    try
                    {
                        var p = t.GetEncodingProperties();
                        enc = $"{p.Subtype} {p.SampleRate}Hz {p.ChannelCount}ch {p.Bitrate / 1000}kbps";
                    }
                    catch { }
                    Console.WriteLine($"[MfProbe]   [{i}] {(i == tracks.SelectedIndex ? "SELECTED" : "        ")} " +
                                      $"label='{t.Label}' lang='{t.Language}' id='{t.Id}' {enc}");
                }
                src.Dispose();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[MfProbe]   failed: {ex.Message}");
            }
        }
    }
}
