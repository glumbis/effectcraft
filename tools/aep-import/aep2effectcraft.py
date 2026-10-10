#!/usr/bin/env python3
"""Convert an After Effects project (.aep or .aepx) into a script that rebuilds it in EffectCraft.

    py -3 aep2effectcraft.py MyProject.aep -o MyProject.jsx

The project is read with py-aep (https://github.com/forticheprod/py-aep, MIT; install it with
`py -m pip install py-aep`); .aepx files are first repacked into the binary layout (aepx.py). The
output is an After Effects-style ExtendScript (.jsx) written against the scripting object model
EffectCraft implements (app.project.items.addComp, layer.property(...).setValueAtTime, ...), so
EffectCraft's own commands build every item, layer, keyframe and effect. Each step runs in its own
try/catch: what EffectCraft can't do (an unknown effect, a custom property type) is skipped and
listed at the end instead of stopping the import.

Exit codes: 0 converted, 2 bad arguments or unreadable project, 3 py-aep missing.
"""

import argparse
import json
import math
import os
import sys
import tempfile

try:
    import py_aep
    from py_aep import KeyframeInterpolationType, PropertyType, PropertyValueType
except ImportError:
    sys.stderr.write("py-aep is not installed. Install it with:  py -m pip install py-aep\n")
    sys.exit(3)

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import aepx  # noqa: E402

# Groups that are rebuilt elsewhere or that EffectCraft has no scripting equivalent for.
SKIP_GROUPS = {
    "ADBE MTrackers",  # motion tracker data
    "ADBE Effect Mask Parade",  # effect mask references
    "ADBE Data Group",
    "ADBE Essential Properties",
    "ADBE Layer Sets",
    "ADBE Source Options Group",  # Essential Graphics media replacement
}
WARN_GROUPS = {"ADBE Layer Styles": "layer styles"}
NO_VALUE_TYPES = {
    PropertyValueType.NO_VALUE,
    PropertyValueType.UNKNOWN,
    PropertyValueType.LRDR,
    PropertyValueType.LITM,
    PropertyValueType.GIDE,
}


class Unsupported(Exception):
    pass


def has_content(group, depth=0):
    """Whether anything under `group` was changed from AE's defaults (a group that's switched on,
    a modified, keyed or expression-driven property)."""
    if depth > 8:
        return False
    try:
        children = list(group.properties)
    except Exception:
        return False
    for c in children:
        if getattr(c, "property_type", None) == PropertyType.PROPERTY:
            try:
                if c.keyframes or getattr(c, "expression", "") or getattr(c, "is_modified", False):
                    return True
            except Exception:
                continue
        else:
            if getattr(c, "can_set_enabled", False) and getattr(c, "enabled", False):
                return True
            if has_content(c, depth + 1):
                return True
    return False


def js(v):
    """A JavaScript literal for a plain value."""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(int(v))  # also py-aep's IntEnum values
    if isinstance(v, float):
        v = float(v)
        if not math.isfinite(v):
            return "0"
        r = round(v, 6)
        return str(int(r)) if r == int(r) and abs(r) < 1e15 else repr(r)
    if isinstance(v, str):
        return json.dumps(v)
    if isinstance(v, (list, tuple)):
        return "[" + ", ".join(js(x) for x in v) + "]"
    if v is None:
        return "null"
    raise Unsupported("a %s value" % type(v).__name__)


def enum_js(enum_name, value):
    """`EnumName.MEMBER` for an AE enum value (py-aep enum or int)."""
    name = getattr(value, "name", None)
    if name is None:
        cls = getattr(py_aep, enum_name, None)
        try:
            name = cls(value).name if cls is not None else None
        except Exception:
            name = None
    return "%s.%s" % (enum_name, name) if name else js(int(value))


