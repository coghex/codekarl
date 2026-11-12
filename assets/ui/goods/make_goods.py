"""Write and check the owner-approved goods icons.

python3 make_goods.py          write the images
python3 make_goods.py --check  fail if an image differs, is missing or extra
"""

import runpy
import sys
from pathlib import Path

# Goods icons, final art (owner: "Good, all 22 approved", owner-direction-goods-icons-r3.md, 2026-11-11).
# 22 goods x 9x9 UI pixels, style A (full colour, no outline). '.' is transparent; each letter is one PALETTE colour (#rrggbb, opaque).
# Frozen from mockups/make_goods_icons_r3.py (rounds 1-3); this file is the source for the shipped art.

PALETTE = {
    "A": "#eed88a",
    "B": "#9c6633",
    "C": "#f0e0b0",
    "E": "#50463a",
    "F": "#f8c040",
    "G": "#6ea83c",
    "H": "#e8a830",
    "L": "#a8d86a",
    "M": "#8a3a8a",
    "N": "#2a5a2a",
    "O": "#e07a2a",
    "P": "#e8a090",
    "R": "#c83a2c",
    "S": "#b8c8d0",
    "T": "#c49a6c",
    "V": "#9a2a4a",
    "W": "#f2ead6",
    "Y": "#d9a42e",
    "b": "#6b4522",
    "g": "#3c6a24",
    "k": "#2a221a",
    "q": "#c07868",
    "r": "#802218",
    "s": "#5c7484",
    "t": "#8a6440",
    "w": "#a89f8a",
    "y": "#9a6a1e",
}

