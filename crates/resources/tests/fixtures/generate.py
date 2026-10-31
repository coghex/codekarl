"""Writes the PNG fixtures of `ck-resources`'s decoder tests, and the pixels each must decode to.

    python3 generate.py            write the fixtures next to this script
    python3 generate.py --check    fail if the committed files differ from what this script writes

The output is deterministic: the image data is deflated with *stored* blocks, so no compressor (and no zlib
version) is involved, and every chunk is written by hand. `expected.txt` has one line per fixture,
`name width height hex`, where `hex` is the premultiplied RGBA8 the decoder must produce. It is computed
here from the *unpacked samples* each fixture is defined by (never from the packed bytes), straight from the
rules the spec gives:

- a 1, 2 or 4-bit sample scales exactly to 8 bits (`v * 255 // max`); a 16-bit one by
  `(v * 255 + 32767) // 65535`;
- a grey or RGB pixel is transparent when its samples equal the `tRNS` key, compared at the image's own bit
  depth, before any reduction; a palette entry's alpha is its `tRNS` byte, or 255 past the end;
- each channel is premultiplied by `(c * a + 127) // 255`, and a transparent pixel is all zeros.
"""

import struct
import sys
import zlib
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent

SIGNATURE = b"\x89PNG\r\n\x1a\n"
GREY, RGB, PALETTE, GREY_ALPHA, RGBA = 0, 2, 3, 4, 6
EIGHT, SIXTEEN = 8, 16
STORED_BLOCK = 65535
MAX16 = 65535

# Adam7: (x0, y0, dx, dy) of each pass.
ADAM7 = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
]

# A palette's alpha bytes: shorter than the palette, so the entries past them are opaque.
ALPHAS = (0, 128, 51)


@dataclass
class Fixture:
    """One image, defined by its unpacked samples: `rows[y][x]` is a tuple with one entry per sample."""

    name: str
    color: int
    depth: int
    rows: list[list[tuple[int, ...]]]
    palette: list[tuple[int, int, int]] | None = None
    # A palette's alpha bytes, or a grey / RGB image's transparent sample (one value, or three).
    trns: tuple[int, ...] | None = None
    interlaced: bool = False
    extra_chunks: tuple[tuple[bytes, bytes], ...] = ()