def text_document_js(td):
    """A JavaScript expression that builds the TextDocument `td`."""
    fields = [("text", "text")]
    simple = [
        ("font", "font"),
        ("font_size", "fontSize"),
        ("fill_color", "fillColor"),
        ("stroke_color", "strokeColor"),
        ("stroke_width", "strokeWidth"),
        ("apply_fill", "applyFill"),
        ("apply_stroke", "applyStroke"),
        ("stroke_over_fill", "strokeOverFill"),
        ("tracking", "tracking"),
        ("faux_bold", "fauxBold"),
        ("faux_italic", "fauxItalic"),
        ("all_caps", "allCaps"),
        ("small_caps", "smallCaps"),
        ("horizontal_scale", "horizontalScale"),
        ("vertical_scale", "verticalScale"),
        ("baseline_shift", "baselineShift"),
        ("tsume", "tsume"),
        ("superscript", "superscript"),
        ("subscript", "subscript"),
    ]
    parts = []
    for attr, key in fields + simple:
        try:
            v = getattr(td, attr)
        except Exception:
            continue
        if v is None:
            continue
        try:
            parts.append("%s: %s" % (key, js(v)))
        except Unsupported:
            pass
    try:
        if not td.auto_leading and td.leading:
            parts.append("leading: %s" % js(td.leading))
    except Exception:
        pass
    try:
        parts.append("justification: %s" % enum_js("ParagraphJustification", td.justification))
    except Exception:
        pass
    return "TD({%s})" % ", ".join(parts)


def value_js(prop, v):
    """A JavaScript expression for one value of `prop`."""
    vt = prop.property_value_type
    if vt == PropertyValueType.SHAPE:
        return "SH(%s, %s, %s, %s)" % (js(v.vertices), js(v.in_tangents), js(v.out_tangents), js(bool(v.closed)))
    if vt == PropertyValueType.TEXT_DOCUMENT:
        return text_document_js(v)
    if vt == PropertyValueType.MARKER:
        return "MV(%s)" % js(
            {
                "comment": v.comment or "",
                "duration": float(v.duration or 0),
                "chapter": v.chapter or "",
                "url": v.url or "",
                "frameTarget": v.frame_target or "",
                "cuePointName": v.cue_point_name or "",
                "label": int(v.label or 0),
            }
        )
    if vt == PropertyValueType.CUSTOM_VALUE:
        raise Unsupported("a custom value (curves, gradients and the like)")
    if isinstance(v, bool):
        return js(int(v))
    return js(v)


