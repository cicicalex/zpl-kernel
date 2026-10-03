"""Measure the shape of the code: file sizes, long functions, `unsafe`, module cycles.

A figure you cannot reproduce with a command is a memory, not a measurement, so the
figures this project quotes about its own shape come from here rather than from
someone having counted once.

    python scripts/arch-metrics.py            # the full report
    python scripts/arch-metrics.py --check    # exit 1 if a limit is exceeded

`--check` is what a gate would call. It is deliberately *not* wired into
`full-verify.ps1` yet: the limits below are the ones this kernel is working towards,
and several are exceeded today. A gate that is red from the day it is installed stops
being read, so it goes in once the numbers are under the line.
"""
import os
import re
import sys

ROOTS = ("crates", "limine-bridge")

# The limits the plan is working towards. See ARHITECTURA_ZPL.md section 5, step 5.
MAX_FILE_LINES = 800
MAX_FN_LINES = 150


def rust_files():
    for root in ROOTS:
        for d, _, files in os.walk(root):
            if "target" in d.split(os.sep):
                continue
            for f in files:
                if f.endswith(".rs"):
                    yield os.path.join(d, f).replace(os.sep, "/")


def read(path):
    """Lines, counted the way `wc -l` counts them.

    A file that ends with a newline splits into a trailing empty string, which would
    make every count one too high and every figure in the audit disagree with the
    obvious command. Dropping it is what makes the two match.
    """
    with open(path, encoding="utf-8", errors="replace") as fh:
        lines = fh.read().split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    return lines


def tests_start(lines):
    """Line index where `mod tests` begins, or None.

    Everything from there to the end of the file is counted as test code. That is a
    simplification -- a file could have code after its test module -- and none here do.
    """
    for i, line in enumerate(lines):
        if line.strip().startswith("mod tests") and "{" in line:
            return i
    return None


def long_functions(path, lines):
    """Functions longer than MAX_FN_LINES, by brace depth.

    Not a parser. It finds `fn name(` at any indentation, then follows `{` and `}`
    counts until the depth returns to where the function started. Strings containing
    braces would fool it; none in this tree do, and the alternative was a dependency.
    """
    out = []
    depth = 0
    start = None
    name = None
    fn_depth = 0
    for i, line in enumerate(lines):
        m = re.match(
            r'^\s*(pub(\([a-z: ]+\))?\s+)?'
            r'(const\s+|async\s+|unsafe\s+|extern\s+"[A-Za-z]+"\s+)*'
            r'fn\s+([A-Za-z0-9_]+)',
            line,
        )
        if m and start is None:
            start, name, fn_depth = i, m.group(4), depth
        depth += line.count("{") - line.count("}")
        if start is not None and depth <= fn_depth and line.count("}") > 0:
            n = i - start + 1
            if n > MAX_FN_LINES:
                out.append((n, path, start + 1, name))
            start = None
    return out


def unsafe_blocks(path, lines):
    """`unsafe { ... }` blocks, and whether a SAFETY note is within eight lines above.

    Eight because that is how far a doc comment plus a blank line reaches in this tree.
    `unsafe fn` is not counted: declaring a function unsafe is a signature, not a use.
    """
    total = 0
    documented = 0
    bare = []
    for i, line in enumerate(lines):
        if re.search(r"(^|[^a-zA-Z_])unsafe\s*\{", line):
            total += 1
            window = "\n".join(lines[max(0, i - 8):i + 1])
            if "SAFETY" in window:
                documented += 1
            else:
                bare.append((path, i + 1, line.strip()[:70]))
    return total, documented, bare


def module_graph():
    """Which module of `zpl-kernel` names which other, from `crate::` paths."""
    src = os.path.join("crates", "zpl-kernel", "src")
    mods = {}
    for d, _, files in os.walk(src):
        for f in files:
            if f.endswith(".rs"):
                mods[f[:-3]] = os.path.join(d, f)
    edges = {}
    for name, path in mods.items():
        # Comments do not create a dependency. A module header that explains why it was
        # split out of another one names that other one, and counting it reported a cycle
        # between two modules that only prose connects.
        text = "\n".join(
            l for l in read(path) if not l.lstrip().startswith(("//", "*"))
        )
        targets = set()
        # `crate::` followed by any path; the module is the last component that is one.
        # Written this way so adding a directory does not silently stop the measurement
        # from seeing through it -- which is exactly what happened when the modules were
        # first grouped: the pairs appeared to drop from twelve to two, and none of them
        # had actually been broken.
        for m in re.finditer(r"crate::((?:[a-z0-9_]+::)*[a-z0-9_]+)", text):
            for part in reversed(m.group(1).split("::")):
                if part in mods:
                    if part != name:
                        targets.add(part)
                    break
        edges[name] = targets
    cycles = set()
    for a, outs in edges.items():
        for b in outs:
            if a in edges.get(b, ()):
                cycles.add(tuple(sorted((a, b))))
    return mods, edges, sorted(cycles)


def main():
    check = "--check" in sys.argv
    files = sorted(rust_files())

    sizes = []
    long_fns = []
    unsafe_total = unsafe_doc = 0
    unsafe_per_file = {}
    bare_all = []
    code_lines = test_lines = 0

    for p in files:
        lines = read(p)
        sizes.append((len(lines), p))
        long_fns.extend(long_functions(p, lines))
        t, d, bare = unsafe_blocks(p, lines)
        unsafe_total += t
        unsafe_doc += d
        bare_all.extend(bare)
        if t:
            unsafe_per_file[p] = t
        idx = tests_start(lines)
        tl = (len(lines) - idx) if idx is not None else 0
        test_lines += tl
        code_lines += len(lines) - tl

    sizes.sort(reverse=True)
    long_fns.sort(reverse=True)
    mods, edges, cycles = module_graph()

    print("== the fifteen largest files ==")
    for n, p in sizes[:15]:
        flag = "  OVER" if n > MAX_FILE_LINES else ""
        print("  %5d  %s%s" % (n, p, flag))

    print()
    print("== functions over %d lines: %d ==" % (MAX_FN_LINES, len(long_fns)))
    for n, p, ln, name in long_fns:
        print("  %5d  %s:%d  %s" % (n, p, ln, name))

    print()
    print("== unsafe ==")
    print("  blocks: %d   with a SAFETY note above: %d   without: %d"
          % (unsafe_total, unsafe_doc, unsafe_total - unsafe_doc))
    for p, n in sorted(unsafe_per_file.items(), key=lambda kv: -kv[1])[:10]:
        print("  %4d  %s" % (n, p))

    print()
    print("== modules of zpl-kernel ==")
    print("  modules: %d   edges: %d   mutually dependent pairs: %d"
          % (len(mods), sum(len(v) for v in edges.values()), len(cycles)))
    for a, b in cycles:
        print("    %s <-> %s" % (a, b))

    print()
    print("== code against tests ==")
    total = code_lines + test_lines
    print("  code %d   tests %d   tests are %.0f%% of %d"
          % (code_lines, test_lines, 100.0 * test_lines / max(1, total), total))

    if check:
        over_files = [(n, p) for n, p in sizes if n > MAX_FILE_LINES]
        problems = len(over_files) + len(long_fns)
        print()
        if problems:
            print("arch-metrics: %d over the limits (%d files, %d functions)"
                  % (problems, len(over_files), len(long_fns)))
            return 1
        print("arch-metrics: within the limits")
    return 0


if __name__ == "__main__":
    sys.exit(main())