ART = {
    "oats": [
        "..A...A..",
        "...A.A...",
        ".A..A..A.",
        "..A.A.A..",
        "...AAA...",
        "....y....",
        "...yyy...",
        "..y.y.y..",
        ".y..y..y.",
    ],
    "barley": [
        "b.b.b.b.b",
        ".bYbYbYb.",
        "..YYYYY..",
        "..YYYYY..",
        "...YYY...",
        "...YYY...",
        "....Y....",
        "....y....",
        "....y....",
    ],
    "peas_beans": [
        ".........",
        ".......g.",
        "......Gg.",
        ".....GLG.",
        "....GLG..",
        "...GLG...",
        "..GLG....",
        ".GGG.....",
        ".g.......",
    ],
    "vegetables": [
        "..G...G..",
        "...G.G...",
        "....G....",
        "..MMMMM..",
        ".MMMMMMM.",
        ".WWWWWWW.",
        "..WWWWW..",
        "...WWW...",
        "....W....",
    ],
    "apples": [
        "....b.GG.",
        "....bGG..",
        "..RRRRR..",
        ".RRWRRRR.",
        ".RRRRRRR.",
        ".RRRRRRr.",
        ".RRRRRrr.",
        "..RRRrr..",
        "...rrr...",
    ],
    "dairy": [
        ".........",
        "......AA.",
        "....AAAA.",
        "..AAAyAA.",
        "AAAAAAAA.",
        "AYYYYYYY.",
        "AYyYYYyY.",
        "AYYYYYYY.",
        ".........",
    ],
    "beef": [
        ".........",
        ".RRRRR...",
        "RRPRRRR..",
        "RRRRRRRWW",
        "RRPRRRRW.",
        "RRRRRRRWW",
        ".RRRPRR..",
        "..RRRR...",
        ".........",
    ],
    "pork": [
        "W........",
        ".W.......",
        "..WPP....",
        "..PPPPP..",
        ".PPPPPPP.",
        ".PPqPPPP.",
        "..PPPPPP.",
        "...PPPP..",
        ".........",
    ],
    "mutton": [
        "..W.W.W..",
        "..W.W.W..",
        "..W.W.W..",
        ".RRRRRRR.",
        "RRPRRRRRR",
        "RRRRRPRR.",
        ".RRRRRR..",
        "..RRRR...",
        ".........",
    ],
    "fish": [
        ".........",
        ".........",
        "..SSSS..s",
        ".SSSSSS.s",
        "SkSSSSSss",
        ".SSSSSS.s",
        "..ssss..s",
        ".........",
        ".........",
    ],
    "seabirds": [
        ".........",
        ".........",
        "w.......w",
        "ww.....ww",
        ".www.www.",
        "...WWW...",
        "....WO...",
        ".........",
        ".........",
    ],
    "ale": [
        ".WWWWW...",
        "WWWWWWW..",
        "BbBbBbBbb",
        "BbBbBbB.b",
        "BbBbBbB.b",
        "BbBbBbBbb",
        "BbBbBbB..",
        "BbBbBbB..",
        ".BBBBB...",
    ],
    "mead": [
        "...bbb...",
        "....A....",
        "...AYA...",
        "..AYYYA..",
        ".AYYWYYA.",
        ".AYYYYYA.",
        ".AYYYYYA.",
        "..AYYYA..",
        "...AAA...",
    ],
    "wine": [
        "....b....",
        "....N....",
        "...NNN...",
        "...NNN...",
        "..NNNNN..",
        "..NVVVN..",
        "..NVVVN..",
        "..NNNNN..",
        "...NNN...",
    ],
    "honey": [
        "..bbbbb..",
        "...HHH...",
        "..HHHHH..",
        ".HHHHHHH.",
        ".HHWHHHH.",
        ".HHHHHHH.",
        ".HHHHHHH.",
        ".HHHHHHH.",
        "..HHHHH..",
    ],
    "herbs": [
        ".G..G..G.",
        "GLG.G.GLG",
        ".GGLGLGG.",
        "..GGGGG..",
        "...GGG...",
        "...BBB...",
        "...ggg...",
        "..g.g.g..",
        ".g..g..g.",
    ],
    "wool": [
        "..WWWW...",
        ".WwWWwW..",
        "WWWwWWwW.",
        "WwWWwWWW.",
        "WWwWWwWW.",
        ".WWwWWW..",
        "..WWWW.W.",
        "......W..",
        ".....W...",
    ],
    "beeswax": [
        "....F....",
        "...FAF...",
        "....F....",
        "...CCC...",
        "...CCC...",
        "...CCC...",
        "...CCC...",
        "..CCCCC..",
        ".bbbbbbb.",
    ],
    "hides": [
        "T.......T",
        ".TTTTTTT.",
        ".TtTTTTT.",
        "TTTTTtTTT",
        ".TTTTTTT.",
        ".TTTtTTT.",
        ".TTTTTTT.",
        "T.......T",
        ".........",
    ],
    "cattle": [
        ".........",
        ".W.....W.",
        ".WBBBBBW.",
        "..BkBkB..",
        "..BBBBB..",
        "..BBBBB..",
        "..PPPPP..",
        "..PkPkP..",
        "...PPP...",
    ],
    "pigs": [
        ".P.....P.",
        "PPP...PPP",
        ".PPPPPPP.",
        "PPkPPPkPP",
        "PPPPPPPPP",
        "PPqqqqqPP",
        "PPqkqkqPP",
        ".PPqqqPP.",
        "..PPPPP..",
    ],
    "sheep": [
        "..WWWWW..",
        "WWWWWWWWW",
        "E.EEEEE.E",
        "..EWEWE..",
        "..EkEkE..",
        "..EEEEE..",
        "..EEEEE..",
        "...EPE...",
        "...EEE...",
    ],
}

HERE = Path(__file__).resolve().parent
FONT_PNG = runpy.run_path(str(HERE.parent / "font" / "make_font.py"))["png"]
WIDTH = 9
HEIGHT = 9


def images() -> dict[Path, bytes]:
    found: dict[Path, bytes] = {}
    for good, pattern in ART.items():
        if len(pattern) != HEIGHT or any(len(row) != WIDTH for row in pattern):
            raise ValueError(f"{good} must be a {WIDTH}x{HEIGHT} icon")
        rows = []
        for row in pattern:
            pixels = []
            for cell in row:
                if cell == ".":
                    pixels.append((0, 0, 0, 0))
                else:
                    colour = PALETTE[cell]
                    pixels.append(
                        (*[int(colour[offset : offset + 2], 16) for offset in (1, 3, 5)], 255)
                    )
            rows.append(pixels)
        found[HERE / f"{good}.png"] = FONT_PNG(rows)
    return found


def main(arguments: list[str]) -> int:
    wanted = images()
    if arguments == ["--check"]:
        problems = [
            f"{path} differs from ART, or is missing"
            for path, data in sorted(wanted.items())
            if not path.exists() or path.read_bytes() != data
        ]
        problems += [
            f"{path} is not in ART" for path in sorted(HERE.glob("*.png")) if path not in wanted
        ]
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1 if problems else 0
    if arguments:
        print(f"usage: {Path(__file__).name} [--check]", file=sys.stderr)
        return 2
    for path, data in sorted(wanted.items()):
        path.write_bytes(data)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
