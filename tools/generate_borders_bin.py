#!/usr/bin/env python3
"""
Pre-bake apps/android-client/assets/globe/countries_50m.geojson into a compact
binary (borders.bin) for the Android globe.

Why: parsing the ~3 MB geojson with org.json at app startup allocates hundreds
of thousands of small JSONArray/Double objects — a 20-30 MB transient spike on
launch. The binary form is ~0.8 MB, loads with one read and a tight float loop,
and keeps the app well inside its memory budget.

Format (little-endian):
  magic        4 bytes   "ZNBR"
  ring_count   uint32
  points[i]    uint32    point count per ring (ring_count of these)
  rings...     float32 lat, float32 lon  (sum of points[i] pairs, in order)

Only outer rings are kept, mirroring GlobeView's loader. Regenerate whenever
countries_50m.geojson changes:

  python tools/generate_borders_bin.py
"""
import json
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "apps" / "android-client" / "assets" / "globe" / "countries_50m.geojson"
DST = ROOT / "apps" / "android-client" / "assets" / "globe" / "borders.bin"


def outer_rings(geom):
    coords = geom.get("coordinates") or []
    gtype = geom.get("type")
    if gtype == "Polygon":
        if coords:
            yield coords[0]
    elif gtype == "MultiPolygon":
        for poly in coords:
            if poly:
                yield poly[0]


def main():
    with open(SRC, "r", encoding="utf-8") as f:
        root = json.load(f)
    features = root.get("features") or []
    rings = []
    for feat in features:
        geom = (feat or {}).get("geometry") or {}
        for ring in outer_rings(geom):
            pts = []
            for pt in ring:
                if not isinstance(pt, (list, tuple)) or len(pt) < 2:
                    continue
                lon = float(pt[0])
                lat = float(pt[1])
                if not (-90.0 <= lat <= 90.0):
                    continue
                if not (-180.0 <= lon <= 180.0):
                    # Normalise wrapped longitudes into [-180, 180].
                    lon = ((lon + 180.0) % 360.0) - 180.0
                pts.append((lat, lon))
            if len(pts) >= 3:
                rings.append(pts)
    with open(DST, "wb") as f:
        f.write(b"ZNBR")
        f.write(struct.pack("<I", len(rings)))
        for pts in rings:
            f.write(struct.pack("<I", len(pts)))
        for pts in rings:
            payload = struct.pack("<%df" % (len(pts) * 2),
                                 *[c for pt in pts for c in pt])
            f.write(payload)
    total = sum(len(r) for r in rings)
    print(f"borders.bin written: {len(rings)} rings, {total} vertices, "
          f"{DST.stat().st_size / 1024:.1f} KB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
