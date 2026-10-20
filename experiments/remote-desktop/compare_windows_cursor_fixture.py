"""Check real product-window cursor pixels against the Windows cursor fixture.

Input: cursor-zones.json from four settled source positions, and zone-0.png
through zone-3.png captured from the same unmoved viewer. Zone 3 provides the
cursor-free background. This acceptance case uses a 1920x1080 primary screen
at 100% DPI, with the operator's independently measured physical viewport.
Only generated fixture regions are inspected; screenshots need not be published.
"""
import argparse
import json
import math
from pathlib import Path

from PIL import Image


def compare(root: Path):
    rows = json.loads((root / "cursor-zones.json").read_text())
    assert len(rows) == 4 and [row["zone"] for row in rows] == [0, 1, 2, 3]
    background = Image.open(root / "zone-3.png").convert("RGB")
    reports = []
    for zone, hotspot in [(1, (5, 7)), (2, (8, 8))]:
        row = rows[zone]
        position, geometry = row["remote"], row["capture"]
        image = Image.open(root / f"zone-{zone}.png").convert("RGB")
        assert image.size == background.size
        assert geometry["pixel_dimensions"] == [1920, 1080]
        for key in ("bounds", "surface_origin", "viewport", "pixel_dimensions"):
            assert geometry[key] == rows[3]["capture"][key], "Viewer moved during comparison"
        assert position["zone"] == zone and position["visible"]
        assert rows[3]["remote"]["zone"] == 3 and not rows[3]["remote"]["visible"]
        sx, sy = geometry["viewport"][2] / 1920, geometry["viewport"][3] / 1080
        left = geometry["surface_origin"][0] + geometry["viewport"][0] + (position["x"] - hotspot[0]) * sx
        top = geometry["surface_origin"][1] + geometry["viewport"][1] + (position["y"] - hotspot[1]) * sy
        checked, differences = 0, []
        for y in range(math.floor(top) - 2, math.ceil(top + 32 * sy) + 2):
            for x in range(math.floor(left) - 2, math.ceil(left + 32 * sx) + 2):
                expected = background.getpixel((x, y))
                px, py = math.floor((x + 0.5 - left) / sx), math.floor((y + 0.5 - top) / sy)
                if 0 <= px < 32 and 0 <= py < 32:
                    if zone == 1:
                        visible = px == 5 or py == 7 or (12 <= px < 28 and 12 <= py < 28)
                        if visible:
                            expected = (255 if py == 7 else 0, 255 if px != 5 else 0, 255)
                    elif py < 16:
                        expected = (255, 255, 255) if px >= 16 else (0, 0, 0)
                    elif px >= 16:
                        expected = tuple(255 - value for value in expected)
                actual = image.getpixel((x, y))
                checked += 1
                if actual != expected:
                    differences.append({"at": [x, y], "shape": [px, py], "actual": actual, "expected": expected})
        reports.append({"zone": zone, "pixels_compared": checked, "different_pixels": len(differences),
                        "first_differences": differences[:5]})
    return reports


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", type=Path)
    args = parser.parse_args()
    results = compare(args.profile)
    (args.profile / "cursor-pixel-comparison.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results))
    if any(result["different_pixels"] for result in results):
        raise SystemExit("Actual product cursor pixels did not match the fixture")
