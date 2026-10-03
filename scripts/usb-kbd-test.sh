#!/usr/bin/env bash
# Type on the kernel's prompt through QEMU's monitor and check that it heard.
#
# The question this answers: does a keystroke reach the command line on a machine
# with no PS/2 controller? Many current mini PCs boot UEFI-only and have no i8042,
# not even an emulated one, so a kernel that reads the keyboard only through ports
# 0x60/0x64 never sees a key there. QEMU can build that machine.
#
# Three machines, each booted with the `shell` build of the kernel:
#
#   ps2      `-machine pc`                     the PS/2 keyboard, as today
#   usb      `-machine q35,i8042=off` + xHCI   a USB keyboard and no i8042 at all
#   usb1     the same, keyboard at full speed  a USB 1.1 keyboard, as most real ones are
#   both     `-machine pc` + xHCI + usb-kbd    both present; QEMU routes keys to USB
#
# On each one the script waits for the prompt, sends `version` and Enter with
# `sendkey`, and looks for the kernel's answer on COM1. COM1 is fed from nothing,
# so the only way the answer can appear is through the keyboard.
#
#   bash scripts/usb-kbd-test.sh [kernel] [machine ...]
#
# `kernel` may also be an ISO image (a name ending in `.iso`), which is booted from a
# CD drive instead of with `-kernel`: that is the Limine path real machines take.
# `QEMU_EXTRA` is added to every QEMU command line, e.g. `-bios <OVMF image>` for UEFI.
#
# Exit status is 0 only when every machine asked for heard the keys.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

KERNEL="${1:-target/shell/x86_64-zpl-kernel/release/zpl-kernel-bin}"
shift || true
MACHINES=("$@")
if (( ${#MACHINES[@]} == 0 )); then
  MACHINES=(ps2 usb usb1 both)
fi
BOOT_SECONDS="${BOOT_SECONDS:-60}"
ANSWER_SECONDS="${ANSWER_SECONDS:-15}"
WORK="${USB_KBD_TEST_DIR:-target/usb-kbd-test}"

if [[ ! -f "$KERNEL" ]]; then
  echo "[E_USB_KBD_001_NO_KERNEL] $KERNEL is missing; build it with --features shell." >&2
  exit 2
fi
if [[ "$KERNEL" == *.iso ]]; then
  BOOT=(-cdrom "$KERNEL")
else
  BOOT=(-kernel "$KERNEL")
fi
if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
  echo "[E_USB_KBD_002_NO_QEMU] qemu-system-x86_64 is not on PATH." >&2
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "[E_USB_KBD_003_NO_PYTHON] python3 is needed to talk to the QEMU monitor." >&2
  exit 2
fi
mkdir -p "$WORK"

machine_args() {
  case "$1" in
    ps2)  echo "-machine pc" ;;
    usb)  echo "-machine q35,i8042=off -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0" ;;
    usb1) echo "-machine q35,i8042=off -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0,usb_version=1" ;;
    both) echo "-machine pc -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0" ;;
    *)    echo "[E_USB_KBD_004_MACHINE] unknown machine: $1" >&2; return 1 ;;
  esac
}

# Send monitor commands, one per line, over the monitor's unix socket.
monitor() {
  python3 - "$1" "${@:2}" <<'PY'
import socket, sys, time
sock_path, cmds = sys.argv[1], sys.argv[2:]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sock_path)
s.settimeout(0.5)
def drain():
    try:
        while s.recv(4096):
            pass
    except OSError:
        pass
drain()
for c in cmds:
    s.sendall((c + "\n").encode())
    time.sleep(0.15)
    drain()
s.close()
PY
}

# Wait for a fixed string in a file, up to a number of seconds, while QEMU lives.
wait_for() {
  local needle="$1" file="$2" seconds="$3" pid="$4" waited=0
  while (( waited < seconds * 4 )); do
    if [[ -s "$file" ]] && grep -qF -- "$needle" "$file"; then
      return 0
    fi
    kill -0 "$pid" 2>/dev/null || return 1
    sleep 0.25
    waited=$(( waited + 1 ))
  done
  return 1
}

run_one() {
  local name="$1" args log mon pid verdict
  args="$(machine_args "$name")"
  log="$WORK/$name.log"
  mon="$WORK/$name.mon"
  rm -f "$log" "$mon"

  # shellcheck disable=SC2086
  qemu-system-x86_64 "${BOOT[@]}" $args ${QEMU_EXTRA:-} \
    -serial "file:$log" -monitor "unix:$mon,server,nowait" \
    -display none -no-reboot -m 256 &
  pid=$!

  verdict="FAIL"
  if wait_for "zpl> " "$log" "$BOOT_SECONDS" "$pid"; then
    # A short pause after the prompt, so a driver that is still bringing a port
    # up when the prompt prints is not what is being measured.
    sleep 1
    monitor "$mon" "sendkey v" "sendkey e" "sendkey r" "sendkey s" \
      "sendkey i" "sendkey o" "sendkey n" "sendkey ret"
    if wait_for "ZPL kernel " "$log" "$ANSWER_SECONDS" "$pid"; then
      verdict="PASS"
    fi
  else
    echo "  ($name: the prompt never appeared)" >&2
  fi

  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true

  echo "  $name: $verdict   [$args]"
  grep -F -- "[ZPL-KBD]" "$log" 2>/dev/null | sed "s/^/      /" || true
  grep -F -- "[ZPL-USB]" "$log" 2>/dev/null | sed "s/^/      /" || true
  [[ "$verdict" == "PASS" ]]
}

echo "==> typing on the prompt of $KERNEL"
failed=0
for m in "${MACHINES[@]}"; do
  if ! run_one "$m"; then
    failed=1
  fi
done

if (( failed )); then
  echo "usb keyboard test FAIL (logs in $WORK)"
  exit 1
fi
echo "usb keyboard test PASS"
