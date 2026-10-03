#!/usr/bin/env bash
# Boot the kernel in QEMU and check what it said.
#
# Three things, in order, and any one of them failing fails the script:
#
#   1. the boot reaches its halt loop, having emitted every marker a healthy
#      boot emits;
#   2. no marker anywhere says FAIL;
#   3. the demo policy's three rules are all visible (delegated to
#      `scripts/demo-policy-test.sh`, which owns that check).
#
# Used by CI and runnable by hand with exactly the same effect, which is the
# point: a gate you cannot run yourself is a gate you cannot debug.
#
#   bash scripts/ci-boot-check.sh [output-log]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

KERNEL="target/x86_64-zpl-kernel/release/zpl-kernel-bin"
LOG="${1:-boot.log}"
# The kernel never exits on its own: it ends in a halt loop, which is correct for
# a kernel and awkward for a script, so it is stopped from the outside. This is
# the ceiling, not the normal wait -- see below.
BOOT_SECONDS="${BOOT_SECONDS:-90}"

if [[ ! -f "$KERNEL" ]]; then
  echo "[E_CI_BOOT_001_NO_KERNEL] $KERNEL is missing; build it first." >&2
  exit 2
fi
if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
  echo "[E_CI_BOOT_002_NO_QEMU] qemu-system-x86_64 is not on PATH." >&2
  exit 2
fi

rm -f "$LOG"
echo "==> booting $KERNEL, up to ${BOOT_SECONDS}s"

# Stopped as soon as it says it has finished, rather than after a fixed wait.
# Waiting the timeout out every time cost a minute and a half per run for a boot
# that takes five seconds. The timeout is still here; it is now the failure path
# rather than the normal one.
set +e
qemu-system-x86_64 \
  -kernel "$KERNEL" \
  -serial "file:$LOG" \
  -display none -no-reboot -m 256 &
qemu_pid=$!

waited=0
while (( waited < BOOT_SECONDS )); do
  if [[ -s "$LOG" ]] && grep -qF -- "[ZPL-BOOT] halt loop entered" "$LOG"; then
    break
  fi
  if ! kill -0 "$qemu_pid" 2>/dev/null; then
    break          # QEMU stopped by itself; the marker checks below say why
  fi
  sleep 1
  waited=$(( waited + 1 ))
done

kill "$qemu_pid" 2>/dev/null
wait "$qemu_pid" 2>/dev/null
qemu_status=$?
set -e
echo "==> stopped after ${waited}s"

if [[ ! -s "$LOG" ]]; then
  echo "[E_CI_BOOT_003_EMPTY_LOG] QEMU exited ${qemu_status} and wrote nothing." >&2
  exit 1
fi

# The markers a healthy boot always emits. Substrings rather than whole lines,
# because several of them carry numbers that are allowed to move.
required=(
  "[ZPL-BOOT] kernel_entry reached"
  "[ZPL-FRAME] stress="
  "[ZPL-HEAP] selfcheck"
  "[ZPL-AUDIT] selfcheck"
  "[ZPL-PAGING] write_read_match"
  "[ZPL-RING] tss installed"
  "[ZPL-STACKGUARD] guard_hit"
  "[ZPL-RAMFS] selfcheck"
  "[ZPL-PROC] selfcheck"
  "[ZPL-IPC] selfcheck"
  "[ZPL-SHM] selfcheck allow_clean+block_hostile OK"
  "[ZPL-MATRIX] selfcheck"
  "[ZPL-RUNQ] selfcheck"
  "[ZPL-HAL] selfcheck"
  "[ZPL-RING3] demo programs done"
  "[ZPL-BOOT] halt loop entered"
)

missing=0
for marker in "${required[@]}"; do
  if ! grep -qF -- "$marker" "$LOG"; then
    echo "[E_CI_BOOT_004_MISSING_MARKER] not in the log: $marker" >&2
    missing=1
  fi
done
if (( missing )); then
  echo "  QEMU exited ${qemu_status}; $(wc -l < "$LOG") lines were captured." >&2
  exit 1
fi

# A self-check that fails says so in capitals, and nothing that passes contains
# the word. Reported with the offending lines, because "something failed" without
# saying what is not a useful gate.
if grep -qF -- "FAIL" "$LOG"; then
  echo "[E_CI_BOOT_005_FAIL_MARKER] a self-check reported FAIL:" >&2
  grep -F -- "FAIL" "$LOG" | head -n 10 >&2
  exit 1
fi

echo "boot check PASS"
echo "  markers      : ${#required[@]}/${#required[@]} present, no FAIL"
echo "  lines        : $(wc -l < "$LOG")"

# Rules 1, 2 and 3 of the demo policy. Kept in its own script so there is one
# place that knows what the policy promises.
bash scripts/demo-policy-test.sh "$LOG"
