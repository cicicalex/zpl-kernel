#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Bounded, for the same reason `determinism-test.sh` is: this kernel does not exit
# on its own, so a caller that simply waits waits forever. The PowerShell callers
# of `qemu-run.ps1` already bound it themselves; this one did not, and because
# `full-verify.ps1` prefers the bash path when bash is present, the whole
# verification hung here -- with a serial log quietly growing past two megabytes
# as the only sign. Fixed in `determinism-test.sh` first and not here, which is
# how one of two callers stays broken.
RUN_SECONDS="${ZPL_SMOKE_RUN_SECONDS:-25}"

pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -File "scripts/qemu-run.ps1" -TimeoutSeconds "$RUN_SECONDS" >/dev/null

LATEST_LOG="$(ls -1t artifacts/kernel/boot-run-*.log 2>/dev/null | head -n 1 || true)"
if [[ -z "${LATEST_LOG}" ]]; then
  echo "[E_QEMU_BOOT_LOG] no boot-run log found"
  exit 1
fi

grep -q "\[ZPL-BOOT\] kernel_entry reached" "$LATEST_LOG"
grep -q "\[ZPL-BOOT\] serial initialized" "$LATEST_LOG"
grep -q "\[ZPL-BOOT\] halt loop entered" "$LATEST_LOG"
grep -q "\[ZPL-SCHED tid=" "$LATEST_LOG"
# Shape, not value. This used to assert the exact text `cap=262144`, which was a
# fixed string in the kernel and therefore always true -- the gate was pinned to a
# number that could not change and did not mean anything. The marker now reports
# what the machine offers, which differs with the memory map and with QEMU -m, so
# what is worth asserting is that it is there and carries a number at all.
grep -Eq "\[ZPL-FRAME\] init base=0x[0-9a-f]+ cap=[0-9]+" "$LATEST_LOG"
grep -q "\[ZPL-FRAME\] burst alloc=1000 free=1000 leak=0" "$LATEST_LOG"
grep -q "\[ZPL-FRAME\] stress=1000000 leak=0" "$LATEST_LOG"
grep -q "\[ZPL-HEAP\] selfcheck box+vec100 OK" "$LATEST_LOG"
grep -q "\[ZPL-AUDIT\] selfcheck append=5 verify=ok tamper_detected OK" "$LATEST_LOG"
grep -q "\[ZPL-SMP max_leaf=" "$LATEST_LOG"
grep -q "\[ZPL-PERF phase=0 name=kernel_entry" "$LATEST_LOG"
grep -q "\[ZPL-PAGING\] init pdpt\[1\]+pd\[0\] ok" "$LATEST_LOG"
grep -q "\[ZPL-PAGING\] map_4k virt=0x40000000 ok" "$LATEST_LOG"
grep -q "\[ZPL-PAGING\] write_read_match val=0xcafebabedeadbeef" "$LATEST_LOG"
grep -q "\[ZPL-PAGING\] unmap virt=0x40000000 ok" "$LATEST_LOG"
grep -q "\[ZPL-PAGING\] page_fault virt=0x40000000 reread ok" "$LATEST_LOG"
grep -q "\[ZPL-RAMFS\] selfcheck create+write+close+reopen+read OK" "$LATEST_LOG"
grep -q "\[ZPL-PROC\] selfcheck spawn=3 wait=3 exit_codes_sum=43 live=0 OK" "$LATEST_LOG"
grep -q "\[ZPL-IPC\] selfcheck pipe+send+recv ipc-roundtrip OK" "$LATEST_LOG"
grep -q "\[ZPL-SHM\] selfcheck allow_clean+block_hostile OK" "$LATEST_LOG"
grep -q "\[ZPL-MATRIX\] selfcheck 8ops naive==simd 64x64 OK" "$LATEST_LOG"
grep -q "\[ZPL-RUNQ\] selfcheck N=5 ticks=10 winner=task0_wins=10 OK" "$LATEST_LOG"
grep -q "\[ZPL-HAL\] selfcheck block_rw+net_tx_rx OK" "$LATEST_LOG"
grep -q "\[ZPL-VNET\] probe" "$LATEST_LOG"

echo "QEMU boot smoke PASS ($LATEST_LOG)"
