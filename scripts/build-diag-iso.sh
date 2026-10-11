#!/usr/bin/env bash
# Build the hardware diagnostic ISO: `limine-bridge --features hw_diag` behind Limine,
# as a hybrid image that boots from a CD or, written raw (Rufus "DD" mode), from a USB
# stick, under both legacy BIOS and UEFI.
#
# The diagnostic never enters the kernel. What it shows, and how to read a photograph of
# it, is in `crates/zpl-kernel/src/diag/mod.rs` and in the pull request that added it.
#
#   bash scripts/build-diag-iso.sh
#
# Output: artifacts/iso/zpl-diag.iso and zpl-diag.iso.sha256 (both ignored by git).
#
# Linux counterpart of the Limine half of `scripts/build-iso.ps1`, with the same pinned
# Limine release and checksum. Needs: nightly cargo, curl, unzip, a C compiler (for
# Limine's host tool) and xorriso.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Same pin as scripts/build-iso.ps1 -- bump both together.
LIMINE_TAG="v12.2.0"
LIMINE_URL="https://github.com/Limine-Bootloader/Limine/releases/download/${LIMINE_TAG}/limine-binary.zip"
LIMINE_SHA256="6a7d1daaa8fd336dd4dc477db1d2533dc070b9f5cba36926a9be9e3fbb438635"
MAX_ISO_BYTES=$((20 * 1024 * 1024))

OUT_DIR="artifacts/iso"
DIST="$OUT_DIR/limine-dist/$LIMINE_TAG"
STAGE="$OUT_DIR/_staging_diag_iso"
ISO="$OUT_DIR/zpl-diag.iso"
ELF="target/x86_64-zpl-limine/release/limine-bridge"

for tool in cargo curl unzip cc make xorriso sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "[E_DIAG_ISO_001_MISSING_TOOL] $tool is not on PATH." >&2
    exit 2
  fi
done

mkdir -p "$DIST"
ZIP="$DIST/limine-binary.zip"
if [[ ! -f "$ZIP" ]]; then
  echo "==> downloading Limine $LIMINE_TAG"
  curl -fsSL -o "$ZIP" "$LIMINE_URL"
fi
got="$(sha256sum "$ZIP" | cut -d' ' -f1)"
if [[ "$got" != "$LIMINE_SHA256" ]]; then
  echo "[E_DIAG_ISO_002_SHA256] limine-binary.zip: expected $LIMINE_SHA256, got $got" >&2
  exit 1
fi
LIM="$DIST/limine-binary"
if [[ ! -d "$LIM" ]]; then
  unzip -q -o "$ZIP" -d "$DIST"
fi
if [[ ! -x "$LIM/limine" ]]; then
  echo "==> building Limine's host tool"
  make -s -C "$LIM" limine
fi

echo "==> building limine-bridge with hw_diag"
cargo -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec \
  build -p limine-bridge --features hw_diag \
  --target limine-bridge/x86_64-zpl-limine.json --release

rm -rf "$STAGE"
mkdir -p "$STAGE/boot/limine" "$STAGE/EFI/BOOT"
cp "$ELF" "$STAGE/boot/zpl-diag.elf"
cp "$LIM/limine-bios.sys" "$LIM/limine-bios-cd.bin" "$LIM/limine-uefi-cd.bin" "$STAGE/boot/limine/"
# BOOTIA32 as well as BOOTX64: some small x86-64 machines ship 32-bit UEFI, and Limine
# can start a 64-bit kernel from it. On those, an image with only BOOTX64 is not
# bootable at all and the firmware quietly moves on to the next device.
cp "$LIM/BOOTX64.EFI" "$LIM/BOOTIA32.EFI" "$STAGE/EFI/BOOT/"

# A five-second menu instead of none: on a machine that will not boot, the menu
# appearing is the proof that the firmware did start the stick, which is half the
# question for a UEFI machine that ends up back in its installed system.
cat > "$STAGE/boot/limine/limine.conf" <<'EOF'
timeout: 5
default_entry: 1
verbose: yes
serial: yes
interface_branding: ZPL HW DIAG (read-only)

/ZPL hardware diagnostic (read-only)
    protocol: limine
    path: boot():/boot/zpl-diag.elf
EOF

rm -f "$ISO"
xorriso -as mkisofs -R -r -J \
  -b boot/limine/limine-bios-cd.bin -no-emul-boot -boot-load-size 4 -boot-info-table \
  -hfsplus -apm-block-size 2048 \
  --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part --efi-boot-image \
  --protective-msdos-label \
  "$STAGE" -o "$ISO" 2>&1 | grep -v '^xorriso : UPDATE' || true
if [[ ! -s "$ISO" ]]; then
  echo "[E_DIAG_ISO_003_XORRISO] xorriso produced no image." >&2
  exit 1
fi
"$LIM/limine" bios-install "$ISO"

size=$(stat -c %s "$ISO")
if (( size > MAX_ISO_BYTES )); then
  echo "[E_DIAG_ISO_004_TOO_LARGE] $ISO is $size bytes, limit $MAX_ISO_BYTES." >&2
  exit 1
fi
(cd "$OUT_DIR" && sha256sum zpl-diag.iso > zpl-diag.iso.sha256)
echo "==> $ISO ($size bytes)"
cat "$OUT_DIR/zpl-diag.iso.sha256"
