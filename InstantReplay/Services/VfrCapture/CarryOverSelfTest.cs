using System;
using System.Collections.Generic;
using FFmpeg.AutoGen;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Headless checks for the carry-over stitching logic (LAG_CARRYTEST=1). Pure bookkeeping — no GPU,
/// no capture, no FFmpeg — so it runs anywhere and pins down the parts that are easy to get subtly
/// wrong: keyframe-aware trimming, the seam offset, and stream-compatibility rules.
/// </summary>
public static class CarryOverSelfTest
{
    private static int _failed;

    public static void Run()
    {
        Console.WriteLine("[CarryTest] carry-over stitching checks");
        TrimKeepsWholeTailWhenShorter();
        TrimStartsOnKeyframe();
        TrimDropsUndecodableTail();
        TrimCutsAudioToTheSameOrigin();
        RebaseStartsAtZero();
        ShiftMovesEveryPacket();
        CompatibilityRules();

        Console.WriteLine(_failed == 0
            ? "[CarryTest] ALL PASSED"
            : $"[CarryTest] {_failed} CHECK(S) FAILED");
    }

    // ── checks ──────────────────────────────────────────────────────────────

    private static void TrimKeepsWholeTailWhenShorter()
    {
        var seg = Segment(startUs: 0, frames: 60, fps: 60, keyEvery: 30);   // 1s of footage
        var t = seg.TrimmedToLast(5);
        Check("short tail is returned untouched", ReferenceEquals(t, seg));
    }

    private static void TrimStartsOnKeyframe()
    {
        // 10s at 60fps, keyframe every 2s. Ask for the last 3s.
        var seg = Segment(0, 600, 60, 120);
        var t = seg.TrimmedToLast(3);
        Check("trim returned a segment", t != null);
        if (t == null) return;
        Check("trim opens on a keyframe", t.Video[0].IsKeyframe);
        Check($"trim keeps >= 3s (got {t.DurationSeconds:0.00}s)", t.DurationSeconds >= 3.0);
        // Keyframe-aligned, so it may reach one GOP (2s) further back, never more.
        Check($"trim keeps <= 5s (got {t.DurationSeconds:0.00}s)", t.DurationSeconds <= 5.0);
        Check("original is unchanged", seg.Video.Count == 600);
    }

    private static void TrimDropsUndecodableTail()
    {
        var seg = Segment(0, 60, 60, keyEvery: 0);   // no keyframes at all
        Check("keyframe-less tail is dropped", seg.TrimmedToLast(1) == null);
        Check("zero window is dropped", Segment(0, 600, 60, 120).TrimmedToLast(0) == null);
    }

    private static void TrimCutsAudioToTheSameOrigin()
    {
        var seg = Segment(0, 600, 60, 120, withAudio: true);
        var t = seg.TrimmedToLast(3);
        Check("trimmed segment has audio", t is { Audio.Count: > 0 });
        if (t == null || t.Audio.Count == 0) return;
        long vStart = t.Video[0].PtsUs;
        bool clean = true;
        foreach (var p in t.Audio[0].Packets) if (p.PtsUs < vStart) clean = false;
        Check("no audio packet predates the video start", clean);
    }

    private static void RebaseStartsAtZero()
    {
        var seg = Segment(startUs: 7_000_000, frames: 120, fps: 60, keyEvery: 60, withAudio: true);
        var v = seg.VideoRebased();
        Check("re-based video starts at 0", v[0].PtsUs == 0);
        Check("re-based video keeps its length",
              Math.Abs(v[^1].PtsUs - (seg.EndUs - seg.StartUs)) < 2);
        var a = seg.AudioRebased(seg.Audio[0]);
        Check("re-based audio starts at/after 0", a.Count > 0 && a[0].PtsUs >= 0);
    }

    private static void ShiftMovesEveryPacket()
    {
        var seg = Segment(0, 10, 60, 10);
        var shifted = ReplayStitcher.Shift(seg.Video, 5_000_000);
        bool ok = shifted.Count == seg.Video.Count;
        for (int i = 0; i < shifted.Count && ok; i++)
            ok = shifted[i].PtsUs == seg.Video[i].PtsUs + 5_000_000
              && shifted[i].DtsUs == seg.Video[i].DtsUs + 5_000_000;
        Check("shift offsets pts and dts alike", ok);
    }

    private static void CompatibilityRules()
    {
        var a = VideoSpec(1920, 1080, new byte[] { 1, 2, 3 });
        Check("same params concat directly", ReplayStitcher.CanConcatDirect(a, VideoSpec(1920, 1080, new byte[] { 1, 2, 3 })));
        Check("different size needs a re-encode", !ReplayStitcher.CanConcatDirect(a, VideoSpec(1280, 720, new byte[] { 1, 2, 3 })));
        Check("different extradata needs a re-encode", !ReplayStitcher.CanConcatDirect(a, VideoSpec(1920, 1080, new byte[] { 9, 9 })));

        var au = AudioSpec(48000, 2, new byte[] { 1, 2 });
        Check("same audio config concats", ReplayStitcher.CanConcatAudio(au, AudioSpec(48000, 2, new byte[] { 1, 2 })));
        Check("different sample rate does not", !ReplayStitcher.CanConcatAudio(au, AudioSpec(44100, 2, new byte[] { 1, 2 })));
        Check("different channel count does not", !ReplayStitcher.CanConcatAudio(au, AudioSpec(48000, 1, new byte[] { 1, 2 })));
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    private static StreamSpec VideoSpec(int w, int h, byte[] extra)
        => new(AVCodecID.AV_CODEC_ID_H264, true, w, h, 0, 0, extra);

    private static StreamSpec AudioSpec(int rate, int ch, byte[] extra)
        => new(AVCodecID.AV_CODEC_ID_AAC, false, 0, 0, rate, ch, extra);

    /// <summary>Synthetic tail: <paramref name="frames"/> packets at <paramref name="fps"/>, a keyframe
    /// every <paramref name="keyEvery"/> frames (0 = none), optionally with a 50 Hz audio track.</summary>
    private static CarryOverSegment Segment(long startUs, int frames, int fps, int keyEvery, bool withAudio = false)
    {
        long step = 1_000_000L / fps;
        var video = new List<EncodedPacket>(frames);
        for (int i = 0; i < frames; i++)
        {
            long pts = startUs + i * step;
            bool key = keyEvery > 0 && i % keyEvery == 0;
            video.Add(new EncodedPacket(new byte[16], pts, pts, key));
        }

        var audio = new List<CarryAudioTrack>();
        if (withAudio)
        {
            var pk = new List<EncodedPacket>();
            long dur = frames * step;
            for (long t = 0; t < dur; t += 20_000)
                pk.Add(new EncodedPacket(new byte[8], startUs + t, startUs + t, true));
            audio.Add(new CarryAudioTrack(AudioSpec(48000, 2, new byte[] { 1, 2 }), pk, "System"));
        }

        return new CarryOverSegment(VideoSpec(1920, 1080, new byte[] { 1, 2, 3 }), video, audio,
                                    wasGame: true, targetName: "TestGame", targetExe: "test.exe");
    }

    private static void Check(string what, bool ok)
    {
        if (!ok) _failed++;
        Console.WriteLine($"  [{(ok ? "PASS" : "FAIL")}] {what}");
    }
}
