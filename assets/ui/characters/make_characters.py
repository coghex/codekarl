"""Write and check the character screen's icons and the unknown portrait (CK-375, characters slice 9a).

python3 make_characters.py          write the images
python3 make_characters.py --check  fail if an image differs, is missing or extra

The icons are the owner-approved 9x9 colour set for the character window (owner r85-r87, round 2): full colour, no outline, in the goods
icons' style (`assets/ui/goods/make_goods.py`). '.' is transparent; each letter is one PALETTE colour (#rrggbb, opaque). Frozen from the
designer's `history-scratch/tools/char_icons.py` (SHA-256 86dc5a31bce827c29f656889a42de47fea41dbad39974040ff614cc0eeec485c); this file is the
source for the shipped art. Only the drawings the character screen's first two tabs use are here; the rest of that set (actions, pop)
joins as the later slices use them. The main screen's icons and every list's filter and sort buttons stay monochrome (owner r88, r89): the
ledger's bust is `assets/ui/theme/icon_characters.png`, never one of these.

Provenance: every drawing is composed pixel by pixel from plain object shapes (a book, a candle, a brain, a shield, a horse's head, a
drumstick, a loaf, a flame, a cloud, a magnifying glass, a ladder, three figures ...); none is copied or traced from another game or icon
set. The palette starts from the goods icons' palette and adds blues and greys.

The unknown portrait (owner r64: every character shows it until the layered portraits exist) is a plain head and shoulders drawn here from
rounded rows of pixels, copied from no other image. It is one white silhouette on a transparent cell at two sizes (the ledger card's 32
pixels and the character screen header's 48); the UI tints it with the theme's `dim` over the dark cell it draws.
"""

import runpy
import sys
from pathlib import Path

PALETTE = {
    "A": "#eed88a",
    "B": "#9c6633",
    "C": "#f0e0b0",
    "D": "#d8dce0",
    "E": "#50463a",
    "F": "#f8c040",
    "G": "#6ea83c",
    "L": "#a8d86a",
    "M": "#8a3a8a",
    "O": "#e07a2a",
    "P": "#e8a090",
    "Q": "#e86a9a",
    "R": "#c83a2c",
    "S": "#b8c8d0",
    "T": "#c49a6c",
    "U": "#5a8fd0",
    "V": "#9a2a4a",
    "W": "#f2ead6",
    "Y": "#d9a42e",
    "b": "#6b4522",
    "e": "#7a6e5e",
    "g": "#3c6a24",
    "k": "#2a221a",
    "q": "#c07868",
    "r": "#802218",
    "s": "#5c7484",
    "w": "#a89f8a",
    "y": "#9a6a1e",
}