def chunk(kind: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(kind + data)
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", crc)


def pack_row(samples: list[int], depth: int) -> bytes:
    """The samples of one row, packed the way PNG stores them (most significant bits first)."""
    if depth == EIGHT:
        return bytes(samples)
    if depth == SIXTEEN:
        return b"".join(struct.pack(">H", value) for value in samples)
    out = bytearray()
    per_byte = EIGHT // depth
    for start in range(0, len(samples), per_byte):
        byte = 0
        for offset in range(per_byte):
            index = start + offset
            value = samples[index] if index < len(samples) else 0
            byte |= value << (EIGHT - depth * (offset + 1))
        out.append(byte)
    return bytes(out)


def scanlines(fixture: Fixture) -> bytes:
    """Every scanline, each with filter type 0, in file order (Adam7 passes for an interlaced image)."""
    height, width = len(fixture.rows), len(fixture.rows[0])
    passes = ADAM7 if fixture.interlaced else [(0, 0, 1, 1)]
    out = bytearray()
    for x0, y0, dx, dy in passes:
        columns = list(range(x0, width, dx))
        for y in range(y0, height, dy):
            if not columns:
                continue
            samples = [value for x in columns for value in fixture.rows[y][x]]
            out += b"\x00" + pack_row(samples, fixture.depth)
    return bytes(out)


def stored_zlib(data: bytes) -> bytes:
    """A zlib stream of stored (uncompressed) deflate blocks: the same bytes from every machine."""
    out = bytearray(b"\x78\x01")
    blocks = [data[i : i + STORED_BLOCK] for i in range(0, len(data), STORED_BLOCK)] or [b""]
    for index, block in enumerate(blocks):
        final = 1 if index == len(blocks) - 1 else 0
        out += struct.pack("<BHH", final, len(block), len(block) ^ 0xFFFF) + block
    out += struct.pack(">I", zlib.adler32(data))
    return bytes(out)


def encode(fixture: Fixture) -> bytes:
    height, width = len(fixture.rows), len(fixture.rows[0])
    header = struct.pack(
        ">IIBBBBB", width, height, fixture.depth, fixture.color, 0, 0, int(fixture.interlaced)
    )
    out = bytearray(SIGNATURE + chunk(b"IHDR", header))
    for kind, data in fixture.extra_chunks:
        out += chunk(kind, data)
    if fixture.palette is not None:
        out += chunk(b"PLTE", bytes(channel for entry in fixture.palette for channel in entry))
    if fixture.trns is not None:
        if fixture.color == PALETTE:
            out += chunk(b"tRNS", bytes(fixture.trns))
        else:
            out += chunk(b"tRNS", b"".join(struct.pack(">H", value) for value in fixture.trns))
    out += chunk(b"IDAT", stored_zlib(scanlines(fixture))) + chunk(b"IEND", b"")
    return bytes(out)


def to_eight(value: int, depth: int) -> int:
    if depth == SIXTEEN:
        return (value * 255 + 32767) // MAX16
    if depth == EIGHT:
        return value
    return value * 255 // ((1 << depth) - 1)


def premultiply(channel: int, alpha: int) -> int:
    return (channel * alpha + 127) // 255


def expected_pixel(fixture: Fixture, samples: tuple[int, ...]) -> tuple[int, int, int, int]:
    depth = fixture.depth
    if fixture.color == PALETTE:
        assert fixture.palette is not None
        red, green, blue = fixture.palette[samples[0]]
        alphas = fixture.trns or ()
        alpha = alphas[samples[0]] if samples[0] < len(alphas) else 255
    elif fixture.color in (GREY, RGB):
        if fixture.trns is not None and samples == fixture.trns:
            return (0, 0, 0, 0)
        reduced = [to_eight(value, depth) for value in samples]
        red, green, blue = reduced * 3 if fixture.color == GREY else reduced
        alpha = 255
    else:
        reduced = [to_eight(value, depth) for value in samples]
        if fixture.color == GREY_ALPHA:
            red = green = blue = reduced[0]
            alpha = reduced[1]
        else:
            red, green, blue, alpha = reduced
    if alpha == 0:
        return (0, 0, 0, 0)
    return (premultiply(red, alpha), premultiply(green, alpha), premultiply(blue, alpha), alpha)


def expected(fixture: Fixture) -> bytes:
    return bytes(
        channel
        for row in fixture.rows
        for samples in row
        for channel in expected_pixel(fixture, samples)
    )


def grid(width: int, values: list[tuple[int, ...]]) -> list[list[tuple[int, ...]]]:
    """`values` cut into rows of `width`."""
    return [values[i : i + width] for i in range(0, len(values), width)]


def one(*values: int) -> list[tuple[int, ...]]:
    return [(value,) for value in values]


def fixtures() -> list[Fixture]:
    found: list[Fixture] = []

    def add(*args: object, **kwargs: object) -> None:
        found.append(Fixture(*args, **kwargs))  # type: ignore[arg-type]

    # Grey, every depth. 5 wide, so the 1, 2 and 4-bit rows are padded to a byte.
    add("grey1", GREY, 1, grid(5, one(0, 1, 1, 0, 1, 1, 0, 0, 1, 0)))
    add("grey2", GREY, 2, grid(5, one(0, 1, 2, 3, 2, 3, 2, 1, 0, 1)))
    add("grey4", GREY, 4, grid(3, one(0, 5, 15, 10, 1, 8)))
    add("grey8", GREY, 8, grid(4, one(0, 1, 128, 255)))
    add("grey16", GREY, 16, grid(5, one(0, 0x00FF, 0x1234, 0x8000, 0xFFFF)))
    # Grey with alpha.
    add("greyalpha8", GREY_ALPHA, 8, grid(2, [(255, 255), (128, 128), (200, 0), (100, 51)]))
    add(
        "greyalpha16",
        GREY_ALPHA,
        16,
        grid(2, [(0xFFFF, 0xFFFF), (0x8000, 0x8000), (0x1234, 0), (0x00FF, 0x0100)]),
    )
    # RGB and RGBA.
    add("rgb8", RGB, 8, grid(2, [(255, 0, 0), (0, 255, 0), (0, 0, 255), (10, 20, 30)]))
    add(
        "rgb16",
        RGB,
        16,
        grid(
            2, [(0xFFFF, 0, 0x00FF), (0x8000, 0x1234, 0xABCD), (0, 0, 0), (0x0101, 0x0202, 0x0303)]
        ),
    )
    add(
        "rgba8",
        RGBA,
        8,
        grid(2, [(200, 100, 50, 255), (200, 100, 50, 128), (200, 100, 50, 1), (200, 100, 50, 0)]),
    )
    add(
        "rgba16",
        RGBA,
        16,
        grid(
            2,
            [
                (0xFFFF, 0x8000, 0x4000, 0xFFFF),
                (0xFFFF, 0x8000, 0x4000, 0x8080),
                (0x1234, 0x5678, 0x9ABC, 0x00FF),
                (0xFFFF, 0xFFFF, 0xFFFF, 0),
            ],
        ),
    )
    # Palettes, with and without tRNS, at every depth. The palette is longer than the image uses; the tRNS
    # is shorter than the palette, so the entries past it are opaque.
    palette = [
        (255, 0, 0),
        (0, 255, 0),
        (0, 0, 255),
        (255, 255, 0),
        (10, 20, 30),
        (200, 100, 50),
        (1, 2, 3),
        (250, 250, 250),
    ]
    for depth, count in ((1, 2), (2, 4), (4, 6), (8, 8)):
        indexes = [(i * 3 + 1) % count for i in range(10)]
        rows = grid(5, one(*indexes))
        add(f"palette{depth}", PALETTE, depth, rows, palette=palette[:count])
        add(
            f"palette{depth}-trns",
            PALETTE,
            depth,
            rows,
            palette=palette[:count],
            trns=ALPHAS[: count - 1],
        )
    # Transparent grey and RGB samples, compared at the image's own depth.
    add("grey1-trns", GREY, 1, grid(5, one(0, 1, 1, 0, 1, 1, 0, 0, 1, 0)), trns=(1,))
    add("grey2-trns", GREY, 2, grid(5, one(0, 1, 2, 3, 2, 3, 2, 1, 0, 1)), trns=(2,))
    add("grey4-trns", GREY, 4, grid(3, one(0, 5, 15, 10, 1, 8)), trns=(5,))
    add("grey8-trns", GREY, 8, grid(4, one(0, 1, 128, 255)), trns=(128,))
    # 0x1234 is transparent. 0x1250 and 0x12FF share its high byte and are not; 0x1250 also reduces to the same
    # 8-bit value as the key (18), so comparing after reducing to 8 bits, by rounding or by dropping the low byte,
    # would get it wrong.
    add(
        "grey16-trns",
        GREY,
        16,
        grid(6, one(0x1234, 0x1250, 0x12FF, 0x3412, 0x0000, 0x1234)),
        trns=(0x1234,),
    )
    add(
        "rgb8-trns",
        RGB,
        8,
        grid(2, [(10, 20, 30), (10, 20, 31), (255, 0, 0), (10, 20, 30)]),
        trns=(10, 20, 30),
    )
    add(
        "rgb16-trns",
        RGB,
        16,
        grid(
            2,
            [
                (0x1234, 0x5678, 0x9ABC),
                (0x1234, 0x5678, 0x9AFF),
                (0x1200, 0x5600, 0x9A00),
                (0x1234, 0x5678, 0x9ABC),
            ],
        ),
        trns=(0x1234, 0x5678, 0x9ABC),
    )
    # Interlaced, so some Adam7 passes are empty and rows are packed per pass.
    grey1_rows = grid(9, one(*[(i * 5 + i // 3) % 2 for i in range(27)]))
    add("grey1-9x3", GREY, 1, grey1_rows)
    add("grey1-adam7", GREY, 1, grey1_rows, interlaced=True)
    add(
        "grey4-trns-adam7",
        GREY,
        4,
        grid(6, one(*[(i * 7) % 16 for i in range(24)])),
        trns=(7,),
        interlaced=True,
    )
    add(
        "palette2-trns-adam7",
        PALETTE,
        2,
        grid(6, one(*[(i * 3) % 4 for i in range(30)])),
        palette=palette[:4],
        trns=(0, 128),
        interlaced=True,
    )
    rgb8_rows = grid(5, [(i * 17 % 256, i * 29 % 256, i * 43 % 256) for i in range(15)])
    add("rgb8-5x3", RGB, EIGHT, rgb8_rows)
    add("rgb8-adam7", RGB, EIGHT, rgb8_rows, interlaced=True)
    add(
        "rgba16-adam7",
        RGBA,
        16,
        grid(
            3,
            [
                (i * 7001 % 65536, i * 9001 % 65536, i * 11003 % 65536, (i + 1) * 7000 % 65536)
                for i in range(9)
            ],
        ),
        interlaced=True,
    )
    add(
        "greyalpha8-adam7",
        GREY_ALPHA,
        8,
        grid(4, [(i * 31 % 256, 255 - i * 15) for i in range(16)]),
        interlaced=True,
    )
    # Chunks the decoder skips are present and must change nothing.
    add(
        "rgba8-metadata",
        RGBA,
        8,
        grid(2, [(200, 100, 50, 255), (200, 100, 50, 128), (1, 2, 3, 4), (0, 0, 0, 0)]),
        extra_chunks=(
            (b"gAMA", struct.pack(">I", 45455)),
            (b"sRGB", b"\x00"),
            (b"tEXt", b"Comment\x00skipped"),
            (b"prVt", b"unknown ancillary chunk"),
        ),
    )
    return found


def manifest(items: list[Fixture]) -> str:
    return "".join(
        f"{item.name} {len(item.rows[0])} {len(item.rows)} {expected(item).hex()}\n"
        for item in items
    )


def main() -> int:
    items = fixtures()
    files = {f"{item.name}.png": encode(item) for item in items}
    files["expected.txt"] = manifest(items).encode()
    if "--check" in sys.argv[1:]:
        stale = [
            name
            for name, data in files.items()
            if not (HERE / name).is_file() or (HERE / name).read_bytes() != data
        ]
        extra = sorted(path.name for path in HERE.glob("*.png") if path.name not in files)
        for name in [*stale, *extra]:
            sys.stdout.write(f"differs: {name}\n")
        return 1 if stale or extra else 0
    for name, data in files.items():
        (HERE / name).write_bytes(data)
    sys.stdout.write(f"wrote {len(files)} files\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
