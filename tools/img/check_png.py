#!/usr/bin/env python3
"""Check that a PNG is a PNG, without opening it in anything.

A file with a `.png` name and the right first eight bytes can still be
truncated, or have a header that disagrees with its contents. This reads the
structure instead of trusting either: every chunk's CRC, the dimensions in the
header, and that the compressed pixel data actually inflates to exactly the
number of bytes those dimensions imply.

Nothing outside the standard library, and nothing is downloaded.

    python tools/img/check_png.py docs/public-drafts/media/*.png

Exit 0 when every file checks out, 1 otherwise. `--expect WxH` additionally
requires those dimensions, which is how a screenshot that was captured at the
wrong resolution gets noticed before it reaches a README.
"""

from __future__ import annotations

import argparse
import binascii
import struct
import sys
import zlib
from pathlib import Path

SIGNATURE = b"\x89PNG\r\n\x1a\n"

# Bytes per pixel for each colour type in the PNG spec: greyscale, truecolour,
# indexed, greyscale+alpha, truecolour+alpha.
CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


class Bad(Exception):
    """One structural check did not hold. The message says which."""


def check(path: Path, expect: tuple[int, int] | None) -> list[str]:
    data = path.read_bytes()
    if data[:8] != SIGNATURE:
        raise Bad(f"does not start with the PNG signature ({len(data)} bytes)")

    offset = 8
    chunks: list[tuple[str, int]] = []
    idat = b""
    header: tuple[int, int, int, int] | None = None

    while offset + 12 <= len(data):
        (length,) = struct.unpack(">I", data[offset : offset + 4])
        ctype = data[offset + 4 : offset + 8]
        end = offset + 8 + length
        if end + 4 > len(data):
            raise Bad(f"chunk {ctype!r} claims {length} bytes, past the end of the file")
        body = data[offset + 8 : end]
        (stored,) = struct.unpack(">I", data[end : end + 4])
        actual = binascii.crc32(ctype + body) & 0xFFFFFFFF
        if stored != actual:
            raise Bad(f"CRC mismatch in {ctype.decode(errors='replace')}")
        chunks.append((ctype.decode(errors="replace"), length))

        if ctype == b"IHDR":
            width, height, depth, colour, _comp, _filter, interlace = struct.unpack(
                ">IIBBBBB", body
            )
            if interlace:
                raise Bad("interlaced; this checker does not model that layout")
            if colour not in CHANNELS:
                raise Bad(f"colour type {colour} is not one the spec defines")
            header = (width, height, depth, colour)
        elif ctype == b"IDAT":
            idat += body

        offset = end + 4

    if header is None:
        raise Bad("no IHDR")
    if not chunks or chunks[-1][0] != "IEND":
        raise Bad("does not end with IEND")

    width, height, depth, colour = header
    if expect is not None and (width, height) != expect:
        raise Bad(f"{width}x{height}, expected {expect[0]}x{expect[1]}")

    try:
        raw = zlib.decompress(idat)
    except zlib.error as exc:
        raise Bad(f"pixel data does not decompress: {exc}") from exc

    # One filter byte per row, then the row itself.
    wanted = height * (1 + width * CHANNELS[colour] * depth // 8)
    if len(raw) != wanted:
        raise Bad(f"pixel data is {len(raw)} bytes, the header implies {wanted}")

    return [
        f"{path.name}: {width}x{height}, depth {depth}, colour type {colour}",
        f"  {len(data)} bytes, {len(chunks)} chunks, every CRC checks out",
        f"  pixel data inflates to {len(raw)} bytes, exactly what the header implies",
    ]


def parse_size(text: str) -> tuple[int, int]:
    try:
        w, h = text.lower().split("x")
        return int(w), int(h)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"not a size like 720x400: {text}") from exc


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("files", type=Path, nargs="+")
    ap.add_argument(
        "--expect",
        type=parse_size,
        default=None,
        help="required dimensions, e.g. 720x400",
    )
    args = ap.parse_args()

    failed = 0
    for path in args.files:
        if not path.exists():
            print(f"[E_PNG_001_MISSING] not found: {path}", file=sys.stderr)
            failed += 1
            continue
        try:
            for line in check(path, args.expect):
                print(line)
        except Bad as exc:
            print(f"[E_PNG_010_MALFORMED] {path.name}: {exc}", file=sys.stderr)
            failed += 1
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
