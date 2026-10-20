"""Check actual viewer screenshots of the owned white Mac cursor fixture.

Run after the seven live cursor phases. The region must be entirely inside the
fixture's white client area, including both cursor positions. Does not edit or
retain images; emits only pixel counts and bounds. This checks a visible region,
not whether the encoder itself included a cursor anywhere in its full frame.
"""
import argparse
import json
from pathlib import Path

from PIL import Image, ImageChops


def analyze(result, region, expected_move):
    stages = result["rounds"][0]["cursor_stages"]
    phases = ["arrow", "custom", "hidden", "shown", "moved", "move_back", "arrow_restored"]
    assert [stage["phase"] for stage in stages] == phases, "Missing or reordered phases"
    left, top, right, bottom = region
    assert 0 <= left < right and 0 <= top < bottom and expected_move > 0
    images = {}
    dimensions = None
    for stage in stages:
        with Image.open(stage["screenshot"]) as source:
            assert right <= source.width and bottom <= source.height
            if dimensions is None:
                dimensions = source.size
            assert source.size == dimensions, "Viewer geometry changed"
            images[stage["phase"]] = source.convert("RGB").crop(region)
    background = images["hidden"]
    assert background.getextrema() == ((255, 255),) * 3, "Hidden fixture is not uniformly white"
    measurements = []
    for phase in phases:
        difference = ImageChops.difference(images[phase], background)
        mask = difference.convert("L").point(lambda value: 255 if value > 5 else 0)
        bounds = mask.getbbox()
        measurements.append({
            "phase": phase,
            "changed_pixels": mask.histogram()[255],
            "bounds": None if bounds is None else [
                bounds[0] + left, bounds[1] + top,
                bounds[2] + left, bounds[3] + top,
            ],
        })
    by = {entry["phase"]: entry for entry in measurements}
    assert by["hidden"]["changed_pixels"] == 0
    for phase in phases:
        if phase != "hidden":
            assert by[phase]["changed_pixels"] > 0, f"Cursor missing: {phase}"
    restoration = []
    for first, second in [("arrow", "arrow_restored"), ("custom", "shown"), ("custom", "move_back")]:
        assert by[first]["bounds"] == by[second]["bounds"], f"Restored cursor moved: {second}"
        bounds = by[first]["bounds"]
        cursor_box = (bounds[0] - left, bounds[1] - top, bounds[2] - left, bounds[3] - top)
        assert images[first].crop(cursor_box).tobytes() == images[second].crop(cursor_box).tobytes(), f"Restored cursor differs: {second}"
        # Lossy desktop video can change a background boundary by a channel
        # value even when the lossless cursor pixels themselves are identical.
        delta = max(high for low, high in ImageChops.difference(images[first], images[second]).getextrema())
        assert delta <= 5, f"Fixture background changed: {second}"
        restoration.append({"phase": second, "cursor_pixels_identical": True, "region_max_channel_delta": delta})
    custom, moved = by["custom"], by["moved"]
    assert custom["bounds"] != by["arrow"]["bounds"], "Shape did not change"
    assert moved["changed_pixels"] == custom["changed_pixels"]
    assert moved["bounds"] == [
        custom["bounds"][0] + expected_move, custom["bounds"][1],
        custom["bounds"][2] + expected_move, custom["bounds"][3],
    ], "Cursor movement differs from expected display-space offset"
    return {"passed": True, "region": region, "background_rgb": [255] * 3,
            "threshold_luma": 5, "expected_move_pixels": expected_move,
            "stages": measurements, "restoration": restoration,
            "scope": "owned white fixture region only"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result", type=Path)
    parser.add_argument("--region", type=int, nargs=4, required=True)
    parser.add_argument("--expected-move", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = analyze(json.loads(args.result.read_text(encoding="utf-8")), args.region, args.expected_move)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))
