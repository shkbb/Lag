using System;
using System.Collections.Generic;

namespace Lag.Services.VfrCapture;

/// <summary>One audio track carried over from a finished capture session.</summary>
public sealed record CarryAudioTrack(StreamSpec Spec, List<EncodedPacket> Packets, string Title);

/// <summary>
/// The tail of a capture session that ended, kept alive so it can still be saved.
///
/// Why this exists: game and desktop capture are deliberately separate sessions (different WGC
/// source, encoder and clock) — quitting a game tears the game session down and starts a desktop
/// one. Without this, the last seconds of the game are gone the instant you alt-F4, which is
/// exactly the footage you usually want. So on teardown we keep the session's encoded packets
/// here and <see cref="VfrReplayEngine.SaveReplay"/> stitches them in front of the live buffer.
///
/// Timestamps stay ABSOLUTE in the dead session's own clock — each session has its own origin, so
/// carry-over and live timestamps are not comparable; the save path re-bases both.
///
/// Memory is bounded by <see cref="TrimToLast"/>: the engine keeps carry-over + live ≤ the user's
/// configured buffer length, so the tail shrinks as the new session fills and costs nothing extra.
/// Immutable: trimming returns a new segment, so a save in flight is never disturbed.
/// </summary>
public sealed class CarryOverSegment
{
    public StreamSpec VideoSpec { get; }
    public List<EncodedPacket> Video { get; }
    public List<CarryAudioTrack> Audio { get; }

    /// <summary>True when the capture that produced this tail was a game (vs the desktop).</summary>
    public bool WasGame { get; }
    /// <summary>Display name of the captured target, for logs and clip metadata.</summary>
    public string TargetName { get; }
    public string? TargetExe { get; }

    public CarryOverSegment(StreamSpec videoSpec, List<EncodedPacket> video,
                            List<CarryAudioTrack> audio, bool wasGame, string targetName, string? targetExe)
    {
        VideoSpec = videoSpec; Video = video; Audio = audio;
        WasGame = wasGame; TargetName = targetName; TargetExe = targetExe;
    }

    public bool IsEmpty => Video.Count == 0;
    public long StartUs => Video.Count == 0 ? 0 : Video[0].PtsUs;
    public long EndUs => Video.Count == 0 ? 0 : Video[^1].PtsUs;
    public double DurationSeconds => Video.Count == 0 ? 0 : (EndUs - StartUs) / 1_000_000.0;

    /// <summary>
    /// Returns the last <paramref name="seconds"/> of the tail, starting at a keyframe (a clip must
    /// open on one to decode); audio is cut to the same origin. Returns <c>this</c> when the tail is
    /// already shorter, or null when nothing decodable is left — the caller then drops the segment.
    ///
    /// Returns a NEW segment rather than mutating: the save path reads a segment without holding a
    /// lock, so trimming must never pull packets out from under it.
    /// </summary>
    public CarryOverSegment? TrimmedToLast(double seconds)
    {
        if (Video.Count == 0 || seconds <= 0) return null;

        long cutoff = EndUs - (long)(seconds * 1_000_000);
        // Latest keyframe at or before the cutoff keeps the whole window decodable; if every
        // keyframe is newer, fall back to the earliest one (a shorter tail beats none).
        // Scanning for the keyframe FIRST also settles the two edge cases: a tail already
        // inside the window resolves to index 0 and is returned as-is, while a tail with no
        // keyframe anywhere is rejected here instead of being handed on as an unplayable clip.
        int start = -1;
        for (int i = 0; i < Video.Count; i++)
        {
            if (!Video[i].IsKeyframe) continue;
            if (Video[i].PtsUs <= cutoff) start = i;
            else { if (start < 0) start = i; break; }
        }
        if (start < 0) return null;                  // no keyframe at all → undecodable
        if (start == 0) return this;

        var video = Video.GetRange(start, Video.Count - start);
        long newStart = video[0].PtsUs;
        var audio = new List<CarryAudioTrack>(Audio.Count);
        foreach (var t in Audio)
        {
            var kept = t.Packets.FindAll(p => p.PtsUs >= newStart);
            if (kept.Count > 0) audio.Add(t with { Packets = kept });
        }
        return new CarryOverSegment(VideoSpec, video, audio, WasGame, TargetName, TargetExe);
    }

    /// <summary>Re-bases the tail so it starts at zero, ready to be written as the head of a clip.</summary>
    public List<EncodedPacket> VideoRebased()
    {
        long b = StartUs;
        var list = new List<EncodedPacket>(Video.Count);
        foreach (var p in Video) list.Add(p with { PtsUs = p.PtsUs - b, DtsUs = p.DtsUs - b });
        return list;
    }

    /// <summary>Re-bases one audio track to the same zero point as <see cref="VideoRebased"/>.</summary>
    public List<EncodedPacket> AudioRebased(CarryAudioTrack track)
    {
        long b = StartUs;
        var list = new List<EncodedPacket>(track.Packets.Count);
        foreach (var p in track.Packets)
        {
            if (p.PtsUs < b) continue;
            list.Add(p with { PtsUs = p.PtsUs - b, DtsUs = p.DtsUs - b });
        }
        return list;
    }
}
