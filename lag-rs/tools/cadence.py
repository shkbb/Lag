"""Frame-cadence report for a clip (same metrics as the 2026-09 Medal comparison).

Usage: python -I cadence.py <clip> [ffprobe.exe]
Prints: frames, duration, avg fps, median interval, distinct intervals (0.1 ms buckets),
% within +/-10% of median, stdev, worst gap, gaps > 25 ms, plus codec/size/bitrate.
"""
import json
import statistics
import subprocess
import sys

clip = sys.argv[1]
ffprobe = sys.argv[2] if len(sys.argv) > 2 else r"E:\project\InstantReplay\InstantReplay\ffmpeg\ffprobe.exe"

meta = json.loads(subprocess.run(
    [ffprobe, "-v", "error", "-show_entries",
     "stream=index,codec_type,codec_name,width,height,bit_rate,color_range:format=duration,bit_rate",
     "-of", "json", clip], capture_output=True, text=True).stdout)
pts = subprocess.run(
    [ffprobe, "-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time",
     "-of", "csv=p=0", clip], capture_output=True, text=True).stdout.split()
t = sorted(float(x) for x in pts if x and x != "N/A")
iv = [(b - a) * 1000.0 for a, b in zip(t, t[1:])]

for s in meta.get("streams", []):
    print(f"stream {s['index']}: {s['codec_type']} {s.get('codec_name')} "
          f"{s.get('width', '')}x{s.get('height', '')} {int(s.get('bit_rate', 0) or 0) / 1e6:.1f} Mbps "
          f"range={s.get('color_range', '')}")
dur = t[-1] - t[0] if len(t) > 1 else 0
print(f"frames {len(t)}, span {dur:.2f}s, avg fps {len(iv) / dur:.1f}" if dur else f"frames {len(t)}")
if iv:
    med = statistics.median(iv)
    near = sum(1 for x in iv if abs(x - med) <= 0.1 * med) / len(iv) * 100
    distinct = len({round(x, 1) for x in iv})
    print(f"median interval {med:.2f} ms (= {1000 / med:.1f} fps)")
    print(f"distinct intervals (0.1 ms) {distinct}, within +/-10% of median {near:.1f}%")
    print(f"stdev {statistics.pstdev(iv):.2f} ms, worst gap {max(iv):.2f} ms, "
          f"gaps > 25 ms: {sum(1 for x in iv if x > 25)}")
    from collections import Counter
    print("most common intervals (ms, count):", Counter(round(x, 2) for x in iv).most_common(5))