# The drawings the first tab and the relations tab use, in the order of their sections. Every name is a key of ART, and each is written as `<name>.png`.
ART = {
    # body
    "height": [
        "SSSSSSSSS",
        "....W....",
        "...WWW...",
        "....W....",
        "....W....",
        "....W....",
        "...WWW...",
        "....W....",
        "SSSSSSSSS",
    ],
    "weight": [
        "...www...",
        "..w...w..",
        "..w...w..",
        ".sssssss.",
        "ssSSsssss",
        "sSSssssss",
        "sssssssss",
        "sssssssss",
        ".sssssss.",
    ],
    "strength": [
        ".........",
        ".........",
        ".s.....s.",
        "ss.....ss",
        "sswwwwwss",
        "sswwwwwss",
        "ss.....ss",
        ".s.....s.",
        ".........",
    ],
    "quill": [
        "......WWW",
        ".....WWWW",
        "....WWWW.",
        "...WWWW..",
        "..WWWW...",
        "..WWW....",
        ".ww......",
        ".b.......",
        "b........",
    ],
    "brain": [
        ".........",
        "..PPqPP..",
        ".PqPqPPq.",
        "PPPqqPqPP",
        "PqPPqPPqP",
        "PPqPqqPPP",
        "PqPPqPqPP",
        ".PPqqPPq.",
        "..PPqPP..",
    ],
    "fox": [
        "O.......O",
        "OO.....OO",
        "OOO...OOO",
        "OOOOOOOOO",
        "OkOOOOOkO",
        "WOOOOOOOW",
        ".WWOOOWW.",
        "..WWkWW..",
        "...WWW...",
    ],
    "heart": [
        ".........",
        ".RR...RR.",
        "RRRR.RRRR",
        "RWRRRRRRR",
        "RRRRRRRRR",
        ".RRRRRRR.",
        "..RRRRR..",
        "...RRR...",
        "....R....",
    ],
    "shield": [
        "sssssssss",
        "sSSSSSSSs",
        "sSSSSSSSs",
        "sSSSwSSSs",
        "sSSwwwSSs",
        "sSSSwSSSs",
        ".sSSSSSs.",
        "..sSSSs..",
        "...sss...",
    ],
    "egg": [
        "....C....",
        "...CCC...",
        "..CCCCC..",
        "..CWCCC..",
        ".CWCCCCC.",
        ".CCCCCCC.",
        ".CCCCCCT.",
        "..CCCCT..",
        "...TTT...",
    ],
    # health
    "cut": [
        ".PPPPPPP.",
        "PPPPPPPRP",
        "PPPPPPRrP",
        "PPPPPRrPP",
        "PPPPRrPPP",
        "PPPRrPPPP",
        "PPRrPPPPP",
        "PPPRPPPPP",
        ".PPRPPPP.",
    ],
    "bruise": [
        ".........",
        "...MMM...",
        "..MVVVM..",
        ".MVrrrVM.",
        ".MVrrrVM.",
        ".MVVrVVM.",
        "..MVVVM..",
        "...MMM...",
        ".........",
    ],
    # skills
    "flag": [
        "bRRRRRR..",
        "bRRRRRRR.",
        "bRRRRRRR.",
        "bRRRRRR..",
        "b........",
        "b........",
        "b........",
        "b........",
        "b........",
    ],
    "hammer": [
        ".sssssss.",
        ".sSSSSss.",
        ".sssssss.",
        "....b....",
        "....b....",
        "....b....",
        "....b....",
        "....b....",
        "....b....",
    ],
    "anchor": [
        "....S....",
        "...S.S...",
        "....S....",
        "..SSSSS..",
        "....S....",
        "....S....",
        "S...S...S",
        ".S..S..S.",
        "..SSSSS..",
    ],
    "book": [
        ".........",
        "CCCC.CCCC",
        "CwwCbCwwC",
        "CCCCbCCCC",
        "CwwCbCwwC",
        "CCCCbCCCC",
        "CwwCbCwwC",
        "BBBBBBBBB",
        ".........",
    ],
    "horse": [
        "....bb...",
        "...BBBb..",
        "..BkBBBb.",
        ".BBBBBBb.",
        "BBBBBBBb.",
        "BB..BBBb.",
        "....BBBb.",
        "....BBBB.",
        "....BBBB.",
    ],
    "sword": [
        "....W....",
        "....S....",
        "....S....",
        "....S....",
        "....S....",
        "..YYYYY..",
        "....b....",
        "....b....",
        "....Y....",
    ],
    "speech": [
        ".WWWWWWW.",
        "WWWWWWWWW",
        "WWkWkWkWW",
        "WWWWWWWWW",
        ".WWWWWWW.",
        "..WW.....",
        ".WW......",
        "W........",
        ".........",
    ],
    "scroll": [
        ".TTTTTTT.",
        "..CCCCC..",
        "..CwwwC..",
        "..CCCCC..",
        "..CwwwC..",
        "..CCCCC..",
        "..CwwCC..",
        ".TTTTTTT.",
        ".........",
    ],
    "lamp": [
        ".....F...",
        "....FOF..",
        "....FOF..",
        ".....y...",
        "YYYYYYYY.",
        "YAYYYYYYY",
        ".YYYYYYY.",
        "...YYY...",
        "..yyyyy..",
    ],
    "mortar": [
        ".......b.",
        "......b..",
        ".....b...",
        "ssssbssss",
        "sSSSSSSSs",
        ".sSSSSSs.",
        "..sssss..",
        "...sss...",
        "..sssss..",
    ],
    "cow": [
        "C.......C",
        ".C.....C.",
        "..BBBBB..",
        ".BkBBBkB.",
        ".BBBWBBB.",
        "..BWWWB..",
        "..PPPPP..",
        "..PkPkP..",
        "...PPP...",
    ],
    # virtue, the sin's end
    "crown": [
        ".........",
        "Y...Y...Y",
        "YY.YYY.YY",
        "YYYYYYYYY",
        "YRYYUYYRY",
        "YYYYYYYYY",
        "yyyyyyyyy",
        ".........",
        ".........",
    ],
    "coins": [
        ".YYYYYYY.",
        "yYYYYYYYy",
        ".yyyyyyy.",
        ".YYYYYYY.",
        "yYYYYYYYy",
        ".yyyyyyy.",
        ".YYYYYYY.",
        "yYYYYYYYy",
        ".yyyyyyy.",
    ],
    "rose": [
        "..RRRR...",
        ".RrRRrR..",
        ".RRrrRR..",
        "..RRRR...",
        "...gG....",
        "G..g.....",
        "GG.g.GG..",
        "...g.G...",
        "...g.....",
    ],
    "eye_green": [
        ".........",
        "...WWW...",
        ".WWWWWWW.",
        "WWWGGGWWW",
        "WWGGkGGWW",
        "WWWGGGWWW",
        ".WWWWWWW.",
        "...WWW...",
        ".........",
    ],
    "drumstick": [
        "...BBB...",
        "..BBBBB..",
        ".BBTBBBB.",
        ".BTBBBBB.",
        ".BBBBBB..",
        "..BBBB...",
        "....WW...",
        ".....WW..",
        "....W.W..",
    ],
    "flame": [
        "....O....",
        "...OO....",
        "...OOO.O.",
        "..OOFOOO.",
        ".OOFFFOO.",
        ".OFFWFFO.",
        "OOFFFFFOO",
        ".OOFFFOO.",
        "..OOOOO..",
    ],
    "zz": [
        "UUUUU....",
        "...U.....",
        "..U......",
        ".U.......",
        "UUUUU....",
        ".....UUUU",
        ".......U.",
        "......U..",
        ".....UUUU",
    ],
    # temper
    "drop": [
        "....R....",
        "....R....",
        "...RRR...",
        "..RRRRR..",
        ".RRRRRRR.",
        ".RWRRRRR.",
        ".RRWRRRR.",
        "..RRRRR..",
        "...RRR...",
    ],
    "sun": [
        "F...F...F",
        ".F.FFF.F.",
        "..FFFFF..",
        ".FFFFFFF.",
        "FFFFFFFFF",
        ".FFFFFFF.",
        "..FFFFF..",
        ".F.FFF.F.",
        "F...F...F",
    ],
    "moon": [
        "..www....",
        ".wEE.....",
        "wEE......",
        "wE.......",
        "wE.......",
        "wE.......",
        "wEE......",
        ".wEE.....",
        "..www....",
    ],
    "waves": [
        ".........",
        ".WW...WW.",
        "W..W.W..W",
        "....W....",
        ".........",
        ".WW...WW.",
        "W..W.W..W",
        "....W....",
        ".........",
    ],
    "cloud": [
        "...www...",
        ".wwwwwww.",
        "wwwwwwwww",
        ".wwwwwww.",
        ".........",
        ".U..U..U.",
        "U..U..U..",
        "..U..U..U",
        ".........",
    ],
    # manner, one end
    "door": [
        "..bbbbb..",
        ".bBBBBBb.",
        ".bBBBBBb.",
        ".bBBBBBb.",
        ".bBBBBYb.",
        ".bBBBBBb.",
        ".bBBBBBb.",
        ".bBBBBBb.",
        ".bbbbbbb.",
    ],
    "mask": [
        ".........",
        ".........",
        "wVVVVVVVw",
        "VVkkVkkVV",
        "VVkkVkkVV",
        ".VVV.VVV.",
        "..V...V..",
        ".........",
        ".........",
    ],
    "lens": [
        "..sss....",
        ".sWWWs...",
        "sWUUUWs..",
        "sWUUUWs..",
        "sWUUUWs..",
        ".sWWWs...",
        "..sssbb..",
        "......bb.",
        ".......bb",
    ],
    "bolt": [
        ".....FF..",
        "....FF...",
        "...FF....",
        "..FFFFF..",
        "....FF...",
        "...FF....",
        "..FF.....",
        ".F.......",
        "F........",
    ],
    "question": [
        "..UUUU...",
        ".UU..UU..",
        ".....UU..",
        "....UU...",
        "...UU....",
        "...UU....",
        ".........",
        "...UU....",
        ".........",
    ],
    "stone": [
        ".........",
        ".........",
        "...www...",
        "..wwDww..",
        ".wwDwwww.",
        "wwwwwwwww",
        "wewwwweww",
        "eeeeeeeee",
        ".........",
    ],
    "crowd": [
        "....P....",
        ".P.PPP.P.",
        "PPP.P.PPP",
        ".P.UUU.P.",
        "RRRUUURRR",
        "RRRUUURRR",
        "RRRUUURRR",
        "R.R.U.R.R",
        ".........",
    ],
    "ladder": [
        "..b...b..",
        "..bbbbb..",
        "..b...b..",
        "..bbbbb..",
        "..b...b..",
        "..bbbbb..",
        "..b...b..",
        "..bbbbb..",
        "..b...b..",
    ],
    "hand": [
        "..P.P.P..",
        "..P.P.P.P",
        "..PPPPPPP",
        "P.PPPPPPP",
        "PPPPPPPPP",
        ".PPPPPPP.",
        "..PPPPP..",
        "..PPPPP..",
        "...PPP...",
    ],
    # standing
    "pair": [
        ".........",
        ".........",
        ".PP...PP.",
        "PPPP.PPPP",
        ".PP...PP.",
        "UUUU.RRRR",
        "UUUU.RRRR",
        "UUUU.RRRR",
        ".........",
    ],
    "star": [
        "....Y....",
        "....Y....",
        "...YYY...",
        "YYYYYYYYY",
        ".YYYYYYY.",
        "..YYYYY..",
        "..YY.YY..",
        ".YY...YY.",
        "YY.....YY",
    ],
    "laurel": [
        "G.......G",
        "GG.....GG",
        ".G.....G.",
        ".GG...GG.",
        "..G...G..",
        "..GG.GG..",
        "...G.G...",
        "....Y....",
        ".........",
    ],
    # relations (slice 9b: the formal ties under a portrait and the kinds of strand)
    "tree": [
        "...GGG...",
        ".GGGGGGG.",
        "GGGLGGGGG",
        "GGGGGGLGG",
        ".GGGGGGG.",
        "...GbG...",
        "....b....",
        "....b....",
        "..bbbbb..",
    ],
    "bowl": [
        ".........",
        "...w.w...",
        "....w....",
        "bbbbbbbbb",
        "BTTTTTTTB",
        ".BTTTTTB.",
        "..BBBBB..",
        "...bbb...",
        ".........",
    ],
    "lock": [
        "..sssss..",
        ".s.....s.",
        ".s.....s.",
        ".s.....s.",
        "sSSSSSSSs",
        "sSSSkSSSs",
        "sSSSkSSSs",
        "sSSSSSSSs",
        ".sssssss.",
    ],
    "handshake": [
        ".........",
        ".........",
        "UU.....RR",
        "UUUPPPRRR",
        "UUPPPPPRR",
        "UU.PPP.RR",
        ".........",
        ".........",
        ".........",
    ],
    "fist": [
        ".PPPPPP..",
        "PPPPPPPP.",
        "PqPqPqPP.",
        "PPPPPPPPP",
        "PPPPPPPPP",
        ".PPPPPPP.",
        "..PPPPP..",
        "..UUUUU..",
        "..UUUUU..",
    ],
    "lovers": [
        ".........",
        "RR.RR....",
        "RRRRR....",
        ".RRR.....",
        "..R.QQ.QQ",
        "....QQQQQ",
        ".....QQQ.",
        "......Q..",
        ".........",
    ],
    "ghost": [
        "..WWWWW..",
        ".WWWWWWW.",
        "WWkWWWkWW",
        "WWkWWWkWW",
        "WWWWWWWWW",
        "WWWkkkWWW",
        ".WWkkkWW.",
        ".WWWWWWW.",
        "..W.W.W..",
    ],
    "angry": [
        "..RRRRR..",
        ".RRRRRRR.",
        "RkRRRRRkR",
        "RRkRRRkRR",
        "RRRRRRRRR",
        "RRRkkkRRR",
        "RRkRRRkRR",
        ".RRRRRRR.",
        "..RRRRR..",
    ],
    "star_pale": [
        "....A....",
        "....A....",
        "...AAA...",
        "AAAAAAAAA",
        ".AAAAAAA.",
        "..AAAAA..",
        "..AA.AA..",
        ".AA...AA.",
        "AA.....AA",
    ],
}

