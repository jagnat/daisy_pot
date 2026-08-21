#!/usr/bin/env python3
"""Convert bitmap fonts into the project's packed Rust ``Font`` format.

Supported inputs:

* OpenType bitmap fonts (``.otb``) with an EBDT/EBLC strike.
* Playdate fonts (``.fnt``) containing an embedded PNG atlas.

The output contains a contiguous codepoint range, defaults to printable ASCII,
and is packed glyph-major then row-major. Each row occupies
``ceil(width / 8)`` bytes. Bit 0 is the leftmost pixel and a set bit means
glyph ink (black), matching the mask convention used by the Sharp display
renderer.
"""

import argparse
import base64
import io
import json
import os
import re
import sys


def glyph_bitmap(glyph, width):
    """Unpack one EBDT glyph into rows of 0/1 pixels."""
    from fontTools.ttLib.tables.E_B_D_T_ import BitAlignedBitmapMixin, ByteAlignedBitmapMixin

    data = glyph.imageData
    if isinstance(glyph, ByteAlignedBitmapMixin):
        stride = (width + 7) // 8
        height = len(data) // stride
        return [
            [(data[y * stride + (x >> 3)] >> (7 - (x & 7))) & 1 for x in range(width)]
            for y in range(height)
        ]

    if not isinstance(glyph, BitAlignedBitmapMixin):
        raise SystemExit(f"unsupported EBDT glyph format: {type(glyph).__name__}")

    height = (len(data) * 8) // width
    return [
        [
            (data[(y * width + x) >> 3] >> (7 - ((y * width + x) & 7))) & 1
            for x in range(width)
        ]
        for y in range(height)
    ]


def load_otb(path, first, last):
    try:
        from fontTools.ttLib import TTFont
    except ImportError as error:
        raise SystemExit("OTB conversion requires fonttools: pip install fonttools") from error

    font = TTFont(path, lazy=False)
    if "EBDT" not in font or "EBLC" not in font:
        raise SystemExit(f"{path}: no EBDT/EBLC bitmap strike")

    strike = font["EBDT"].strikeData[0]
    size = font["EBLC"].strikes[0].bitmapSizeTable
    cmap = font.getBestCmap()
    width = size.hori.widthMax
    glyphs = {}
    height = None

    for cp in range(first, last + 1):
        name = cmap.get(cp)
        if name is None or name not in strike:
            continue
        bitmap = glyph_bitmap(strike[name], width)
        if height is None:
            height = len(bitmap)
        elif len(bitmap) != height:
            raise SystemExit(
                f"{path}: glyph U+{cp:04X} is {len(bitmap)} rows, expected {height}; "
                "this converter requires a fixed bitmap cell"
            )
        glyphs[cp] = bitmap

    if height is None:
        raise SystemExit(f"{path}: no glyphs in range {first:#x}..{last:#x}")

    blank = [[0] * width for _ in range(height)]
    bitmaps = [glyphs.get(cp, blank) for cp in range(first, last + 1)]
    missing = [cp for cp in range(first, last + 1) if cp not in glyphs]
    baseline = int(size.hori.maxBeforeBL)
    if not 0 <= baseline <= height:
        baseline = height

    return {
        "kind": "OTB",
        "notice": (
            "Oldschool PC Font Pack v2.2 (VileR, int10h.org), CC BY-SA 4.0."
            if "oldschool_pc_font_pack" in path
            else None
        ),
        "width": width,
        "height": height,
        "baseline": baseline,
        "tracking": 0,
        "bitmaps": bitmaps,
        "advances": width,
        "missing": missing,
    }


def property_value(path, lines, name):
    prefix = name + "="
    for line in lines:
        if line.startswith(prefix):
            return line[len(prefix):]
    raise SystemExit(f"{path}: missing {name}= property")


