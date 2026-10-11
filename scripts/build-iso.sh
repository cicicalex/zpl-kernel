#!/usr/bin/env bash
# Build the Limine hybrid BIOS/UEFI ISO on Linux, reproducibly.
#
# The same image `scripts/build-iso.ps1` makes on Windows -- same pinned Limine
# release and checksum, same staging layout, same `limine.conf`, same xorriso
# arguments -- so CI can build the ISO that is published, and check its bytes.
#
# Reproducible: every timestamp that ends up in the image is taken from
# SOURCE_DATE_EPOCH, which defaults to the time of the commit being built. Two
# builds of one commit, from any directory, give the same SHA-256; see
# `scripts/repro-check.sh` and docs/REPRODUCIBLE_BUILD.md.
#
#   bash scripts/build-iso.sh [output.iso]
#
# Needs: the nightly toolchain from rust-toolchain.toml, xorriso, a C compiler (for
# Limine's host tool, built from the `limine.c` that ships in the release zip),
# python3, and network access the first time, to fetch the Limine zip.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Pinned, and kept equal to the values in scripts/build-iso.ps1.
LIMINE_TAG="v12.2.0"
LIMINE_ZIP_SHA256="6a7d1daaa8fd336dd4dc477db1d2533dc070b9f5cba36926a9be9e3fbb438635"
LIMINE_URL="https://github.com/Limine-Bootloader/Limine/releases/download/${LIMINE_TAG}/limine-binary.zip"

OUT="${1:-artifacts/iso/zpl-kernel.iso}"
DIST="artifacts/iso/limine-dist/${LIMINE_TAG}"
STAGING="artifacts/iso/_staging_limine_iso_sh"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"

for tool in xorriso cc python3 sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "[E_ISO_SH_001_TOOL] $tool is not on PATH." >&2
    exit 2
  fi
done

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
fi
export SOURCE_DATE_EPOCH
echo "==> SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}"

# --- Limine ----------------------------------------------------------------
mkdir -p "$DIST"
ZIP="$DIST/limine-binary.zip"
if [[ ! -f "$ZIP" ]]; then
  echo "==> downloading Limine ${LIMINE_TAG}"
  curl -fsSL -o "$ZIP.part" "$LIMINE_URL"
  mv "$ZIP.part" "$ZIP"
fi
got="$(sha256sum "$ZIP" | cut -d' ' -f1)"
if [[ "$got" != "$LIMINE_ZIP_SHA256" ]]; then
  echo "[E_ISO_SH_002_SHA256] Limine zip: expected ${LIMINE_ZIP_SHA256}, got ${got}" >&2
  exit 1
fi
LIM="$DIST/limine-binary"
if [[ ! -f "$LIM/limine.c" ]]; then
  python3 -I -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' \
    "$ZIP" "$DIST"
fi
if [[ ! -x "$LIM/limine" ]]; then
  echo "==> building Limine's host tool"
  cc -O2 -std=c99 -o "$LIM/limine" "$LIM/limine.c"
fi

# --- kernel ----------------------------------------------------------------
echo "==> building limine-bridge (--release)"
cargo -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec \
  build -p limine-bridge --target limine-bridge/x86_64-zpl-limine.json --release
ELF="$TARGET_DIR/x86_64-zpl-limine/release/limine-bridge"
if [[ ! -f "$ELF" ]]; then
  echo "[E_ISO_SH_003_NO_ELF] expected $ELF" >&2
  exit 1
fi

# --- staging ---------------------------------------------------------------
rm -rf "$STAGING"
mkdir -p "$STAGING/boot/limine" "$STAGING/EFI/BOOT"
cp "$ELF" "$STAGING/boot/zpl-kernel.elf"
cp "$LIM/limine-bios.sys" "$LIM/limine-bios-cd.bin" "$LIM/limine-uefi-cd.bin" \
  "$STAGING/boot/limine/"
cp "$LIM/BOOTX64.EFI" "$STAGING/EFI/BOOT/"
# Byte for byte the file build-iso.ps1 writes, line endings included.
{
  printf 'timeout: 0\n'
  printf 'default_entry: 1\n'
  printf 'verbose: yes\n'
  printf 'serial: yes\n'
  printf '\n'
  printf '/ZPL Kernel (%s)\n' "$LIMINE_TAG"
  printf '    protocol: limine\n'
  printf '    path: boot():/boot/zpl-kernel.elf\n'
} > "$STAGING/boot/limine/limine.conf"
# Every file and directory dated the same, so the copy time does not reach the image.
find "$STAGING" -exec touch -h -d "@${SOURCE_DATE_EPOCH}" {} +

# --- image -----------------------------------------------------------------
mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
echo "==> xorriso"
xorriso -as mkisofs \
  -R -r -J \
  -b boot/limine/limine-bios-cd.bin \
  -no-emul-boot -boot-load-size 4 -boot-info-table \
  -hfsplus -apm-block-size 2048 \
  --efi-boot boot/limine/limine-uefi-cd.bin \
  -efi-boot-part --efi-boot-image \
  --protective-msdos-label \
  "$STAGING" -o "$OUT" 2>&1 | grep -v '^xorriso : UPDATE' || true
if [[ ! -s "$OUT" ]]; then
  echo "[E_ISO_SH_004_XORRISO] no image written" >&2
  exit 1
fi
"$LIM/limine" bios-install "$OUT"

# `limine bios-install` gives the MBR a disk ID seeded from the clock
# (`srand(time(NULL))` in limine.c), so two builds a second apart differ in exactly
# these four bytes at 0x1B8 and nowhere else. The ID only has to tell disks apart;
# derive it from the image itself -- the SHA-256 of the image with the ID zeroed --
# so it is stable for one commit and still changes when anything else does.
python3 -I - "$OUT" <<'PY'
import hashlib, sys
path = sys.argv[1]
with open(path, "r+b") as f:
    data = bytearray(f.read())
    data[0x1B8:0x1BC] = b"\0\0\0\0"
    disk_id = hashlib.sha256(data).digest()[:4]
    if disk_id == b"\0\0\0\0":
        disk_id = b"\0\0\0\1"
    f.seek(0x1B8)
    f.write(disk_id)
PY

sha="$(sha256sum "$OUT" | cut -d' ' -f1)"
echo "$sha  $(basename "$OUT")" > "$OUT.sha256"
echo "==> $OUT"
echo "    size   $(stat -c %s "$OUT") bytes"
echo "    sha256 $sha"
