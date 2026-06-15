using System;
using System.Collections.Generic;

namespace Lag.Services.VfrCapture;

/// <summary>
/// A rolling in-memory buffer of the last N seconds of encoded video packets — the actual
/// "instant replay" store. Packets carry real (variable) timestamps, so the buffer holds the
/// true game footage, not CFR padding.
///
/// Trimming is keyframe-aware: a clip must start on a keyframe to be decodable, so we always keep
/// one keyframe older than the cutoff. On save we hand back everything from the latest keyframe
/// at or before (now − N seconds) through the newest packet, re-based to start at zero.
///
/// Thread-safe: the encoder thread calls <see cref="Add"/>, the hotkey thread calls
/// <see cref="Snapshot"/>.
/// </summary>
public sealed class PacketRingBuffer
{
    private readonly long _bufferUs;
    private readonly object _gate = new();
    private readonly LinkedList<EncodedPacket> _packets = new();
    private readonly Queue<LinkedListNode<EncodedPacket>> _keyframes = new();

    public PacketRingBuffer(double bufferSeconds)
    {
        _bufferUs = (long)(Math.Max(5, bufferSeconds) * 1_000_000);
    }

    /// <summary>Approximate buffered span in seconds (newest − oldest pts).</summary>
    public double BufferedSeconds
    {
        get
        {
            lock (_gate)
            {
                if (_packets.First == null || _packets.Last == null) return 0;
                return (_packets.Last.Value.PtsUs - _packets.First.Value.PtsUs) / 1_000_000.0;
            }
        }
    }

    public void Add(EncodedPacket packet)
    {
        lock (_gate)
        {
            var node = _packets.AddLast(packet);
            if (packet.IsKeyframe) _keyframes.Enqueue(node);

            long cutoff = packet.PtsUs - _bufferUs;
            // Drop the oldest keyframe segment while the NEXT keyframe already covers the cutoff
            // (i.e. a full window starting at a keyframe ≤ cutoff is still available without it).
            while (_keyframes.Count >= 2)
            {
                var first = _keyframes.Peek();
                // Peek the second keyframe by dequeuing/re-checking is awkward; track via its node.
                var secondNode = SecondKeyframeNode();
                if (secondNode == null || secondNode.Value.PtsUs > cutoff) break;

                _keyframes.Dequeue();                 // first keyframe no longer needed
                // Remove all packets before the second keyframe.
                while (_packets.First != null && !ReferenceEquals(_packets.First, secondNode))
                    _packets.RemoveFirst();
            }
        }
    }

    private LinkedListNode<EncodedPacket>? SecondKeyframeNode()
    {
        // _keyframes is FIFO; the second element. Cheap because we only call it during trim.
        int i = 0;
        foreach (var n in _keyframes) { if (i++ == 1) return n; }
        return null;
    }

    /// <summary>
    /// Returns the replay packets: from the latest keyframe at or before (newest − N seconds)
    /// through the newest, with PTS/DTS re-based so the clip starts at 0. Empty if nothing
    /// buffered yet. The returned list is a private copy — safe to mux without holding the lock.
    /// </summary>
    public List<EncodedPacket> Snapshot() => Snapshot(out _);

    /// <summary>As <see cref="Snapshot()"/>, and reports the ABSOLUTE pts the clip was re-based from
    /// (<paramref name="absBaseUs"/>) so a companion stream (audio) can re-base to the same origin
    /// and stay in sync.</summary>
    public List<EncodedPacket> Snapshot(out long absBaseUs)
    {
        absBaseUs = 0;
        lock (_gate)
        {
            var result = new List<EncodedPacket>();
            if (_packets.Last == null) return result;

            long newest = _packets.Last.Value.PtsUs;
            long cutoff = newest - _bufferUs;

            // Latest keyframe with pts <= cutoff, else the earliest keyframe.
            LinkedListNode<EncodedPacket>? start = null;
            foreach (var kf in _keyframes)
            {
                if (kf.Value.PtsUs <= cutoff) start = kf;
                else break;
            }
            if (start == null && _keyframes.Count > 0)
            {
                foreach (var kf in _keyframes) { start = kf; break; }
            }
            if (start == null) return result; // no keyframe yet — nothing decodable

            long baseUs = start.Value.PtsUs;
            absBaseUs = baseUs;
            for (var n = start; n != null; n = n.Next)
            {
                var p = n.Value;
                result.Add(p with { PtsUs = p.PtsUs - baseUs, DtsUs = p.DtsUs - baseUs });
            }
            return result;
        }
    }

    /// <summary>Returns every buffered packet at or after <paramref name="absBaseUs"/>, re-based to
    /// that origin. Used for the audio track so it lines up with the video clip's zero point;
    /// negative-pts packets (slightly before the video keyframe) are dropped.</summary>
    public List<EncodedPacket> SnapshotRebasedFrom(long absBaseUs)
    {
        lock (_gate)
        {
            var result = new List<EncodedPacket>();
            for (var n = _packets.First; n != null; n = n.Next)
            {
                var p = n.Value;
                if (p.PtsUs < absBaseUs) continue;
                result.Add(p with { PtsUs = p.PtsUs - absBaseUs, DtsUs = p.DtsUs - absBaseUs });
            }
            return result;
        }
    }

    public void Clear()
    {
        lock (_gate) { _packets.Clear(); _keyframes.Clear(); }
    }
}
