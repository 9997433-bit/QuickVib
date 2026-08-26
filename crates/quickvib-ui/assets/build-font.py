#!/usr/bin/env python3
"""Rebuild the CJK font the window embeds.

egui ships Latin fonts only, so a Chinese interface drawn with the stock font set is a window
full of tofu. QuickVib therefore embeds a font, and embedding it means shipping the bytes in
the repository — a download at build time would break the offline, reproducible `cargo build`
the rest of this workspace is careful to keep.

The full Noto Sans SC is ~17 MB, which has no business in a source tree, so what is committed
is a subset: Latin, the punctuation and symbols the panel uses, and the 3755 hanzi of GB 2312
level 1 — the everyday character set, which covers the interface, project names and Chinese
file paths. That lands at ~1.1 MB.

Run this script only to regenerate the asset (a new label with a rarer character, or a font
update). It needs network access and `pip install fonttools brotli`:

    python3 crates/quickvib-ui/assets/build-font.py

`cargo test -p quickvib-ui` fails if any label in `i18n` uses a character the committed font
does not carry, so a forgotten regeneration is caught without opening a window.

Font: Noto Sans SC, Copyright 2014-2021 Adobe, SIL Open Font License 1.1 (see OFL.txt).
"""

from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile
import urllib.request

SOURCE_URL = (
    "https://raw.githubusercontent.com/google/fonts/main/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf"
)
LICENSE_URL = "https://raw.githubusercontent.com/google/fonts/main/ofl/notosanssc/OFL.txt"

HERE = pathlib.Path(__file__).resolve().parent
OUTPUT = HERE / "NotoSansSC-Regular-subset.ttf"

# Latin, Latin-1, the typographic punctuation egui draws, the physical symbols the panel uses
# (µ ° ² ³ × ÷), geometric shapes for the state LEDs, and CJK/full-width punctuation.
UNICODES = ",".join(
    [
        "U+0020-007E",
        "U+00A0-00FF",
        "U+00B0",
        "U+00B2",
        "U+00B3",
        "U+00B5",
        "U+00D7",
        "U+00F7",
        "U+03BC",
        "U+2010-201F",
        "U+2026",
        "U+2030",
        "U+2032-2033",
        "U+203B",
        "U+2103",
        "U+2116",
        "U+2190-2193",
        "U+21BB",
        "U+2500-257F",
        "U+25A0-25CF",
        "U+2713",
        "U+2717",
        "U+3000-303F",
        "U+FF01-FF5E",
    ]
)


def gb2312_level_1() -> str:
    """The 3755 most common simplified hanzi, in the order the GB 2312 table lists them."""
    chars = []
    for high in range(0xB0, 0xD8):
        for low in range(0xA1, 0xFF):
            try:
                chars.append(bytes([high, low]).decode("gb2312"))
            except UnicodeDecodeError:
                continue
    return "".join(chars)


def main() -> int:
    try:
        import fontTools  # noqa: F401
    except ImportError:
        print("this script needs fonttools: pip install fonttools brotli", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        variable = tmp / "NotoSansSC-variable.ttf"
        print(f"downloading {SOURCE_URL}")
        urllib.request.urlretrieve(SOURCE_URL, variable)
        urllib.request.urlretrieve(LICENSE_URL, HERE / "OFL.txt")

        # The upstream file is a variable font; the window wants one static regular weight.
        static = tmp / "NotoSansSC-Regular.ttf"
        print("pinning wght=400")
        subprocess.run(
            [sys.executable, "-m", "fontTools.varLib.instancer",
             str(variable), "wght=400", "-o", str(static)],
            check=True,
            stdout=subprocess.DEVNULL,
        )

        text = tmp / "glyphs.txt"
        text.write_text(gb2312_level_1(), encoding="utf-8")

        print(f"subsetting into {OUTPUT.name}")
        subprocess.run(
            [sys.executable, "-m", "fontTools.subset", str(static),
             f"--text-file={text}",
             f"--unicodes={UNICODES}",
             "--layout-features=",
             "--no-hinting",
             "--drop-tables+=DSIG",
             f"--output-file={OUTPUT}"],
            check=True,
        )

    print(f"{OUTPUT} is {OUTPUT.stat().st_size / 1024:.0f} KiB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
