#!/usr/bin/env python3
"""Read a GIF back: structure, frames, and (with --pixels) the decoded image data.

The counterpart to `make_gif.py`. A GIF that a browser refuses to animate looks
exactly like one it accepts until something reads the blocks, so this exists to
answer "what is actually in the file" instead of guessing from the rendering.

    python tools/img/read_gif.py out.gif
    python tools/img/read_gif.py out.gif --pixels
"""
import struct
import sys

EXTENSION = 0x21
IMAGE_SEPARATOR = 0x2C
TRAILER = 0x3B
GRAPHIC_CONTROL = 0xF9


class Malformed(Exception):
    pass


def read_blocks(data, pos):
    """Read a chain of sub-blocks; returns (payload, position after the terminator)."""
    out = bytearray()
    while True:
        if pos >= len(data):
            raise Malformed("sub-block chain runs off the end of the file")
        n = data[pos]
        pos += 1
        if n == 0:
            return bytes(out), pos
        out += data[pos:pos + n]
        pos += n


def lzw_decode(data, min_code_size, expected):
    """Decode one image's LZW stream into palette indices."""
    clear = 1 << min_code_size
    end = clear + 1
    code_size = min_code_size + 1
    table = [bytes([i]) for i in range(clear)] + [b"", b""]
    out = bytearray()

    bits = 0
    nbits = 0
    prev = None
    pos = 0
    while True:
        while nbits < code_size:
            if pos >= len(data):
                if prev is None and not out:
                    raise Malformed("empty LZW stream")
                return bytes(out), "ran out of codes before the end marker"
            bits |= data[pos] << nbits
            nbits += 8
            pos += 1
        code = bits & ((1 << code_size) - 1)
        bits >>= code_size
        nbits -= code_size

        if code == clear:
            table = [bytes([i]) for i in range(clear)] + [b"", b""]
            code_size = min_code_size + 1
            prev = None
            continue
        if code == end:
            return bytes(out), None
        if code < len(table):
            entry = table[code]
        elif code == len(table) and prev is not None:
            entry = prev + prev[:1]
        else:
            return bytes(out), "code %d is past the table (%d entries)" % (code, len(table))
        out += entry
        if prev is not None:
            table.append(prev + entry[:1])
            if len(table) == (1 << code_size) and code_size < 12:
                code_size += 1
        prev = entry
        if len(out) > expected * 2:
            return bytes(out), "stream produced far more pixels than the frame holds"


def main():
    path = sys.argv[1]
    want_pixels = "--pixels" in sys.argv
    data = open(path, "rb").read()
    if data[:6] not in (b"GIF87a", b"GIF89a"):
        raise Malformed("not a GIF: %r" % data[:6])
    width, height = struct.unpack("<HH", data[6:10])
    packed = data[10]
    pos = 13
    table_size = 0
    if packed & 0x80:
        table_size = 2 << (packed & 7)
        pos += 3 * table_size
    print("%s: %dx%d, global table %d colours" % (path, width, height, table_size))

    frames = 0
    problems = 0
    delay = None
    while pos < len(data):
        marker = data[pos]
        if marker == TRAILER:
            pos += 1
            break
        if marker == EXTENSION:
            label = data[pos + 1]
            pos += 2
            body, pos = read_blocks(data, pos)
            if label == GRAPHIC_CONTROL:
                delay = struct.unpack("<H", body[1:3])[0]
            continue
        if marker != IMAGE_SEPARATOR:
            raise Malformed("byte %#x at offset %d is not a block marker" % (marker, pos))
        x, y, w, h = struct.unpack("<HHHH", data[pos + 1:pos + 9])
        local = data[pos + 9]
        pos += 10
        if local & 0x80:
            pos += 3 * (2 << (local & 7))
        min_code_size = data[pos]
        pos += 1
        stream, pos = read_blocks(data, pos)
        frames += 1
        note = ""
        if want_pixels:
            pixels, err = lzw_decode(stream, min_code_size, w * h)
            if err:
                note = "  PROBLEM: " + err
                problems += 1
            elif len(pixels) != w * h:
                note = "  PROBLEM: decoded %d pixels, frame holds %d" % (len(pixels), w * h)
                problems += 1
            else:
                note = "  %d pixels OK" % len(pixels)
        print("  frame %2d  at %4d,%-4d  %4dx%-4d  delay %s  %5d bytes of LZW%s"
              % (frames, x, y, w, h, delay, len(stream), note))

    print("%d frames, %d with problems" % (frames, problems))
    return 1 if problems else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Malformed as exc:
        print("malformed: %s" % exc)
        sys.exit(1)
