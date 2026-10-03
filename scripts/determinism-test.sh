#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

mkdir -p artifacts/kernel

# Bounded runs. This kernel does not stop on its own -- its scheduler keeps deciding,
# which is exactly the thing being compared -- so each run is cut off after a fixed
# number of seconds. Before, the loop waited for a guest that never exited and the whole
# test hung; it only ever reached a verdict on machines without QEMU, where the caller
# skipped it. A serial log of several megabytes was the only sign anything was wrong.
RUN_SECONDS="${ZPL_DETERMINISM_RUN_SECONDS:-25}"

for i in 1 2 3; do
  pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -File "scripts/qemu-run.ps1" -TimeoutSeconds "$RUN_SECONDS" >/dev/null
  latest_log="$(ls -1t artifacts/kernel/boot-run-*.log | head -n 1)"
  grep "\[ZPL-SCHED" "$latest_log" | head -n 100 > "artifacts/kernel/determinism-${i}.txt"
done

# Three empty files are identical, so count before comparing: a run that produced fewer
# than 100 decision lines would make the comparison say nothing while looking like a pass.
for i in 1 2 3; do
  n="$(wc -l < "artifacts/kernel/determinism-${i}.txt")"
  if [ "$n" -lt 100 ]; then
    echo "run ${i} produced only ${n} decision lines; raise ZPL_DETERMINISM_RUN_SECONDS" >&2
    exit 1
  fi
done

cmp -s artifacts/kernel/determinism-1.txt artifacts/kernel/determinism-2.txt
cmp -s artifacts/kernel/determinism-1.txt artifacts/kernel/determinism-3.txt

echo "Determinism PASS (first 100 ZPL-SCHED lines identical in 3 runs)"
