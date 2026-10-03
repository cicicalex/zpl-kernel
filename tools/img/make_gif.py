#!/usr/bin/env python3
"""Build an animated GIF out of PNG frames, with nothing but the standard library.

Why this exists: `docs/public-drafts/media/boot-demo.gif` was committed with no way
to make another one. When the panel grew a row the GIF became the only picture in
the README that could not be brought up to date, which is a poor reason to leave a
stale image in front of readers. Adding an image library would have been a new
dependency; this is about two hundred lines instead.

Scope is deliberately narrow, and the code says so rather than failing obscurely:
8-bit non-interlaced PNGs, colour type 2 (RGB) or 6 (RGBA), every frame the same
size, and at most 256 distinct colours across the whole set -- which is what a VGA
text screen gives you. Anything else is refused with a message naming the file.

    python tools/img/make_gif.py out.gif 6 frame-*.png

The `6` is the delay between frames in hundredths of a second, as GIF counts.
"""
import glob
import struct
import sys
import zlib

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


class Unsupported(Exception):
    """A PNG this script does not claim to read."""


def read_png(path):
    """Return (width, height, rows) where rows is a list of bytes, 3 per pixel."""
    data = open(path, "rb").read()
    if not data.startswith(PNG_MAGIC):
        raise Unsupported("%s: not a PNG" % path)

    pos = len(PNG_MAGIC)
    width = height = None
    channels = None
    idat = bytearray()
    while pos + 8 <= len(data):
        (length,) = struct.unpack(">I", data[pos:pos + 4])
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, colour, _, _, interlace = struct.unpack(">IIBBBBB", body)
            if depth != 8:
                raise Unsupported("%s: %d bits per channel, only 8 is handled" % (path, depth))
            if interlace:
                raise Unsupported("%s: interlaced" % path)
            if colour == 2:
                channels = 3
            elif colour == 6:
                channels = 4
            else:
                raise Unsupported("%s: colour type %d, only 2 and 6 are handled" % (path, colour))
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break

    if width is None or channels is None:
        raise Unsupported("%s: no IHDR" % path)

    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    if len(raw) != (stride + 1) * height:
        raise Unsupported("%s: %d bytes of pixel data, expected %d"
                          % (path, len(raw), (stride + 1) * height))

    rows = []
    prev = bytearray(stride)
    at = 0
    for _ in range(height):
        filt = raw[at]
        line = bytearray(raw[at + 1:at + 1 + stride])
        at += 1 + stride
        unfilter(filt, line, prev, channels)
        # The next line's filter refers to this one *before* alpha is dropped, so
        # `prev` keeps the original channel count and the RGB copy is taken from it.
        prev = line
        if channels == 4:
            # Drop alpha: a screendump is opaque, and GIF's only transparency is one
            # fully transparent palette index, which is not what this is for.
            rows.append(bytes(b for i, b in enumerate(line) if i % 4 != 3))
        else:
            rows.append(bytes(line))
    return width, height, rows


