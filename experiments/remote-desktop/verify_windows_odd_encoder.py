"""Verify generated streams from windows_odd_encoder.cpp with FFmpeg.

Usage: python verify_windows_odd_encoder.py OWNED_OUTPUT_DIRECTORY
This decodes generated pixels only. It never reads screens or sends input.
"""
import json
import pathlib
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve(strict=True)
reports = []
for width, height in [(1298, 754), (1297, 754), (1298, 753), (1297, 753)]:
    coded_width, coded_height = (width + 1) & ~1, (height + 1) & ~1
    decoded = subprocess.run([
        'ffmpeg', '-v', 'error', '-threads', '1', '-i', str(root/f'gpu-{width}x{height}.h264'),
        '-pix_fmt', 'nv12', '-f', 'rawvideo', '-',
    ], capture_output=True, timeout=30)
    assert decoded.returncode == 0, decoded.stderr.decode(errors='replace')
    frame_bytes = coded_width * coded_height * 3 // 2
    assert len(decoded.stdout) == frame_bytes * 3
    frames = []
    for index in range(3):
        plane = decoded.stdout[index*frame_bytes:(index+1)*frame_bytes]
        # Last visible edge and replicated padding must both survive encoding.
        points = {'right': (width-1, height//2), 'bottom': (width//2, height-1),
                  'corner': (width-1, height-1), 'padded_corner': (coded_width-1, coded_height-1),
                  'inside_right': (width-2, height//2), 'inside_bottom': (width//2, height-2),
                  'center': (width//2, height//2)}
        luma = {name: plane[y*coded_width+x] for name, (x, y) in points.items()}
        assert all(luma[name] > 200 for name in ('right', 'bottom', 'corner', 'padded_corner')), luma
        assert all(45 < luma[name] < 100 for name in ('inside_right', 'inside_bottom', 'center')), luma
        frames.append(luma)
    reports.append({'visible': [width, height], 'coded': [coded_width, coded_height],
                    'frames': frames, 'passed': True})
(root/'decoded-edges.json').write_text(json.dumps(reports, indent=2))
print(json.dumps(reports))