HERE = Path(__file__).resolve().parent
FONT_PNG = runpy.run_path(str(HERE.parent / "font" / "make_font.py"))["png"]
WIDTH = 9
HEIGHT = 9
CLEAR = (0, 0, 0, 0)
WHITE = (255, 255, 255, 255)


def image(pattern: list[str], name: str) -> bytes:
    if len(pattern) != HEIGHT or any(len(row) != WIDTH for row in pattern):
        raise ValueError(f"{name} must be a {WIDTH}x{HEIGHT} icon")
    rows = []
    for row in pattern:
        pixels = []
        for cell in row:
            if cell == ".":
                pixels.append(CLEAR)
            else:
                colour = PALETTE[cell]
                pixels.append(
                    (*[int(colour[offset : offset + 2], 16) for offset in (1, 3, 5)], 255)
                )
        rows.append(pixels)
    return FONT_PNG(rows)


# The head's half-widths, row by row, at the header's size (48 pixels) and at the card's (32).
CARD_SIZE = 32
HEADER_SIZE = 48
HEAD_LARGE = (3, 5, 6, 6, 7, 7, 7, 7, 6, 6, 5, 4, 3)
HEAD_SMALL = (2, 4, 4, 5, 5, 5, 4, 4, 2)


def portrait(size: int) -> bytes:
    """The unknown portrait: a head, a neck and shoulders in white on a transparent `size` by `size` cell."""
    pixels = [[CLEAR] * size for _ in range(size)]
    centre = size // 2

    def row(y: int, half: int) -> None:
        for x in range(centre - half, centre + half):
            pixels[y][x] = WHITE

    if size == HEADER_SIZE:
        for j, half in enumerate(HEAD_LARGE):
            row(8 + j, half)
        for j in range(size - 3 - 21):
            row(21 + j, min(18, 3 + j * 2))
    elif size == CARD_SIZE:
        for j, half in enumerate(HEAD_SMALL):
            row(6 + j, half)
        for j in range(2):
            row(15 + j, 2)
        for j in range(13):
            row(17 + j, min(12, 5 + j * 2))
    else:
        raise ValueError(f"no unknown portrait of {size} pixels")
    return FONT_PNG(pixels)


def images() -> dict[Path, bytes]:
    found = {HERE / f"{name}.png": image(pattern, name) for name, pattern in ART.items()}
    found[HERE / "unknown_card.png"] = portrait(CARD_SIZE)
    found[HERE / "unknown_header.png"] = portrait(HEADER_SIZE)
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