def unfilter(filt, line, prev, channels):
    """Undo one PNG scanline filter in place. `line` and `prev` are bytearrays."""
    bpp = channels
    if filt == 0:
        return
    if filt == 1:
        for i in range(bpp, len(line)):
            line[i] = (line[i] + line[i - bpp]) & 0xFF
    elif filt == 2:
        for i in range(len(line)):
            line[i] = (line[i] + prev[i]) & 0xFF
    elif filt == 3:
        for i in range(len(line)):
            left = line[i - bpp] if i >= bpp else 0
            line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
    elif filt == 4:
        for i in range(len(line)):
            a = line[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
            line[i] = (line[i] + pred) & 0xFF
    else:
        raise Unsupported("scanline filter %d" % filt)


def build_palette(frames):
    """One shared palette for every frame, or a clear refusal."""
    seen = {}
    for path, (_, _, rows) in frames:
        for row in rows:
            for i in range(0, len(row), 3):
                colour = row[i:i + 3]
                if colour not in seen:
                    if len(seen) == 256:
                        raise Unsupported(
                            "%s: more than 256 distinct colours across the frames; "
                            "this script does no quantisation" % path)
                    seen[colour] = len(seen)
    return seen


def index_frame(rows, palette):
    out = bytearray()
    for row in rows:
        for i in range(0, len(row), 3):
            out.append(palette[row[i:i + 3]])
    return bytes(out)


def lzw_encode(indexed, min_code_size):
    """Standard GIF LZW. Emits a clear code whenever the table fills."""
    clear = 1 << min_code_size
    end = clear + 1
    code_size = min_code_size + 1
    table = {bytes([i]): i for i in range(clear)}
    next_code = end + 1

    bits = 0
    nbits = 0
    out = bytearray()

    def emit(code):
        nonlocal bits, nbits
        bits |= code << nbits
        nbits += code_size
        while nbits >= 8:
            out.append(bits & 0xFF)
            bits >>= 8
            nbits -= 8

    emit(clear)
    prefix = b""
    for byte in indexed:
        nxt = prefix + bytes([byte])
        if nxt in table:
            prefix = nxt
            continue
        emit(table[prefix])
        table[nxt] = next_code
        next_code += 1
        if next_code > (1 << code_size):
            if code_size < 12:
                code_size += 1
            else:
                emit(clear)
                table = {bytes([i]): i for i in range(clear)}
                next_code = end + 1
                code_size = min_code_size + 1
        prefix = bytes([byte])
    if prefix:
        emit(table[prefix])
    emit(end)
    if nbits:
        out.append(bits & 0xFF)
    return bytes(out)


def blocks(data):
    """Chop LZW output into GIF's sub-blocks of at most 255 bytes."""
    out = bytearray()
    for i in range(0, len(data), 255):
        chunk = data[i:i + 255]
        out.append(len(chunk))
        out += chunk
    out.append(0)
    return bytes(out)


# GIF block markers, as numbers. Written this way on purpose: the first draft spelled
# them as escaped byte strings, and a shell heredoc ate the backslashes, leaving a file
# with raw control bytes in the middle of a literal. Numbers cannot be mangled.
EXTENSION = 0x21
GRAPHIC_CONTROL = 0xF9
APPLICATION = 0xFF
IMAGE_SEPARATOR = 0x2C
TRAILER = 0x3B
# Disposal method 1: leave the frame on screen, so the next partial frame paints over it.
DISPOSE_KEEP = 1


def graphic_control(delay):
    """The per-frame control block: how long to hold it, and what to do afterwards."""
    return bytes([EXTENSION, GRAPHIC_CONTROL, 4, DISPOSE_KEEP << 2]) + \
        struct.pack("<H", delay) + bytes([0, 0])


def image_header(x, y, w, h):
    """An image descriptor with no local colour table and no interlace."""
    return bytes([IMAGE_SEPARATOR]) + struct.pack("<HHHH", x, y, w, h) + bytes([0])


def netscape_loop():
    """The extension every browser reads as "repeat for ever"."""
    return (bytes([EXTENSION, APPLICATION, 11]) + b"NETSCAPE2.0"
            + bytes([3, 1, 0, 0, 0]))


def write_gif(path, width, height, palette, indexed_frames, delay):
    size = 1
    while (1 << size) < max(len(palette), 2):
        size += 1
    table = bytearray()
    for colour, _ in sorted(palette.items(), key=lambda kv: kv[1]):
        table += colour
    table += bytes(3 * ((1 << size) - len(palette)))

    out = bytearray(b"GIF89a")
    out += struct.pack("<HH", width, height)
    # Global colour table present, 8-bit colour resolution, table size in the low bits.
    out.append(0xF0 | (size - 1))
    out += bytes([0, 0])
    out += table
    out += netscape_loop()

    min_code_size = max(size, 2)
    previous = None
    for indexed in indexed_frames:
        # Only the part of the screen that changed. Consecutive frames of a boot log
        # differ by a line or two, so writing the whole 720x400 every time cost three
        # times the file for nothing.
        box = changed_box(previous, indexed, width, height)
        previous = indexed
        if box is None:
            # Nothing moved. Hold the picture for another interval with a one-pixel
            # image rather than repainting every pixel with itself.
            out += graphic_control(delay) + image_header(0, 0, 1, 1)
            out.append(min_code_size)
            out += blocks(lzw_encode(indexed[:1], min_code_size))
            continue
        x0, y0, x1, y1 = box
        sub = bytearray()
        for y in range(y0, y1):
            sub += indexed[y * width + x0:y * width + x1]
        out += graphic_control(delay) + image_header(x0, y0, x1 - x0, y1 - y0)
        out.append(min_code_size)
        out += blocks(lzw_encode(bytes(sub), min_code_size))
    out.append(TRAILER)
    open(path, "wb").write(bytes(out))
    return len(out)



def changed_box(previous, current, width, height):
    """Smallest rectangle covering every pixel that differs, or None if none do."""
    if previous is None:
        return (0, 0, width, height)
    if previous == current:
        return None
    y0 = y1 = None
    for y in range(height):
        a = y * width
        if previous[a:a + width] != current[a:a + width]:
            if y0 is None:
                y0 = y
            y1 = y + 1
    x0, x1 = width, 0
    for y in range(y0, y1):
        a = y * width
        row_p = previous[a:a + width]
        row_c = current[a:a + width]
        for x in range(width):
            if row_p[x] != row_c[x]:
                if x < x0:
                    x0 = x
                if x + 1 > x1:
                    x1 = x + 1
    return (x0, y0, x1, y1)


def main():
    if len(sys.argv) < 4:
        print(__doc__)
        return 2
    out_path = sys.argv[1]
    delay = int(sys.argv[2])
    paths = []
    for pattern in sys.argv[3:]:
        matched = sorted(glob.glob(pattern))
        paths.extend(matched if matched else [pattern])
    if not paths:
        print("no frames matched")
        return 1

    frames = []
    for p in paths:
        frames.append((p, read_png(p)))

    width, height, _ = frames[0][1]
    for p, (w, h, _) in frames:
        if (w, h) != (width, height):
            raise Unsupported("%s is %dx%d, the first frame is %dx%d" % (p, w, h, width, height))

    palette = build_palette(frames)
    indexed = [index_frame(rows, palette) for _, (_, _, rows) in frames]
    size = write_gif(out_path, width, height, palette, indexed, delay)
    print("%s: %d frames, %dx%d, %d colours, %d bytes"
          % (out_path, len(frames), width, height, len(palette), size))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Unsupported as exc:
        print("refused: %s" % exc)
        sys.exit(1)
