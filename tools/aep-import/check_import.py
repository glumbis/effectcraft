#!/usr/bin/env python3
"""Check an import: compare what py-aep reads from an .aep / .aepx with the EffectCraft project the
generated script built.

    py -3 check_import.py MyProject.aep MyProject.ecproj [--cli path/to/effectcraft-cli]

Dumps both sides as plain data (comps, layers in order with their type, timing, switches, parent,
transform values at a few times, effects, masks and keyframe counts) and prints every difference.
Exit code 1 when anything differs.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from aep2effectcraft import load  # noqa: E402

SAMPLE_TIMES = (0.0, 0.5, 1.0, 2.0)
TRANSFORM = [
    ("ADBE Anchor Point", "anchor"),
    ("ADBE Position", "position"),
    ("ADBE Scale", "scale"),
    ("ADBE Rotate Z", "rotation"),
    ("ADBE Opacity", "opacity"),
]

DUMP_JSX = r"""
function V(v) { if (v instanceof Array) { var r = []; for (var i = 0; i < v.length; i++) r.push(Math.round(v[i] * 1000) / 1000); return r; } return typeof v === "number" ? Math.round(v * 1000) / 1000 : null; }
var out = {comps: {}};
var times = __TIMES__, tr = __TRANSFORM__;
for (var i = 1; i <= app.project.numItems; i++) {
  var c = app.project.item(i);
  if (!(c instanceof CompItem)) continue;
  var C = {width: c.width, height: c.height, fps: Math.round(c.frameRate * 1000) / 1000, duration: Math.round(c.duration * 1000) / 1000, layers: []};
  for (var j = 1; j <= c.numLayers; j++) {
    var L = c.layer(j), D = {name: L.name, inPoint: V(L.inPoint), outPoint: V(L.outPoint), startTime: V(L.startTime), enabled: L.enabled, parent: L.parent ? L.parent.name : null, values: {}, keys: {}, effects: [], masks: 0};
    try { var tg = L.property("ADBE Transform Group");
      for (var k = 0; k < tr.length; k++) { var p = tg ? tg.property(tr[k][0]) : null; if (!p) continue;
        if (p.expressionEnabled && p.expression) { D.keys[tr[k][1]] = "expr"; continue; }
        D.keys[tr[k][1]] = p.numKeys; var vs = []; for (var t = 0; t < times.length; t++) vs.push(V(p.valueAtTime(times[t], true))); D.values[tr[k][1]] = vs; } } catch (e) {}
    try { var fx = L.property("ADBE Effect Parade"); for (var k = 1; k <= fx.numProperties; k++) D.effects.push(fx.property(k).matchName); } catch (e) {}
    try { D.masks = L.property("ADBE Mask Parade").numProperties; } catch (e) {}
    C.layers.push(D);
  }
  out.comps[c.name] = C;
}
JSON.stringify(out);
"""


def r3(v):
    if isinstance(v, (list, tuple)):
        return [round(float(x), 3) for x in v]
    if isinstance(v, (int, float)):
        return round(float(v), 3)
    return None


def dump_aep(path):
    p = load(path).project
    out = {"comps": {}}
    for c in p.compositions:
        C = {"width": c.width, "height": c.height, "fps": round(float(c.frame_rate), 3), "duration": round(float(c.duration), 3), "layers": []}
        for L in c.layers:
            D = {
                "name": L.name,
                "inPoint": r3(L.in_point),
                "outPoint": r3(L.out_point),
                "startTime": r3(L.start_time),
                "enabled": bool(getattr(L, "enabled", True)),
                "parent": L.parent.name if getattr(L, "parent", None) is not None else None,
                "values": {},
                "keys": {},
                "effects": [],
                "masks": 0,
            }
            try:
                tg = L.property("ADBE Transform Group")
                for mn, key in TRANSFORM:
                    pr = tg.property(mn)
                    if pr is None:
                        continue
                    if pr.expression and pr.expression_enabled:
                        D["keys"][key] = "expr"
                        continue
                    D["keys"][key] = len(pr.keyframes)
                    D["values"][key] = [r3(pr.value_at_time(t, True) if pr.keyframes else pr.value) for t in SAMPLE_TIMES]
            except Exception:
                pass
            try:
                D["effects"] = [fx.match_name for fx in L.property("ADBE Effect Parade").properties]
            except Exception:
                pass
            try:
                D["masks"] = len(list(L.property("ADBE Mask Parade").properties))
            except Exception:
                pass
            C["layers"].append(D)
        out["comps"][c.name] = C
    return out


def dump_ec(ecproj, cli):
    code = DUMP_JSX.replace("__TIMES__", json.dumps(list(SAMPLE_TIMES))).replace("__TRANSFORM__", json.dumps(TRANSFORM))
    fd, tmp = tempfile.mkstemp(suffix=".jsx")
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(code)
    try:
        r = subprocess.run([cli, "script", tmp, ecproj, "--json"], capture_output=True, text=True, encoding="utf-8")
    finally:
        os.remove(tmp)
    res = json.loads(r.stdout)
    result = res.get("result")
    return json.loads(result) if isinstance(result, str) else result


def close(a, b, tol=0.05):
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            # 2D vs 3D values (AE keeps a z of 0 on 2D layers).
            n = min(len(a), len(b))
            return all(close(x, y, tol) for x, y in zip(a[:n], b[:n]))
        return all(close(x, y, tol) for x, y in zip(a, b))
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return abs(a - b) <= tol * max(1.0, abs(a), abs(b)) * 0.01 + tol
    return a == b


def compare(a, e):
    diffs = []
    for name, ca in a["comps"].items():
        ce = e["comps"].get(name)
        if ce is None:
            diffs.append("comp %r missing" % name)
            continue
        for k in ("width", "height", "fps", "duration"):
            if not close(ca[k], ce[k]):
                diffs.append("comp %r %s: aep %r, effectcraft %r" % (name, k, ca[k], ce[k]))
        la, le = ca["layers"], ce["layers"]
        if [x["name"] for x in la] != [x["name"] for x in le]:
            diffs.append("comp %r layer order: aep %r, effectcraft %r" % (name, [x["name"] for x in la], [x["name"] for x in le]))
        for x, y in zip(la, le):
            tag = "%s > %s" % (name, x["name"])
            for k in ("inPoint", "outPoint", "startTime", "enabled", "parent", "masks"):
                if not close(x[k], y[k]):
                    diffs.append("%s %s: aep %r, effectcraft %r" % (tag, k, x[k], y[k]))
            if x["effects"] != y["effects"]:
                diffs.append("%s effects: aep %r, effectcraft %r" % (tag, x["effects"], y["effects"]))
            for key in x["keys"]:
                if key not in y["keys"]:
                    continue
                if x["keys"][key] != y["keys"][key]:
                    diffs.append("%s %s keyframes: aep %r, effectcraft %r" % (tag, key, x["keys"][key], y["keys"][key]))
                elif key in x["values"] and key in y["values"] and not close(x["values"][key], y["values"][key]):
                    diffs.append("%s %s at %r: aep %r, effectcraft %r" % (tag, key, list(SAMPLE_TIMES), x["values"][key], y["values"][key]))
    return diffs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("project")
    ap.add_argument("ecproj")
    ap.add_argument("--cli", default="effectcraft-cli")
    a = ap.parse_args()
    diffs = compare(dump_aep(a.project), dump_ec(a.ecproj, a.cli))
    for d in diffs:
        print(d)
    print("%d differences" % len(diffs))
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main())
