#!/usr/bin/env bash
# Boot the diagnostic ISO in QEMU the way a stick written in Rufus "DD" mode boots: as a
# raw disk. Types on the emulated PS/2 keyboard during the listening windows, takes
# screenshots, and checks the serial log for the lines that say each part worked.
#
#   bash scripts/diag-qemu.sh bios|uefi [out-dir]
#
# Writes, in out-dir (default artifacts/diag-qemu):
#   <mode>-serial.log            everything the diagnostic sent to COM1
#   <mode>-window-a.png          screen during window A, after a few keys
#   <mode>-final.png             screen in window C, the "photograph"
#
# QEMU has a real (emulated) i8042 and no firmware USB keyboard emulation, so keys
# arrive the way they would from a PS/2 keyboard. What this proves is that the image
# boots in both modes, every step runs, and the counters count; what a given laptop's
# firmware does with a USB keyboard is what the image is for, and no emulator answers it.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

MODE="${1:-}"
OUT="${2:-artifacts/diag-qemu}"
ISO="artifacts/iso/zpl-diag.iso"
OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}"
OVMF_VARS="${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.fd}"

if [[ "$MODE" != "bios" && "$MODE" != "uefi" ]]; then
  echo "usage: $0 bios|uefi [out-dir]" >&2
  exit 2
fi
if [[ ! -f "$ISO" ]]; then
  echo "[E_DIAG_QEMU_001_NO_ISO] $ISO is missing; run scripts/build-diag-iso.sh." >&2
  exit 2
fi

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
LOG="$OUT/$MODE-serial.log"
SOCK="$OUT/$MODE-monitor.sock"
rm -f "$LOG" "$SOCK"

# Copy the image so the emulated disk can never write back into the artifact.
DISK="$OUT/$MODE-disk.img"
cp "$ISO" "$DISK"

args=(
  -m 512
  -drive "file=$DISK,format=raw,if=ide,index=0,media=disk"
  -serial "file:$LOG"
  -monitor "unix:$SOCK,server,nowait"
  -display none
  -vga std
  -no-reboot
)
if [[ "$MODE" == "uefi" ]]; then
  if [[ ! -f "$OVMF_CODE" ]]; then
    echo "[E_DIAG_QEMU_002_NO_OVMF] $OVMF_CODE is missing (package ovmf)." >&2
    exit 2
  fi
  cp "$OVMF_VARS" "$OUT/uefi-vars.fd"
  args+=(
    -machine q35
    -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
    -drive "if=pflash,format=raw,file=$OUT/uefi-vars.fd"
    -device ich9-usb-uhci1 -device ich9-usb-ehci1 -device qemu-xhci
  )
else
  args+=(
    -machine pc
    -device piix3-usb-uhci -device usb-ehci -device qemu-xhci
  )
fi

echo "==> qemu ($MODE), serial -> $LOG"
qemu-system-x86_64 "${args[@]}" &
qemu_pid=$!
trap 'kill $qemu_pid 2>/dev/null || true' EXIT

# Drives the monitor: waits for log lines, types keys, takes screenshots. Python only
# because talking to a Unix socket and writing a PNG from bash would be worse.
python3 - "$SOCK" "$LOG" "$OUT" "$MODE" <<'PY'
import os, socket, sys, time, zlib, struct

sock_path, log, out, mode = sys.argv[1:5]

def wait_for(text, timeout):
    end = time.time() + timeout
    while time.time() < end:
        try:
            with open(log, 'rb') as f:
                if text.encode() in f.read():
                    return True
        except FileNotFoundError:
            pass
        time.sleep(0.5)
    return False

for _ in range(100):
    if os.path.exists(sock_path):
        break
    time.sleep(0.1)
mon = socket.socket(socket.AF_UNIX)
mon.connect(sock_path)
mon.settimeout(2)

def hmp(cmd):
    mon.sendall((cmd + '\n').encode())
    time.sleep(0.3)
    try:
        mon.recv(65536)
    except socket.timeout:
        pass

def png(ppm_path, png_path):
    data = open(ppm_path, 'rb').read()
    parts = data.split(b'\n', 3)
    w, h = map(int, parts[1].split())
    pix = parts[3]
    raw = b''.join(b'\x00' + pix[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def chunk(t, d):
        return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    with open(png_path, 'wb') as f:
        f.write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                + chunk(b'IDAT', zlib.compress(raw, 6)) + chunk(b'IEND', b''))
    os.remove(ppm_path)

def shot(name):
    ppm = os.path.join(out, f'{mode}-{name}.ppm')
    hmp(f'screendump {ppm}')
    time.sleep(1)
    png(ppm, os.path.join(out, f'{mode}-{name}.png'))

def type_keys(keys):
    for k in keys:
        hmp(f'sendkey {k}')
        time.sleep(0.2)

ok = True
if not wait_for('window A: listen', 120):
    print('window A never started'); ok = False
else:
    time.sleep(1)
    type_keys(['a', 'b', 'c'])
    shot('window-a')
if ok and wait_for('window B: listen', 60):
    time.sleep(1)
    type_keys(['1', '2'])
else:
    print('window B never started'); ok = False
if ok and wait_for('window C: listen', 60):
    time.sleep(2)
    type_keys(['z'])
    time.sleep(2)
    shot('final')
else:
    print('window C never started'); ok = False
sys.exit(0 if ok else 1)
PY
driver_status=$?
kill "$qemu_pid" 2>/dev/null || true
wait "$qemu_pid" 2>/dev/null || true
trap - EXIT
rm -f "$SOCK" "$DISK" "$OUT/uefi-vars.fd"
if (( driver_status != 0 )); then
  echo "[E_DIAG_QEMU_003_STALLED] the diagnostic did not reach window C; see $LOG" >&2
  exit 1
fi

# What a working run says. Substrings, because the numbers differ by machine.
required=(
  "[01] COM1 serial 115200 8N1 -> ok"
  "[02] exception catcher (IDT, 32 vectors) -> ok"
  "framebuffer -> "
  "bpp, text x"
  "memory map -> "
  "ACPI FADT 8042 flag -> 8042="
  "i8042 status 0x64 -> 0x"
  "window A: listen 10 s, nothing sent -> kbd 6, aux 0 bytes"
  "PCI scan: USB controllers -> "
  "window B: listen 10 s after PCI scan -> kbd 4, aux 0 bytes"
  "8042 cmd 0xAA self-test -> 0x55 passed"
  "window C: listen forever"
  "0x60 window A kbd byte 0x1e"
  "0x60 window B kbd byte 0x02"
  "0x60 window C kbd byte 0x2c"
)
if [[ "$MODE" == "uefi" ]]; then
  required+=("boot mode (firmware type) -> UEFI 64-bit")
else
  required+=("boot mode (firmware type) -> BIOS (legacy)")
fi
missing=0
for line in "${required[@]}"; do
  if ! grep -qF -- "$line" "$LOG"; then
    echo "[E_DIAG_QEMU_004_MISSING] not in the serial log: $line" >&2
    missing=1
  fi
done
if grep -qF -- "CPU EXCEPTION" "$LOG"; then
  echo "[E_DIAG_QEMU_005_EXCEPTION] the diagnostic caught a CPU exception:" >&2
  grep -F -- "CPU EXCEPTION" -A1 "$LOG" >&2
  missing=1
fi
if (( missing )); then
  exit 1
fi
echo "diag qemu ($MODE) PASS"
echo "  serial     : $LOG"
echo "  screenshots: $OUT/$MODE-window-a.png $OUT/$MODE-final.png"
