"""Writes `demo-quad.png`, the placeholder image of the `render.quad` demonstration, next to this script.

    python3 generate_demo_quad.py            write the image
    python3 generate_demo_quad.py --check    fail if the committed image differs from what this script writes

A 32 x 32 gem in pixel art on a transparent background: fully transparent corners, a half-transparent halo, and
opaque facets. Nearest sampling keeps the pixels square when the demonstration scales it up eight times, and the
transparent and half-transparent areas show the blending over the clear colour. The owner's own image replaces
it later, as a file change.

The output is deterministic: the image data is deflated with *stored* blocks, so no compressor (and no zlib
version) is involved, and every chunk is written by hand.
"""

import struct
import sys
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
NAME = "demo-quad.png"
SIZE = 32
SIGNATURE = b"\x89PNG\r\n\x1a\n"
STORED_BLOCK = 65535

Pixel = tuple[int, int, int, int]
CLEAR: Pixel = (0, 0, 0, 0)
HALO: Pixel = (255, 255, 255, 96)
# The facets of the gem, from the top-left light to the bottom-right shadow.
LIGHT: Pixel = (170, 235, 255, 255)
MID: Pixel = (70, 170, 235, 255)
DARK: Pixel = (30, 90, 175, 255)
EDGE: Pixel = (10, 40, 100, 255)
# The gem's size, as a distance from the centre in the diamond metric (|dx| + |dy|): its edge starts at
# `EDGE_FROM`, the body ends at `BODY_TO`, and the halo ends at `HALO_TO`.
EDGE_FROM = 9.5
BODY_TO = 10.5
HALO_TO = 13.5


def pixel(x: int, y: int) -> Pixel:
    """The colour at (x, y): a diamond with an edge, three facets, and a halo around it."""
    centre = (SIZE - 1) / 2
    distance = abs(x - centre) + abs(y - centre)
    if distance <= BODY_TO:
        if distance >= EDGE_FROM:
            return EDGE
        if y < centre - 2:
            return LIGHT if x < centre else MID
        return MID if x < centre else DARK
    if distance <= HALO_TO:
        return HALO
    return CLEAR


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


def image() -> bytes:
    """The whole PNG file."""
    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)  # filter type: none
        for x in range(SIZE):
            raw += bytes(pixel(x, y))
    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        SIGNATURE
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", stored(bytes(raw)))
        + chunk(b"IEND", b"")
    )


def main(arguments: list[str]) -> int:
    target = HERE / NAME
    data = image()
    if arguments == ["--check"]:
        if not target.exists() or target.read_bytes() != data:
            print(f"{target} differs from what {Path(__file__).name} writes", file=sys.stderr)
            return 1
        return 0
    if arguments:
        print(f"usage: {Path(__file__).name} [--check]", file=sys.stderr)
        return 2
    target.write_bytes(data)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
