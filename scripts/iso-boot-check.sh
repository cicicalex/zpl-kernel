#!/usr/bin/env bash
# Boot the ISO in QEMU, under BIOS and under UEFI, and fail on any self-check FAIL.
#
# `ci-boot-check.sh` boots the `-kernel` build, and the `-kernel` path declares no
# memory map, so a defect that shows only when there is one never reached CI: the
# frame allocator's self-check printed `selfcheck FAIL` on every ISO boot, and no
# gate saw it. This boots the image people actually write to a USB stick.
#
#   bash scripts/iso-boot-check.sh [iso]          boot it; exit 1 on a FAIL
#   bash scripts/iso-boot-check.sh --self-test    prove the log check can fail
#
# Each boot has to reach `[ZPL-BOOT] halt loop entered`, and no line may contain
# `selfcheck FAIL`. Memory is 256 MiB, small enough that the firmware's memory map
# leaves part of the allocator's window reserved, which is the case that failed.
# UEFI runs only where an OVMF image is installed, and says so when it is not.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

BOOT_SECONDS="${BOOT_SECONDS:-120}"
WORK="${ISO_BOOT_CHECK_DIR:-target/iso-boot-check}"
HALT="[ZPL-BOOT] halt loop entered"

# The verdict on one serial log. Prints why, returns non-zero on a failure.
check_log() {
  local log="$1" name="$2"
  if [[ ! -s "$log" ]]; then
    echo "  $name: FAIL, the serial log is empty" >&2
    return 1
  fi
  if grep -aqF -- "selfcheck FAIL" "$log"; then
    echo "  $name: FAIL, a self-check failed:" >&2
    grep -aF -A1 -- "selfcheck FAIL" "$log" | head -n 6 | sed 's/^/    /' >&2
    return 1
  fi
  if ! grep -aqF -- "$HALT" "$log"; then
    echo "  $name: FAIL, the boot never reached its halt loop ($(wc -l < "$log") lines)" >&2
    return 1
  fi
  echo "  $name: PASS, $(grep -ac 'selfcheck' "$log") self-check lines, none FAIL"
}

if [[ "${1:-}" == "--self-test" ]]; then
  mkdir -p "$WORK"
  good="$WORK/self-test-good.log"
  bad="$WORK/self-test-bad.log"
  stuck="$WORK/self-test-stuck.log"
  printf '[ZPL-FRAME] burst alloc=1000 free=1000 leak=0\n[ZPL-HEAP] selfcheck box+vec100 OK\n%s\n' "$HALT" > "$good"
  printf '[ZPL-FRAME] selfcheck FAIL\n[ZPL-FRAME] selfcheck detail leak_after_burst used=198558 reserved=197558\n%s\n' "$HALT" > "$bad"
  printf '[ZPL-HEAP] selfcheck box+vec100 OK\n' > "$stuck"
  ok=1
  check_log "$good" "clean log" >/dev/null || { echo "[E_ISO_BOOT_010] a clean log was rejected" >&2; ok=0; }
  if check_log "$bad" "FAIL log" 2>/dev/null; then echo "[E_ISO_BOOT_011] a FAIL line was not caught" >&2; ok=0; fi
  if check_log "$stuck" "stuck log" 2>/dev/null; then echo "[E_ISO_BOOT_012] a boot that never finished passed" >&2; ok=0; fi
  if (( ok )); then
    echo "iso boot check self-test PASS: clean log passes, a FAIL line fails, an unfinished boot fails"
    exit 0
  fi
  exit 1
fi

ISO="${1:-artifacts/iso/zpl-kernel.iso}"
if [[ ! -f "$ISO" ]]; then
  echo "[E_ISO_BOOT_001_NO_ISO] $ISO is missing; build it with scripts/build-iso.sh." >&2
  exit 2
fi
if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
  echo "[E_ISO_BOOT_002_NO_QEMU] qemu-system-x86_64 is not on PATH." >&2
  exit 2
fi
mkdir -p "$WORK"

OVMF=""
for f in /usr/share/qemu/OVMF.fd /usr/share/ovmf/OVMF.fd /usr/share/OVMF/OVMF.fd; do
  if [[ -f "$f" ]]; then OVMF="$f"; break; fi
done

boot() {
  local name="$1"; shift
  local log="$WORK/$name.log" pid waited=0
  rm -f "$log"
  qemu-system-x86_64 -cdrom "$ISO" "$@" -serial "file:$log" \
    -display none -no-reboot -m 256 >/dev/null 2>&1 &
  pid=$!
  while (( waited < BOOT_SECONDS )); do
    if [[ -s "$log" ]] && grep -aqF -- "$HALT" "$log"; then break; fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
    waited=$(( waited + 1 ))
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  check_log "$log" "$name"
}

echo "==> booting $ISO"
failed=0
boot bios || failed=1
if [[ -n "$OVMF" ]]; then
  boot uefi -bios "$OVMF" || failed=1
else
  echo "  uefi: skipped, no OVMF image installed"
fi

if (( failed )); then
  echo "iso boot check FAIL (serial logs in $WORK)" >&2
  exit 1
fi
echo "iso boot check PASS"
