#!/usr/bin/env python3
"""Render the original SVG with Inkscape and package the same PNGs in an ICO.

This is an authoring tool only. Normal Cargo/Windows builds use checked-in assets
and do not require Python or Inkscape. No raster artwork is drawn by this script.
"""

import argparse
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile


SIZES = (16, 24, 32, 48, 64, 128, 256)
ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify without changing assets")
    args = parser.parse_args()
    inkscape = shutil.which("inkscape")
    if not inkscape:
        parser.error("Inkscape is required to regenerate the checked-in icon assets")

    with tempfile.TemporaryDirectory(prefix="coralspynext-icons-") as temp:
        env = dict(os.environ, INKSCAPE_PROFILE_DIR=str(Path(temp) / "inkscape-profile"),
                   XDG_CACHE_HOME=str(Path(temp) / "cache"))
        images = []
        for size in SIZES:
            output = Path(temp) / f"icon-{size}.png"
            subprocess.run(
                [inkscape, str(ASSETS / "coralspynext.svg"), "--export-area-page",
                 f"--export-width={size}", f"--export-height={size}",
                 f"--export-filename={output}"],
                env=env, check=True, stdout=subprocess.DEVNULL,
            )
            data = output.read_bytes()
            if data[:8] != b"\x89PNG\r\n\x1a\n" or struct.unpack(">II", data[16:24]) != (size, size):
                raise ValueError(f"Unexpected rendered PNG dimensions for {size}px")
            if data[24:26] != b"\x08\x06":
                raise ValueError(f"Expected 8-bit RGBA with transparency for {size}px")
            images.append(data)

    # Windows 11 supports PNG frames in ICON resources. Keep each rendered size,
    # rather than resizing one bitmap. The 256px PNG is also the eframe icon.
    directory = bytearray(struct.pack("<HHH", 0, 1, len(SIZES)))
    offset = 6 + 16 * len(SIZES)
    for size, data in zip(SIZES, images):
        directory.extend(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0,
                                     1, 32, len(data), offset))
        offset += len(data)
    artifacts = {
        ASSETS / "coralspynext.ico": bytes(directory) + b"".join(images),
        ASSETS / "coralspynext.png": images[-1],
    }
    for path, data in artifacts.items():
        if args.check:
            if not path.exists() or path.read_bytes() != data:
                raise SystemExit(f"Outdated icon asset: {path.relative_to(ROOT)}")
        else:
            path.write_bytes(data)
    print(f"{'Verified' if args.check else 'Generated'} icon sizes: {', '.join(map(str, SIZES))}")


if __name__ == "__main__":
    main()
