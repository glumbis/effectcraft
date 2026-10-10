"""Repack an After Effects XML project (.aepx) into the binary RIFX layout of an .aep.

An .aepx is the same chunk tree as an .aep written as XML: every element is one chunk named by its
four-character tag (shorter tags are padded with spaces: `Pin` is `Pin `), `bdata` holds a leaf
chunk's bytes in hex, and an element with children is a `LIST` whose list type is the tag. A few
elements are spelled differently:

* `<string>` is a `Utf8` chunk (its text, UTF-8).
* `<fileReference .../>` (inside `Als2`) is the `alas` chunk: the path record as compact JSON with
  sorted keys.
* `tdsn`, `pdnm`, `fnam`, `vfdn` and `RCom` wrap their children without a list type.
* `btdk` keeps raw bytes after its list type (`<btdk bdata=...>` becomes `LIST btdk` + the bytes).
* `<ProjectXMPMetadata>` is the XMP packet that follows the RIFX chunk.

The binary layout follows py-aep's file format notes (docs/file_format.md,
https://github.com/forticheprod/py-aep, MIT): big-endian `RIFX` / `Egg!`, chunk = tag + u32 length +
body + a pad byte when the length is odd.
"""

import json
import struct
import xml.etree.ElementTree as ET

WRAPPERS = {"tdsn", "pdnm", "fnam", "vfdn", "RCom"}


def _local(tag):
    return tag.rsplit("}", 1)[-1]


def _fourcc(name):
    if len(name) > 4:
        raise ValueError("not a chunk tag: %r" % name)
    return name.ljust(4).encode("latin-1")


def _chunk(tag, body):
    out = _fourcc(tag) + struct.pack(">I", len(body)) + body
    if len(body) % 2:
        out += b"\0"
    return out


def _alas(el):
    a = el.attrib
    path = a.get("fullpath", "")

    def num(k):
        try:
            return int(a.get(k, "0"))
        except ValueError:
            return 0

    platform = a.get("platform", "")
    if platform.isdigit():
        plat = int(platform)
    else:
        # The XML names the platform; the binary record encodes the path style (1 = drive letter or
        # UNC, 2 = POSIX).
        plat = 2 if path.startswith("/") else 1
    rec = {
        "ascendcount_base": num("ascendcount_base"),
        "ascendcount_target": num("ascendcount_target"),
        "fullpath": path,
        "platform": plat,
        "server_name": a.get("server_name", ""),
        "server_volume_name": a.get("server_volume_name", ""),
        "target_is_folder": a.get("target_is_folder", "0") not in ("0", "false", ""),
    }
    return _chunk("alas", json.dumps(rec, separators=(",", ":"), sort_keys=True).encode("utf-8"))


def _children(el):
    return b"".join(_encode(c) for c in el if _local(c.tag) != "ProjectXMPMetadata")


def _encode(el):
    tag = _local(el.tag)
    if tag == "string":
        return _chunk("Utf8", (el.text or "").encode("utf-8"))
    if tag == "fileReference":
        return _alas(el)
    hexdata = el.attrib.get("bdata")
    if tag == "btdk" and hexdata is not None:
        return _chunk("LIST", b"btdk" + bytes.fromhex(hexdata))
    if hexdata is not None and len(el) == 0:
        return _chunk(tag, bytes.fromhex(hexdata))
    if tag in WRAPPERS:
        return _chunk(tag, _children(el))
    body = _children(el)
    if tag == "CIFO":
        body = _cps2_from_caps(el) + body
    return _chunk("LIST", _fourcc(tag) + body)


def _cps2_from_caps(cifo):
    """Older After Effects versions write a comp's essential-graphics captions only as
    `LIST:CapS` (CsCt, CapL, a name); newer ones also write `LIST:CpS2` (CsCt, the name, a locale),
    which py-aep needs. Synthesise the newer list from the older one when it's missing."""
    names = [_local(c.tag) for c in cifo]
    if "CpS2" in names or "CapS" not in names:
        return b""
    caps = next(c for c in cifo if _local(c.tag) == "CapS")
    csct = next((c for c in caps if _local(c.tag) == "CsCt"), None)
    title = next((c for c in caps if _local(c.tag) == "string"), None)
    body = b"CpS2"
    body += _chunk("CsCt", bytes.fromhex(csct.attrib.get("bdata", "01000000")) if csct is not None else b"\x01\0\0\0")
    body += _chunk("Utf8", ((title.text or "") if title is not None else "").encode("utf-8"))
    body += _chunk("Utf8", b"en_US")
    return _chunk("LIST", body)


def aepx_to_aep_bytes(path):
    """The .aep bytes of the .aepx at `path`."""
    root = ET.parse(path).getroot()
    if _local(root.tag) != "AfterEffectsProject":
        raise ValueError("%s: not an After Effects XML project (root element %s)" % (path, _local(root.tag)))
    body = b"Egg!" + _children(root)
    out = b"RIFX" + struct.pack(">I", len(body)) + body
    xmp = next((c for c in root if _local(c.tag) == "ProjectXMPMetadata"), None)
    if xmp is not None and xmp.text:
        out += xmp.text.encode("utf-8")
    return out


if __name__ == "__main__":
    import sys

    if len(sys.argv) != 3:
        sys.exit("usage: aepx.py IN.aepx OUT.aep")
    with open(sys.argv[2], "wb") as f:
        f.write(aepx_to_aep_bytes(sys.argv[1]))