class Converter:
    def __init__(self, project, source_path):
        self.p = project
        self.out = []
        self.n = 0
        self.notes = []
        self.src_dir = os.path.dirname(os.path.abspath(source_path))
        self.item_var = {}
        self.null_layer = False
        self.stats = {"comps": 0, "layers": 0, "footage": 0, "missing": 0, "effects": 0, "keyframes": 0}

    # ---------------------------------------------------------------- output helpers
    def emit(self, line, depth=0):
        self.out.append("  " * depth + line)

    def var(self, prefix):
        self.n += 1
        return "%s%d" % (prefix, self.n)

    def step(self, what, body, depth=0):
        """One guarded statement block."""
        self.emit("T(%s, function () { %s });" % (js(what), body), depth)

    # ---------------------------------------------------------------- items
    def resolve_file(self, path):
        """The footage file on this machine: the saved absolute path, else the same file found
        relative to the project (its last 1-4 path components below the project folder)."""
        if not path:
            return None
        if os.path.exists(path):
            return path
        parts = [x for x in path.replace("\\", "/").split("/") if x]
        for n in range(1, min(5, len(parts)) + 1):
            cand = os.path.join(self.src_dir, *parts[-n:])
            if os.path.exists(cand):
                return cand
        return None

    def folders(self):
        root = self.p.root_folder
        order = []

        def visit(folder):
            for it in sorted((i for i in self.p.items.values() if type(i).__name__ == "FolderItem" and i.parent_folder is folder), key=lambda i: i.id):
                order.append(it)
                visit(it)

        visit(root)
        for f in order:
            v = self.var("F")
            self.item_var[f.id] = v
            self.emit("var %s = T(%s, function () { return app.project.items.addFolder(%s); });" % (v, js("folder " + f.name), js(f.name)))
            parent = self.item_var.get(f.parent_folder.id) if f.parent_folder is not root else None
            if parent:
                self.emit("if (%s && %s) T(%s, function () { %s.parentFolder = %s; });" % (v, parent, js("folder " + f.name), v, parent))

    def place(self, item, v):
        parent = item.parent_folder
        pv = self.item_var.get(parent.id) if parent is not None and parent is not self.p.root_folder else None
        if pv:
            self.emit("if (%s && %s) T(%s, function () { %s.parentFolder = %s; });" % (v, pv, js("move " + item.name), v, pv))

    def footage(self, used_ids):
        for it in sorted(self.p.footages, key=lambda i: i.id):
            if it.id not in used_ids:
                continue
            ms = it.main_source
            kind = type(ms).__name__
            if kind == "SolidSource":
                continue  # solids are created with their layers
            v = self.var("I")
            self.item_var[it.id] = v
            name = it.name or "Footage"
            w, h = int(getattr(it, "width", 0) or 0), int(getattr(it, "height", 0) or 0)
            fps = float(getattr(it, "frame_rate", 0) or 0) or 30.0
            dur = float(getattr(it, "duration", 0) or 0)
            path = getattr(ms, "file", None) if kind == "FileSource" else None
            found = self.resolve_file(path)
            if found:
                seq = not bool(getattr(ms, "is_still", True)) and dur > 0 and os.path.splitext(found)[1].lower() in (
                    ".png", ".jpg", ".jpeg", ".tif", ".tiff", ".exr", ".dpx", ".tga", ".gif", ".bmp", ".psd",
                )
                self.emit(
                    "var %s = T(%s, function () { var r = app.run(\"file.import\", {paths: [%s], sequence: %s}); return app.project.itemByID(r.items[0]); });"
                    % (v, js("import " + name), js(found), js(seq))
                )
                self.emit("if (%s && %s.name !== %s) T(%s, function () { %s.name = %s; });" % (v, v, js(name), js("rename " + name), v, js(name)))
                cfr = float(getattr(ms, "conform_frame_rate", 0) or 0)
                if cfr > 0:
                    self.step("frame rate of " + name, "if (%s) %s.mainSource.conformFrameRate = %s;" % (v, v, js(cfr)))
                self.stats["footage"] += 1
            else:
                if path:
                    self.notes.append("missing footage %r (%s): imported as a placeholder" % (name, path))
                    self.stats["missing"] += 1
                self.emit(
                    "var %s = T(%s, function () { return app.project.importPlaceholder(%s, %d, %d, %s, %s); });"
                    % (v, js("placeholder " + name), js(name), max(w, 1) if w else 1920, max(h, 1) if h else 1080, js(fps), js(dur or 10.0))
                )
            self.place(it, v)

    def comps(self):
        for c in sorted(self.p.compositions, key=lambda c: c.id):
            v = self.var("C")
            self.item_var[c.id] = v
            self.stats["comps"] += 1
            self.emit(
                "var %s = T(%s, function () { return app.project.items.addComp(%s, %d, %d, %s, %s, %s); });"
                % (v, js("comp " + c.name), js(c.name), int(c.width), int(c.height), js(float(c.pixel_aspect or 1)), js(float(c.duration or 1)), js(float(c.frame_rate or 30)))
            )
            sets = []
            for attr, key in (
                ("bg_color", "bgColor"),
                ("display_start_time", "displayStartTime"),
                ("work_area_start", "workAreaStart"),
                ("work_area_duration", "workAreaDuration"),
                ("motion_blur", "motionBlur"),
                ("shutter_angle", "shutterAngle"),
                ("shutter_phase", "shutterPhase"),
                ("frame_blending", "frameBlending"),
                ("hide_shy_layers", "hideShyLayers"),
                ("preserve_nested_frame_rate", "preserveNestedFrameRate"),
                ("preserve_nested_resolution", "preserveNestedResolution"),
            ):
                try:
                    val = getattr(c, attr)
                    if val is None:
                        continue
                    sets.append("T(%s, function () { %s.%s = %s; });" % (js("%s of %s" % (key, c.name)), v, key, js(val)))
                except Exception:
                    continue
            if sets:
                self.emit("if (%s) { %s }" % (v, " ".join(sets)))
            self.place(c, v)
            self.markers(c.marker_property if hasattr(c, "marker_property") else None, "%s.markerProperty" % v, v, "markers of " + c.name)

    # ---------------------------------------------------------------- layers
    def layers(self, c):
        cv = self.item_var[c.id]
        self.emit("if (%s) {" % cv)
        layers = list(c.layers)
        lv = {}
        # Bottom to top: every new layer lands at index 1, so the stack ends up in AE's order.
        for L in reversed(layers):
            v = self.var("L")
            lv[L.id] = v
            self.stats["layers"] += 1
            self.create_layer(c, cv, L, v)
        for L in layers:
            self.parenting(L, lv)
        for L in layers:
            v = lv[L.id]
            self.emit("if (%s) {" % v, 1)
            self.null_layer = bool(getattr(L, "null_layer", False))
            self.walk(L, v, "%s > %s" % (c.name, L.name), 2)
            self.emit("}", 1)
        for L in layers:
            if getattr(L, "locked", False):
                self.emit("if (%s) T(%s, function () { %s.locked = true; });" % (lv[L.id], js("lock " + L.name), lv[L.id]), 1)
        self.emit("}")

    def create_layer(self, c, cv, L, v):
        kind = type(L).__name__
        what = "layer %s > %s" % (c.name, L.name)
        name = js(L.name)
        center = js([c.width / 2.0, c.height / 2.0])
        if kind == "CameraLayer":
            make = "%s.layers.addCamera(%s, %s)" % (cv, name, center)
        elif kind == "LightLayer":
            make = "%s.layers.addLight(%s, %s)" % (cv, name, center)
        elif kind == "TextLayer":
            try:
                doc = L.property("ADBE Text Properties").property("ADBE Text Document")
                td = doc.keyframes[0].value if doc.keyframes else doc.value
                tdj = text_document_js(td)
                box = bool(getattr(td, "box_text", False))
                size = getattr(td, "box_text_size", None)
            except Exception:
                tdj, box, size = js(L.name), False, None
            if box and size:
                make = "%s.layers.addBoxText(%s, %s)" % (cv, js(list(size)), tdj)
            else:
                make = "%s.layers.addText(%s)" % (cv, tdj)
        elif kind == "ShapeLayer":
            make = "%s.layers.addShape()" % cv
        elif kind == "AVLayer":
            src = L.source
            src_kind = type(getattr(src, "main_source", None)).__name__ if src is not None else ""
            if src is not None and type(src).__name__ == "CompItem":
                make = "%s.layers.add(%s)" % (cv, self.item_var.get(src.id, "null"))
            elif src_kind == "SolidSource":
                if getattr(L, "null_layer", False):
                    make = "%s.layers.addNull()" % cv
                else:
                    color = list(src.main_source.color)[:3]
                    make = "%s.layers.addSolid(%s, %s, %d, %d, %s)" % (cv, js(color), js(src.name), int(src.width), int(src.height), js(float(src.pixel_aspect or 1)))
            elif src is not None and src.id in self.item_var:
                make = "%s.layers.add(%s)" % (cv, self.item_var[src.id])
            else:
                self.notes.append("%s: no source, created as a null" % what)
                make = "%s.layers.addNull()" % cv
        else:
            self.notes.append("%s: %s layers are not supported, created as a null" % (what, kind))
            make = "%s.layers.addNull()" % cv
        self.emit("var %s = T(%s, function () { return %s; });" % (v, js(what), make), 1)
        sets = ["%s.name = %s;" % (v, name)]

        def attr(py_attr, js_key, conv=js, default=None):
            try:
                val = getattr(L, py_attr)
            except Exception:
                return
            if val is None or (default is not None and val == default):
                return
            try:
                sets.append("T(%s, function () { %s.%s = %s; });" % (js("%s.%s" % (L.name, js_key)), v, js_key, conv(val)))
            except Unsupported:
                pass

        if kind == "LightLayer":
            attr("light_type", "lightType", lambda x: enum_js("LightType", x))
        attr("start_time", "startTime", default=0.0)
        attr("stretch", "stretch", default=100.0)
        attr("in_point", "inPoint")
        attr("out_point", "outPoint")
        attr("enabled", "enabled", default=True)
        attr("solo", "solo", default=False)
        attr("shy", "shy", default=False)
        attr("label", "label")
        attr("comment", "comment", default="")
        if kind in ("AVLayer", "TextLayer", "ShapeLayer"):
            attr("three_d_layer", "threeDLayer", default=False)
            attr("adjustment_layer", "adjustmentLayer", default=False)
            attr("guide_layer", "guideLayer", default=False)
            attr("blending_mode", "blendingMode", lambda x: enum_js("BlendingMode", x))
            attr("motion_blur", "motionBlur", default=False)
            attr("quality", "quality", lambda x: enum_js("LayerQuality", x))
            attr("collapse_transformation", "collapseTransformation", default=False)
            attr("preserve_transparency", "preserveTransparency", default=False)
            attr("frame_blending_type", "frameBlendingType", lambda x: enum_js("FrameBlendingType", x))
            attr("time_remap_enabled", "timeRemapEnabled", default=False)
            attr("audio_enabled", "audioEnabled", default=True)
        # Cameras and lights default to orienting towards their point of interest; layers to none.
        attr("auto_orient", "autoOrient", lambda x: enum_js("AutoOrientType", x), default=4214 if kind in ("CameraLayer", "LightLayer") else 4212)
        self.emit("if (%s) { %s }" % (v, " ".join(sets)), 1)

    def parenting(self, L, lv):
        v = lv[L.id]
        parent = getattr(L, "parent", None)
        if parent is not None and parent.id in lv:
            self.emit("if (%s && %s) T(%s, function () { %s.setParentWithJump(%s); });" % (v, lv[parent.id], js("parent of " + L.name), v, lv[parent.id]), 1)
        try:
            matte = L.track_matte_layer
            tm_type = L.track_matte_type
        except Exception:
            matte, tm_type = None, None
        if matte is not None and matte.id in lv and tm_type and int(tm_type) != 5012:
            # Setting a matte hides the matte layer; keep its visibility as saved.
            self.emit(
                "if (%s && %s) T(%s, function () { %s.setTrackMatte(%s, %s); %s.enabled = %s; });"
                % (
                    v,
                    lv[matte.id],
                    js("track matte of " + L.name),
                    v,
                    lv[matte.id],
                    enum_js("TrackMatteType", tm_type),
                    lv[matte.id],
                    js(bool(getattr(matte, "enabled", False))),
                ),
                1,
            )

    # ---------------------------------------------------------------- properties
    def walk(self, group, ref, ctx, depth):
        indexed = getattr(group, "property_type", None) == PropertyType.INDEXED_GROUP
        try:
            children = list(group.properties)
        except Exception:
            return
        for child in children:
            mn = child.match_name
            if mn in SKIP_GROUPS:
                continue
            ptype = getattr(child, "property_type", None)
            if mn in WARN_GROUPS:
                if ptype != PropertyType.PROPERTY and has_content(child):
                    self.notes.append("%s: %s are not imported" % (ctx, WARN_GROUPS[mn]))
                continue
            if mn == "ADBE Marker":
                self.markers(child, "Q(%s, %s)" % (ref, js(mn)), ref, "markers of " + ctx)
                continue
            if ptype == PropertyType.PROPERTY:
                self.leaf(child, "Q(%s, %s)" % (ref, js(mn)), ctx, depth)
            elif indexed:
                gv = self.var("G")
                label = "%s > %s" % (ctx, child.name or mn)
                if group.match_name == "ADBE Effect Parade":
                    self.stats["effects"] += 1
                self.emit(
                    "var %s = T(%s, function () { var g = %s; if (!g) { MISS(%s); return null; } return g.addProperty(%s); });"
                    % (gv, js("add " + label), ref, js(group.name or group.match_name), js(mn)),
                    depth,
                )
                self.emit("if (%s) {" % gv, depth)
                try:
                    if child.name and getattr(child, "is_name_set", False):
                        self.step("rename " + label, "%s.name = %s;" % (gv, js(child.name)), depth + 1)
                except Exception:
                    pass
                if getattr(child, "enabled", True) is False:
                    self.step("disable " + label, "%s.enabled = false;" % gv, depth + 1)
                if type(child).__name__ == "MaskPropertyGroup":
                    self.mask_attrs(child, gv, label, depth + 1)
                self.walk(child, gv, label, depth + 1)
                self.emit("}", depth)
            else:
                self.walk(child, "Q(%s, %s)" % (ref, js(mn)), ctx, depth)

    def mask_attrs(self, m, gv, label, depth):
        for py_attr, key, conv in (
            ("mask_mode", "maskMode", lambda x: enum_js("MaskMode", x)),
            ("inverted", "inverted", js),
            ("roto_bezier", "rotoBezier", js),
            ("color", "color", js),
            ("mask_motion_blur", "maskMotionBlur", lambda x: js(int(x))),
            ("mask_feather_falloff", "maskFeatherFalloff", lambda x: js(int(x))),
        ):
            try:
                val = getattr(m, py_attr)
                if val is None:
                    continue
                self.step("%s %s" % (label, key), "%s.%s = %s;" % (gv, key, conv(val)), depth)
            except Exception:
                continue

    def leaf(self, prop, ref, ctx, depth):
        try:
            vt = prop.property_value_type
        except Exception:
            return
        if vt in NO_VALUE_TYPES:
            return
        try:
            keys = list(prop.keyframes)
        except Exception:
            keys = []
        expr = getattr(prop, "expression", "") or ""
        # Defaults that differ between the two apps are written out even when unchanged: a null's
        # anchor point is its top-left corner in After Effects (it pivots its children), and its
        # opacity.
        forced = prop.match_name == "ADBE Anchor Point" or (self.null_layer and prop.match_name == "ADBE Opacity")
        if not keys and not expr and not forced and not getattr(prop, "is_modified", False):
            return
        label = "%s > %s" % (ctx, prop.name or prop.match_name)
        if getattr(prop, "is_separation_leader", False) and getattr(prop, "dimensions_separated", False):
            self.step(label + " (separate dimensions)", "%s.dimensionsSeparated = true;" % ref, depth)
            return
        if getattr(prop, "is_separation_follower", False):
            leader = getattr(prop, "separation_leader", None)
            if leader is None or not getattr(leader, "dimensions_separated", False):
                return  # X / Y / Z Position only count while the dimensions are separated
        body = ["var p = %s; if (!p) { MISS(%s); return; }" % (ref, js(prop.name or prop.match_name))]
        try:
            if keys:
                times = [float(k.time) for k in keys]
                vals = [value_js(prop, k.value) for k in keys]
                body.append("p.setValuesAtTimes(%s, [%s]);" % (js(times), ", ".join(vals)))
                self.stats["keyframes"] += len(keys)
                # Per key: interpolation types, then the continuity flags, then the exact eases and
                # tangents the .aep stores. The auto-Bezier and roving flags are left off when exact
                # values are there: they'd make EffectCraft recompute the curve (and roving the key
                # times) with its own smoothing instead of keeping After Effects' result.
                for i, k in enumerate(keys, 1):
                    kin, kout = int(k.in_interpolation_type), int(k.out_interpolation_type)
                    if (kin, kout) != (6612, 6612):
                        body.append(
                            "p.setInterpolationTypeAtKey(%d, %s, %s);"
                            % (i, enum_js("KeyframeInterpolationType", kin), enum_js("KeyframeInterpolationType", kout))
                        )
                    for flag, fn in (("temporal_continuous", "setTemporalContinuousAtKey"), ("spatial_continuous", "setSpatialContinuousAtKey")):
                        if getattr(k, flag, False):
                            body.append("T('', function () { p.%s(%d, true); });" % (fn, i))
                    has_ease = 6613 in (kin, kout) and bool(k.in_temporal_ease) and bool(k.out_temporal_ease)
                    if has_ease:
                        ein = [[float(e.speed), float(e.influence)] for e in k.in_temporal_ease]
                        eout = [[float(e.speed), float(e.influence)] for e in k.out_temporal_ease]
                        body.append("p.setTemporalEaseAtKey(%d, KE(%s), KE(%s));" % (i, js(ein), js(eout)))
                    elif getattr(k, "temporal_auto_bezier", False):
                        body.append("T('', function () { p.setTemporalAutoBezierAtKey(%d, true); });" % i)
                    has_tangents = False
                    if getattr(prop, "is_spatial", False) and k.in_spatial_tangent is not None:
                        tin, tout = list(k.in_spatial_tangent), list(k.out_spatial_tangent or [])
                        has_tangents = True
                        body.append("T('', function () { p.setSpatialTangentsAtKey(%d, %s, %s); });" % (i, js(tin), js(tout)))
                    if not has_tangents and getattr(k, "spatial_auto_bezier", False):
                        body.append("T('', function () { p.setSpatialAutoBezierAtKey(%d, true); });" % i)
            elif forced or getattr(prop, "is_modified", False):
                body.append("p.setValue(%s);" % value_js(prop, prop.value))
        except Unsupported as e:
            self.notes.append("%s: %s is not imported" % (label, e))
            body = body[:1]
        if expr:
            body.append("p.expression = %s;" % js(expr))
            if not getattr(prop, "expression_enabled", True):
                body.append("p.expressionEnabled = false;")
        if len(body) > 1:
            self.step(label, " ".join(body), depth)

    def markers(self, prop, ref, owner, label):
        if prop is None:
            return
        try:
            keys = list(prop.keyframes)
        except Exception:
            return
        if not keys:
            return
        body = []
        for k in keys:
            try:
                body.append("m.setValueAtTime(%s, %s);" % (js(float(k.time)), value_js(prop, k.value)))
            except Exception:
                continue
        if body:
            self.emit("if (%s) T(%s, function () { var m = %s; %s });" % (owner, js(label), ref, " ".join(body)))

    # ---------------------------------------------------------------- driver
    def used_items(self):
        used = set()
        for c in self.p.compositions:
            for L in c.layers:
                src = getattr(L, "source", None)
                if src is not None:
                    used.add(src.id)
        for it in self.p.footages:
            if type(it.main_source).__name__ in ("FileSource", "PlaceholderSource"):
                used.add(it.id)  # imported footage stays in the project even when unused
        return used

    def main_comp(self):
        """The comp to open after the import: one no other comp nests, with the most layers (then
        the longest)."""
        comps = list(self.p.compositions)
        nested = set()
        for c in comps:
            for L in c.layers:
                src = getattr(L, "source", None)
                if src is not None and type(src).__name__ == "CompItem":
                    nested.add(src.id)
        top = [c for c in comps if c.id not in nested] or comps
        return max(top, key=lambda c: (c.num_layers, float(c.duration or 0)), default=None)

    def run(self, title):
        self.emit("// Generated by aep2effectcraft.py from %s: rebuilds the After Effects project in EffectCraft." % title)
        self.emit("var __warnings = [], __missing = {};")
        self.emit("function MISS(name) { __missing[name] = (__missing[name] || 0) + 1; }")
        self.emit("function Q(o, n) { return o ? o.property(n) : null; }")
        self.emit("function T(what, f) { try { return f(); } catch (e) { if (what) __warnings.push(what + \": \" + (e && e.message ? e.message : e)); return null; } }")
        self.emit("function SH(v, i, o, c) { var s = new Shape(); s.vertices = v; s.inTangents = i; s.outTangents = o; s.closed = c; return s; }")
        self.emit("function KE(a) { var r = []; for (var i = 0; i < a.length; i++) r.push(new KeyframeEase(a[i][0], Math.max(0.1, Math.min(100, a[i][1])))); return r; }")
        self.emit("function TD(o) { var t = new TextDocument(o.text); for (var k in o) { if (k !== \"text\" && o[k] !== undefined) { try { t[k] = o[k]; } catch (e) {} } } return t; }")
        self.emit("function MV(o) { var m = new MarkerValue(o.comment, o.chapter, o.url, o.frameTarget, o.cuePointName); try { m.duration = o.duration; } catch (e) {} try { m.label = o.label; } catch (e) {} return m; }")
        self.emit("app.beginUndoGroup(%s);" % js("Import " + title))
        self.folders()
        self.footage(self.used_items())
        self.comps()
        for c in sorted(self.p.compositions, key=lambda c: c.id):
            self.layers(c)
        s = self.stats
        summary = "Imported %s: %d comps, %d layers, %d footage items (%d missing), %d effects, %d keyframes." % (
            title, s["comps"], s["layers"], s["footage"], s["missing"], s["effects"], s["keyframes"])
        main = self.main_comp()
        if main is not None and main.id in self.item_var:
            mv = self.item_var[main.id]
            self.emit("if (%s) T(%s, function () { %s.openInViewer(); });" % (mv, js("open " + main.name), mv))
            summary += " Opened %s." % main.name
        self.emit("app.endUndoGroup();")
        self.emit("writeLn(%s);" % js(summary))
        notes = js(self.notes)
        self.emit("var __notes = %s;" % notes)
        self.emit("var __miss = []; for (var k in __missing) __miss.push(k + (__missing[k] > 1 ? \" (x\" + __missing[k] + \")\" : \"\"));")
        self.emit("if (__miss.length) __notes.push(\"properties EffectCraft doesn't have: \" + __miss.join(\", \"));")
        self.emit("var __all = __notes.concat(__warnings);")
        self.emit("if (__all.length) { writeLn(__all.length + \" things could not be imported exactly:\"); for (var i = 0; i < __all.length && i < 200; i++) writeLn(\"- \" + __all[i]); }")
        return "\n".join(self.out) + "\n"


