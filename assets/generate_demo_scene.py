"""Writes the twelve tile images of the live-scene demonstration into `demo-scene/`, next to this script.

    python3 generate_demo_scene.py            write the images
    python3 generate_demo_scene.py --check    fail if a committed image differs from what this script writes

Twelve 32 x 32 RGBA images, each a flat colour with a different simple pattern, so they are told apart at a glance in the
window; the outermost ring of pixels is half transparent, so the blending over the clear colour shows. `demo-quad.png`
(the gem, written by `generate_demo_quad.py`) is the moved quad's image; these are the tiles that appear around it.

The output is deterministic, written like `generate_demo_quad.py`: the image data is deflated with *stored* blocks, so no
compressor (and no zlib version) is involved, and every chunk is written by hand.
"""

import struct
import sys
import zlib
from collections.abc import Callable
from pathlib import Path

HERE = Path(__file__).resolve().parent
DIRECTORY = HERE / "demo-scene"
SIZE = 32
SIGNATURE = b"\x89PNG\r\n\x1a\n"
STORED_BLOCK = 65535

Pixel = tuple[int, int, int, int]
Pattern = Callable[[int, int], bool]
HALF = 128

CENTRE = (SIZE - 1) / 2
HALF_SIDE = SIZE // 2
CELL = 4
LARGE_CELL = 8
DOT_ON = (3, 4)
CROSS_HALF_WIDTH = 3
RING_INNER = 8
RING_OUTER = 12
FRAME_ROWS = (3, 4, 5)
DIAMOND_RADIUS = 10
BAR_PERIOD = 6
BAR_WIDTH = 3
BAR_FROM_ROW = 20


def stripes_h(_x: int, y: int) -> bool:
    return (y // CELL) % 2 == 0


def stripes_v(x: int, _y: int) -> bool:
    return (x // CELL) % 2 == 0


def diagonal(x: int, y: int) -> bool:
    return ((x + y) // CELL) % 2 == 0


def checker(x: int, y: int) -> bool:
    return ((x // LARGE_CELL) + (y // LARGE_CELL)) % 2 == 0


def dots(x: int, y: int) -> bool:
    return x % LARGE_CELL in DOT_ON and y % LARGE_CELL in DOT_ON


def cross(x: int, y: int) -> bool:
    return abs(x - CENTRE) < CROSS_HALF_WIDTH or abs(y - CENTRE) < CROSS_HALF_WIDTH


def ring(x: int, y: int) -> bool:
    return RING_INNER <= ((x - CENTRE) ** 2 + (y - CENTRE) ** 2) ** 0.5 <= RING_OUTER


def frame(x: int, y: int) -> bool:
    return min(x, y, SIZE - 1 - x, SIZE - 1 - y) in FRAME_ROWS


def diamond(x: int, y: int) -> bool:
    return abs(x - CENTRE) + abs(y - CENTRE) < DIAMOND_RADIUS


def quarters(x: int, y: int) -> bool:
    return (x < HALF_SIDE) == (y < HALF_SIDE)


def bars(x: int, y: int) -> bool:
    return x % BAR_PERIOD < BAR_WIDTH and y > BAR_FROM_ROW


def corner(x: int, y: int) -> bool:
    return x + y < HALF_SIDE


# (name, base colour, pattern colour, pattern): the pattern says where the second colour is used.
TILES: list[tuple[str, tuple[int, int, int], tuple[int, int, int], Pattern]] = [
    ("tile-01-stripes-h", (220, 60, 60), (255, 220, 220), stripes_h),
    ("tile-02-stripes-v", (60, 160, 70), (210, 255, 210), stripes_v),
    ("tile-03-diagonal", (60, 90, 220), (210, 220, 255), diagonal),
    ("tile-04-checker", (230, 170, 40), (60, 40, 10), checker),
    ("tile-05-dots", (150, 70, 200), (255, 230, 255), dots),
    ("tile-06-cross", (40, 170, 170), (230, 255, 255), cross),
    ("tile-07-ring", (200, 90, 140), (255, 235, 245), ring),
    ("tile-08-frame", (110, 110, 110), (240, 240, 240), frame),
    ("tile-09-diamond", (90, 190, 40), (20, 60, 10), diamond),
    ("tile-10-quarters", (230, 120, 40), (255, 240, 200), quarters),
    ("tile-11-bars", (40, 60, 120), (150, 190, 255), bars),
    ("tile-12-corner", (170, 40, 70), (255, 200, 210), corner),
]


def pixel(
    base: tuple[int, int, int], other: tuple[int, int, int], pattern: Pattern, x: int, y: int
) -> Pixel:
    """The colour at (x, y): the pattern's colour or the base, with the outermost ring half transparent."""
    red, green, blue = other if pattern(x, y) else base
    ring = min(x, y, SIZE - 1 - x, SIZE - 1 - y) == 0
    return (red, green, blue, HALF if ring else 255)


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


def image(base: tuple[int, int, int], other: tuple[int, int, int], pattern: Pattern) -> bytes:
    """The whole PNG file of one tile."""
    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)  # filter type: none
        for x in range(SIZE):
            raw += bytes(pixel(base, other, pattern, x, y))
    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        SIGNATURE
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", stored(bytes(raw)))
        + chunk(b"IEND", b"")
    )


def files() -> dict[Path, bytes]:
    """Every file this script writes, by path."""
    return {
        DIRECTORY / f"{name}.png": image(base, other, pattern)
        for name, base, other, pattern in TILES
    }


def main(arguments: list[str]) -> int:
    expected = files()
    if arguments == ["--check"]:
        wrong = [
            path
            for path, data in expected.items()
            if not path.exists() or path.read_bytes() != data
        ]
        extra = (
            [path for path in DIRECTORY.glob("*.png") if path not in expected]
            if DIRECTORY.exists()
            else []
        )
        for path in wrong:
            print(f"{path} differs from what {Path(__file__).name} writes", file=sys.stderr)
        for path in extra:
            print(f"{path} is not written by {Path(__file__).name}", file=sys.stderr)
        return 1 if wrong or extra else 0
    if arguments:
        print(f"usage: {Path(__file__).name} [--check]", file=sys.stderr)
        return 2
    DIRECTORY.mkdir(exist_ok=True)
    for path, data in expected.items():
        path.write_bytes(data)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
