#!/usr/bin/env bash
# The path gate, with the evidence that it works.
#
# A gate that has never failed proves nothing, so this runs it four ways:
#
#   1. planted:  each build-machine mark, in ASCII and UTF-16, is planted into a copy
#                of the built kernel; every one must be caught.
#   2. real:     the kernel is built once more with `trim-paths` switched off -- the
#                build that leaked a `.rustup` path into the ISO -- and the gate must
#                reject it. This is the incident itself, reproduced on purpose.
#   3. debug:    a debug build, with full DWARF (compile directories, file tables,
#                `global_asm!`, the linker's output), must come out clean too.
#   4. shipped:  the release kernel ELF, the Limine ELF and the ISO must be clean.
#
#   bash scripts/path-gate-check.sh [file ...]
#
# With no files, step 4 scans the default outputs of the CI build and of
# `scripts/build-iso.sh`; each one must exist.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

GATE=(python3 -I scripts/path-leak-gate.py)
KERNEL="target/x86_64-zpl-kernel/release/zpl-kernel-bin"
BUILD=(cargo -Zbuild-std=core,compiler_builtins,alloc
  -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec
  build -p zpl-kernel-bin --target crates/zpl-kernel/x86_64-zpl-kernel.json)

if (( $# > 0 )); then
  SHIPPED=("$@")
else
  SHIPPED=(
    "$KERNEL"
    target/x86_64-zpl-limine/release/limine-bridge
    artifacts/iso/zpl-kernel.iso
  )
fi
for f in "$KERNEL" "${SHIPPED[@]}"; do
  if [[ ! -f "$f" ]]; then
    echo "[E_PATH_CHECK_001_MISSING] $f is missing; build it first." >&2
    exit 2
  fi
done

echo "==> 1. planted strings"
"${GATE[@]}" --self-test "$KERNEL"

echo "==> 2. a real leak: the kernel built with trim-paths off"
# Only meaningful where the toolchain itself lives under a marked path, which is
# the normal rustup layout (~/.rustup) and the CI runner's. Elsewhere there is no
# path for the control build to leak, and it says so instead of claiming a result.
sysroot="$(rustc --print sysroot)"
if python3 -I - "$sysroot" <<'PY'
import importlib.util
import sys
spec = importlib.util.spec_from_file_location("gate", "scripts/path-leak-gate.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
sys.exit(0 if gate.scan(sys.argv[1].encode()) else 1)
PY
then
  "${BUILD[@]}" --release --target-dir target/path-gate-control \
    --config 'profile.release.trim-paths="none"' 2>&1 | tail -n 1
  control="target/path-gate-control/x86_64-zpl-kernel/release/zpl-kernel-bin"
  if "${GATE[@]}" "$control" >/dev/null 2>&1; then
    echo "[E_PATH_CHECK_002_BLIND] the kernel built without trim-paths passed the gate;" \
      "the gate cannot see the leak it exists for" >&2
    exit 1
  fi
  echo "  rejected, as it must be:"
  "${GATE[@]}" "$control" 2>&1 | grep -F E_PATH_GATE_002 | head -n 2 \
    | sed -e 's/^/    /' -e "s#${HOME}#<home>#g" || true
else
  echo "  skipped: the toolchain at the sysroot is not under a marked path, so a" \
    "build without trim-paths has nothing of this machine's to leak"
fi

echo "==> 3. a debug build, full DWARF"
"${BUILD[@]}" --target-dir target/path-gate-debug 2>&1 | tail -n 1
"${GATE[@]}" target/path-gate-debug/x86_64-zpl-kernel/debug/zpl-kernel-bin

echo "==> 4. what ships"
"${GATE[@]}" "${SHIPPED[@]}"

echo "path gate check PASS"