def load(path):
    if path.lower().endswith(".aepx"):
        data = aepx.aepx_to_aep_bytes(path)
        fd, tmp = tempfile.mkstemp(suffix=".aep")
        try:
            with os.fdopen(fd, "wb") as f:
                f.write(data)
            return py_aep.parse(tmp)
        finally:
            try:
                os.remove(tmp)
            except OSError:
                pass
    return py_aep.parse(path)


def main(argv=None):
    ap = argparse.ArgumentParser(description="Convert an After Effects project (.aep / .aepx) into an EffectCraft import script.")
    ap.add_argument("project", help=".aep or .aepx file")
    ap.add_argument("-o", "--output", help="the .jsx to write (default: standard output)")
    a = ap.parse_args(argv)
    if not os.path.isfile(a.project):
        sys.stderr.write("%s: no such file\n" % a.project)
        return 2
    try:
        app = load(a.project)
    except Exception as e:
        sys.stderr.write("%s: could not read the project: %s\n" % (a.project, e))
        return 2
    jsx = Converter(app.project, a.project).run(os.path.basename(a.project))
    if a.output:
        with open(a.output, "w", encoding="utf-8", newline="\n") as f:
            f.write(jsx)
    else:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stdout.write(jsx)
    return 0


if __name__ == "__main__":
    sys.exit(main())
