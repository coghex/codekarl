"""Writes the glyph PNGs and `panel.png` of the pixel font from `font.toml`, next to this script.

    python3 make_font.py            write the images
    python3 make_font.py --check    fail if a committed image differs from what this script writes, or is missing or extra

`font.toml` is the source: each glyph's `rows` ('#' set, '.' clear). A scene quad is one whole texture, so every glyph gets its
own PNG, `glyphs/U+XXXX.png` (the code point in at least four upper-case hex digits) and `glyphs/replacement.png`. A set pixel is
the text colour `#ece3cb` at full alpha and a clear pixel is transparent. `panel.png` is one opaque pixel of `#2a2119`, the
background of the date readout and of the hover tooltip.

The output is deterministic: the image data is deflated with *stored* blocks, so no compressor (and no zlib version) is
involved, and every chunk is written by hand.
"""

import struct
import sys
import tomllib
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
GLYPHS = HERE / "glyphs"
SIGNATURE = b"\x89PNG\r\n\x1a\n"
STORED_BLOCK = 65535

Pixel = tuple[int, int, int, int]
TEXT: Pixel = (0xEC, 0xE3, 0xCB, 255)
CLEAR: Pixel = (0, 0, 0, 0)
PANEL: Pixel = (0x2A, 0x21, 0x19, 255)


def chunk(kind: bytes, data: bytes) -> bytes:
    """One PNG chunk: length, type, data and CRC."""
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))


def stored(raw: bytes) -> bytes:
    """`raw` as a zlib stream of stored (uncompressed) deflate blocks."""
    out = bytearray(b"\x78\x01")
    blocks = [raw[i : i + STORED_BLOCK] for i in range(0, len(raw), STORED_BLOCK)] or [b""]
    for index, block in enumerate(blocks):
        final = 1 if index == len(blocks) - 1 else 0
        out += struct.pack("<BHH", final, len(block), len(block) ^ 0xFFFF) + block
    out += struct.pack(">I", zlib.adler32(raw))
    return bytes(out)


def png(rows: list[list[Pixel]]) -> bytes:
    """A whole RGBA8 PNG file of `rows` of pixels (top row first)."""
    raw = bytearray()
    for row in rows:
        raw.append(0)  # filter type: none
        for pixel in row:
            raw += bytes(pixel)
    header = struct.pack(">IIBBBBB", len(rows[0]), len(rows), 8, 6, 0, 0, 0)
    return (
        SIGNATURE
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", stored(bytes(raw)))
        + chunk(b"IEND", b"")
    )


def glyph_png(rows: list[str]) -> bytes:
    """The PNG of one glyph: '#' is the text colour, '.' is transparent."""
    return png([[TEXT if cell == "#" else CLEAR for cell in row] for row in rows])


def images() -> dict[Path, bytes]:
    """Every image the font has, by path: one per glyph, the replacement and the panel."""
    with (HERE / "font.toml").open("rb") as source:
        font = tomllib.load(source)
    found: dict[Path, bytes] = {
        GLYPHS / "replacement.png": glyph_png(font["replacement"]["rows"]),
        HERE / "panel.png": png([[PANEL]]),
    }
    for character, glyph in font["glyph"].items():
        found[GLYPHS / f"U+{ord(character):04X}.png"] = glyph_png(glyph["rows"])
    return found


def main(arguments: list[str]) -> int:
    wanted = images()
    if arguments == ["--check"]:
        problems = [
            f"{path} differs from what {Path(__file__).name} writes, or is missing"
            for path, data in sorted(wanted.items())
            if not path.exists() or path.read_bytes() != data
        ]
        problems += [
            f"{path} is not a glyph of font.toml"
            for path in sorted(GLYPHS.glob("*.png"))
            if path not in wanted
        ]
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1 if problems else 0
    if arguments:
        print(f"usage: {Path(__file__).name} [--check]", file=sys.stderr)
        return 2
    GLYPHS.mkdir(exist_ok=True)
    for path, data in wanted.items():
        path.write_bytes(data)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