def load_playdate(path, first, last):
    try:
        from PIL import Image
    except ImportError as error:
        raise SystemExit("Playdate conversion requires Pillow: pip install Pillow") from error

    with open(path, encoding="utf-8") as source:
        lines = source.read().splitlines()

    metrics_line = next((line for line in lines if line.startswith("--metrics=")), None)
    metrics = json.loads(metrics_line[len("--metrics="):]) if metrics_line else {}
    width = int(property_value(path, lines, "width"))
    height = int(property_value(path, lines, "height"))
    tracking = int(property_value(path, lines, "tracking"))
    image_data = base64.b64decode(property_value(path, lines, "data"))
    atlas = Image.open(io.BytesIO(image_data)).convert("RGBA")

    tracking_index = next(i for i, line in enumerate(lines) if line.startswith("tracking="))
    entries = []
    started = False
    for line in lines[tracking_index + 1:]:
        if not line.strip():
            if started:
                break
            continue
        started = True
        name, advance = line.rsplit(None, 1)
        char = " " if name == "space" else name
        if len(char) != 1:
            raise SystemExit(f"{path}: unsupported glyph name {name!r}")
        entries.append((ord(char), int(advance)))

    if atlas.width % width or atlas.height % height:
        raise SystemExit(
            f"{path}: {atlas.width}x{atlas.height} atlas is not divisible by "
            f"{width}x{height} cells"
        )
    capacity = (atlas.width // width) * (atlas.height // height)
    if len(entries) > capacity:
        raise SystemExit(f"{path}: {len(entries)} glyphs but atlas holds {capacity}")

    glyphs = {}
    columns = atlas.width // width
    for index, (cp, advance) in enumerate(entries):
        x0 = (index % columns) * width
        y0 = (index // columns) * height
        bitmap = [
            [1 if atlas.getpixel((x0 + x, y0 + y))[3] >= 128 else 0 for x in range(width)]
            for y in range(height)
        ]
        glyphs[cp] = (advance, bitmap)

    blank = [[0] * width for _ in range(height)]
    fallback_advance = glyphs.get(ord(" "), (width, blank))[0]
    bitmaps = []
    advances = []
    missing = []
    for cp in range(first, last + 1):
        if cp in glyphs:
            advance, bitmap = glyphs[cp]
        else:
            advance, bitmap = fallback_advance, blank
            missing.append(cp)
        advances.append(advance)
        bitmaps.append(bitmap)

    return {
        "kind": "Playdate",
        "notice": None,
        "width": width,
        "height": height,
        "baseline": int(metrics.get("baseline", height - 1)),
        "tracking": tracking,
        "bitmaps": bitmaps,
        "advances": advances,
        "missing": missing,
    }


def dedouble(bitmaps, height):
    """Collapse a font whose every row is stored twice."""
    if height % 2:
        raise SystemExit(f"cannot de-double an odd cell height ({height})")
    for index, glyph in enumerate(bitmaps):
        for y in range(0, height, 2):
            if glyph[y] != glyph[y + 1]:
                raise SystemExit(
                    f"glyph index {index} differs between rows {y} and {y + 1}; "
                    "rerun without --dedouble"
                )
    return [glyph[::2] for glyph in bitmaps], height // 2


def trim(bitmaps, height):
    """Return the shared nonblank vertical range across all glyphs."""
    top = 0
    while top < height and all(not any(glyph[top]) for glyph in bitmaps):
        top += 1
    if top == height:
        return 0, height
    bottom = height
    while bottom > top and all(not any(glyph[bottom - 1]) for glyph in bitmaps):
        bottom -= 1
    return top, bottom


def pack(bitmaps, width, top, bottom):
    stride = (width + 7) // 8
    packed = bytearray()
    for glyph in bitmaps:
        for row in glyph[top:bottom]:
            output = bytearray(stride)
            for x, ink in enumerate(row):
                if ink:
                    output[x >> 3] |= 1 << (x & 7)
            packed.extend(output)
    return packed


def preview(bitmaps, first, chars):
    for char in chars:
        index = ord(char) - first
        if not 0 <= index < len(bitmaps):
            continue
        print(f"--- {char!r}", file=sys.stderr)
        for row in bitmaps[index]:
            print("".join("#" if pixel else "." for pixel in row), file=sys.stderr)


def ident(path):
    stem = os.path.splitext(os.path.basename(path))[0]
    stem = re.sub(r"^Bm\d*_", "", stem)
    return re.sub(r"[^A-Za-z0-9]+", "_", stem).strip("_").upper()


def char_label(cp):
    if cp == 0x20:
        return "space"
    if cp == 0x5C:
        return "backslash"
    if 0x21 <= cp <= 0x7E:
        return chr(cp)
    return f"U+{cp:04X}"


def check_u8(label, value):
    if not 0 <= value <= 0xFF:
        raise SystemExit(f"{label}={value} does not fit in u8")


def check_u16(label, value):
    if not 0 <= value <= 0xFFFF:
        raise SystemExit(f"{label}={value} does not fit in u16")


def emit(out, source, name, font, first, count, rows, top, row_repeat, packed):
    width = font["width"]
    stride = (width + 7) // 8
    advances = font["advances"]
    tracking = font["tracking"]

    for label, value in (
        ("width", width),
        ("rows", rows),
        ("top", top * row_repeat),
        ("line_height", font["height"]),
        ("baseline", font["baseline"]),
        ("row_repeat", row_repeat),
    ):
        check_u8(label, value)
    if not 0 <= first <= 0x10FFFF:
        raise SystemExit(f"first={first} is not a Unicode codepoint")
    check_u16("count", count)
    if not -128 <= tracking <= 127:
        raise SystemExit(f"tracking={tracking} does not fit in i8")

    print(f"// Generated by fontconv.py from {os.path.basename(source)} -- do not edit.", file=out)
    if font["notice"]:
        print(f"// {font['notice']}", file=out)
    print("// Glyph-major, then row-major. Bit 0 is the leftmost pixel; set bits are ink.", file=out)
    print(file=out)
    print("use crate::font::{Font, FontAdvances};", file=out)
    print(file=out)

    if isinstance(advances, list):
        if any(not 0 <= advance <= 0xFF for advance in advances):
            raise SystemExit("one or more glyph advances do not fit in u8")
        print(f"static {name}_ADVANCES: [u8; {count}] = [", file=out)
        for start in range(0, count, 16):
            chunk = advances[start:start + 16]
            print("    " + " ".join(f"{advance}," for advance in chunk), file=out)
        print("];", file=out)
        print(file=out)
        advance_value = f"FontAdvances::Variable(&{name}_ADVANCES)"
    else:
        check_u8("advance", advances)
        advance_value = f"FontAdvances::Fixed({advances})"

    print(f"pub static {name}: Font = Font {{", file=out)
    print(f"    width: {width},", file=out)
    print(f"    rows: {rows},", file=out)
    print(f"    top: {top * row_repeat},", file=out)
    print(f"    line_height: {font['height']},", file=out)
    print(f"    baseline: {font['baseline']},", file=out)
    print(f"    row_repeat: {row_repeat},", file=out)
    print(f"    tracking: {tracking},", file=out)
    print(f"    first: {first:#04x},", file=out)
    print(f"    count: {count},", file=out)
    print(f"    advances: {advance_value},", file=out)
    print(f"    data: &{name}_DATA,", file=out)
    print("};", file=out)
    print(file=out)

    print(f"static {name}_DATA: [u8; {len(packed)}] = [", file=out)
    glyph_bytes = stride * rows
    for index in range(count):
        print(f"    // {char_label(first + index)}", file=out)
        glyph = packed[index * glyph_bytes:(index + 1) * glyph_bytes]
        for y in range(rows):
            row = glyph[y * stride:(y + 1) * stride]
            print("    " + " ".join(f"0x{byte:02x}," for byte in row), file=out)
    print("];", file=out)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("font", help="input .otb or Playdate .fnt file")
    parser.add_argument("-o", "--output", help="write Rust here instead of stdout")
    parser.add_argument("-n", "--name", help="Rust static name (default: derived from filename)")
    parser.add_argument("--first", type=lambda value: int(value, 0), default=0x20)
    parser.add_argument("--last", type=lambda value: int(value, 0), default=0x7E)
    parser.add_argument("--no-trim", action="store_true", help="retain shared blank top/bottom rows")
    parser.add_argument("--dedouble", action="store_true", help="store identical row pairs once")
    parser.add_argument("--tracking", type=int, help="override the source tracking value")
    parser.add_argument("--preview", metavar="CHARS", help="print selected glyphs as ASCII art")
    args = parser.parse_args()

    if not 0 <= args.first <= args.last <= 0x10FFFF:
        parser.error("first and last must form a valid Unicode codepoint range")
    if args.last - args.first + 1 > 0xFFFF:
        parser.error("the selected range contains more than 65535 glyphs")

    extension = os.path.splitext(args.font)[1].lower()
    if extension == ".otb":
        font = load_otb(args.font, args.first, args.last)
    elif extension == ".fnt":
        font = load_playdate(args.font, args.first, args.last)
    else:
        parser.error("input must have a .otb or .fnt extension")

    if args.tracking is not None:
        font["tracking"] = args.tracking

    bitmaps = font["bitmaps"]
    logical_height = font["height"]
    row_repeat = 1
    if args.dedouble:
        bitmaps, logical_height = dedouble(bitmaps, logical_height)
        row_repeat = 2

    if args.preview:
        preview(bitmaps, args.first, args.preview)

    top, bottom = (0, logical_height) if args.no_trim else trim(bitmaps, logical_height)
    rows = bottom - top
    packed = pack(bitmaps, font["width"], top, bottom)
    count = args.last - args.first + 1
    name = args.name or ident(args.font)

    destination = open(args.output, "w") if args.output else sys.stdout
    try:
        emit(destination, args.font, name, font, args.first, count, rows, top, row_repeat, packed)
    finally:
        if args.output:
            destination.close()

    if font["missing"]:
        missing = ", ".join(f"U+{cp:04X}" for cp in font["missing"])
        print(f"warning: blank glyphs emitted for {missing}", file=sys.stderr)
    advance_kind = "proportional" if isinstance(font["advances"], list) else "fixed"
    print(
        f"{os.path.basename(args.font)}: {font['kind']} {advance_kind}, "
        f"{font['width']}x{font['height']} cell -> {font['width']}x{rows} stored "
        f"at top {top * row_repeat}, repeat {row_repeat}, {count} glyphs, {len(packed)} bytes",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
