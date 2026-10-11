#!/usr/bin/env python3
"""Fail if a built file carries a path from the machine that built it.

Why this exists: a Windows account's `.rustup` folder once travelled inside every
public ISO. A `panic!` in the standard library records the file it came from, and
with `-Zbuild-std` that file is on the build machine. No gate saw it -- the source
scan reads source files, and this was a string inside a compiled binary.

`trim-paths = "all"` in the workspace `Cargo.toml` stops it at compile time. This
is the check that it stays stopped: it reads the *built bytes* -- the kernel ELF,
the ISO, anything else it is given -- and looks for the marks of a build machine,
both as plain ASCII and as UTF-16 (little- and big-endian), because PE/EFI files
store strings that way and an ASCII-only scan would walk past them.

    python3 scripts/path-leak-gate.py FILE...      scan; exit 1 on any hit
    python3 scripts/path-leak-gate.py --self-test [FILE]
                                                   prove the scan can fail: plant
                                                   each mark, in each encoding, into
                                                   a copy of FILE (or of a blank
                                                   buffer) and require a hit

Matching ignores case: Windows paths do too. A hit prints the file, the offset,
the encoding and a short readable excerpt, so the log says what leaked and where.
"""

import sys

# The marks of a build machine. Each is something no file in the image has a
# reason to contain: a home directory on Linux, root's home, a Windows profile
# (both slash directions), and the two tool homes Rust installs into them.
PATTERNS = [
    "/home/",
    "/root/",
    "C:\\Users\\",
    "C:/Users/",
    ".rustup",
    ".cargo",
]

ENCODINGS = ["ascii", "utf-16-le", "utf-16-be"]


def needles():
    for pattern in PATTERNS:
        for enc in ENCODINGS:
            yield pattern, enc, pattern.lower().encode(enc)


def lower_ascii(data):
    """Lower-case ASCII letters only, leaving every other byte as it was, so a
    UTF-16 string keeps its zero bytes and offsets stay the same."""
    return data.translate(bytes.maketrans(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",
                                          b"abcdefghijklmnopqrstuvwxyz"))


def excerpt(data, at, enc):
    width = 48 if enc == "ascii" else 96
    raw = data[max(0, at - width // 2): at + width]
    if enc != "ascii":
        raw = raw.replace(b"\x00", b"")
    return "".join(chr(b) if 32 <= b < 127 else "." for b in raw)


def scan(data):
    """Every (pattern, encoding, offset) found in `data`, first hit of each."""
    low = lower_ascii(data)
    hits = []
    for pattern, enc, needle in needles():
        at = low.find(needle)
        if at >= 0:
            hits.append((pattern, enc, at))
    return hits


def scan_files(paths):
    failed = False
    for path in paths:
        try:
            with open(path, "rb") as f:
                data = f.read()
        except OSError as e:
            print(f"[E_PATH_GATE_001_UNREADABLE] {path}: {e}", file=sys.stderr)
            failed = True
            continue
        hits = scan(data)
        if not hits:
            print(f"  clean  {path} ({len(data)} bytes)")
            continue
        failed = True
        for pattern, enc, at in hits:
            print(f"[E_PATH_GATE_002_LEAK] {path}: {pattern!r} as {enc} at "
                  f"offset {at:#x}: ...{excerpt(data, at, enc)}...", file=sys.stderr)
    return failed


def self_test(base_path):
    if base_path:
        with open(base_path, "rb") as f:
            base = f.read()
        if scan(base):
            print(f"[E_PATH_GATE_010_BASE_DIRTY] {base_path} is not clean to begin "
                  "with; scan it first", file=sys.stderr)
            return False
    else:
        base = bytes(range(32, 127)) * 64
    if scan(base):
        print("[E_PATH_GATE_011_BLANK_DIRTY] the blank buffer matched", file=sys.stderr)
        return False

    ok = True
    tried = 0
    for pattern in PATTERNS:
        for enc in ENCODINGS:
            # Planted the way a real leak looks: inside a longer path, in the
            # middle of the file, with a case the pattern list does not use.
            planted = ("x" + pattern.swapcase() + "someone/project/src/lib.rs").encode(enc)
            mid = len(base) // 2
            copy = base[:mid] + planted + base[mid:]
            tried += 1
            if not any(p == pattern for p, _, _ in scan(copy)):
                print(f"[E_PATH_GATE_012_MISSED] planted {pattern!r} as {enc} and "
                      "the scan did not find it", file=sys.stderr)
                ok = False
    if ok:
        print(f"path gate self-test PASS: {tried} planted strings, {tried} caught; "
              f"the unplanted {'file' if base_path else 'buffer'} stays clean")
    return ok


def main(argv):
    if len(argv) >= 2 and argv[1] == "--self-test":
        return 0 if self_test(argv[2] if len(argv) > 2 else None) else 1
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    print(f"==> scanning {len(argv) - 1} built file(s) for build-machine paths")
    if scan_files(argv[1:]):
        print("path gate FAIL", file=sys.stderr)
        return 1
    print("path gate PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
