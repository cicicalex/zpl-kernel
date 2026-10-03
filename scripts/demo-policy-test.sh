#!/usr/bin/env bash
# Public-build gate: the demo policy's three rules must be visible in a boot.
#
# This replaces the bit-identity gate, which compared the kernel against a
# userspace reference that is not part of the public tree. There is nothing to
# compare here, and pretending otherwise would be a gate that checks nothing.
# What the public build does promise is the demo policy documented in
# `crates/zpl-kernel/src/zpl_policy.rs`, and that is what this asserts:
#
#   rule 1  a request within the demo budget is allowed      -> at least one ALLOW
#   rule 2  a request over the demo limit is refused          -> at least one BLOCK
#   rule 3  after the per-boot quota, requests are throttled  -> DEGRADE once the
#           boot has made more requests than the quota
#
# It also refuses to pass on a log with no decisions in it, and on any decision
# word other than the three the policy can produce.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Keep in step with DEMO_REQUESTS_PER_BOOT in zpl_policy.rs.
QUOTA=256

LOG="${1:-}"
if [[ -z "$LOG" ]]; then
  LOG="$(ls -1t artifacts/kernel/boot-run-*.log 2>/dev/null | head -n 1 || true)"
fi
if [[ -z "$LOG" || ! -f "$LOG" ]]; then
  echo "[E_DEMO_001_NO_LOG] no boot log; run scripts/qemu-run.ps1 first." >&2
  exit 2
fi

total="$(grep -c 'action=' "$LOG" || true)"
allowed="$(grep -c 'action=ALLOW' "$LOG" || true)"
degraded="$(grep -c 'action=DEGRADE' "$LOG" || true)"
blocked="$(grep -c 'action=BLOCK' "$LOG" || true)"

if [[ "$total" -eq 0 ]]; then
  echo "[E_DEMO_002_NO_DECISIONS] the boot produced no decisions at all." >&2
  echo "A gate that silently checks nothing is worse than no gate." >&2
  exit 1
fi

if (( allowed + degraded + blocked != total )); then
  echo "[E_DEMO_003_UNKNOWN_DECISION] $total decisions, but only" >&2
  echo "  $((allowed + degraded + blocked)) are ALLOW/DEGRADE/BLOCK." >&2
  grep 'action=' "$LOG" | grep -vE 'action=(ALLOW|DEGRADE|BLOCK)' | head -n 5 >&2
  exit 1
fi

fail=0
if [[ "$allowed" -eq 0 ]]; then
  echo "[E_DEMO_004_NO_ALLOW] rule 1 not demonstrated: nothing was allowed." >&2
  fail=1
fi
if [[ "$blocked" -eq 0 ]]; then
  echo "[E_DEMO_005_NO_BLOCK] rule 2 not demonstrated: nothing was refused." >&2
  fail=1
fi
if (( total > QUOTA )) && [[ "$degraded" -eq 0 ]]; then
  echo "[E_DEMO_006_NO_THROTTLE] rule 3 not demonstrated: $total decisions is past" >&2
  echo "  the quota of $QUOTA, so some should have been throttled." >&2
  fail=1
fi
if (( fail )); then
  exit 1
fi

echo "Demo policy gate PASS"
echo "  rule 1, within budget   : $allowed ALLOW"
echo "  rule 2, over the limit  : $blocked BLOCK"
echo "  rule 3, past the quota  : $degraded DEGRADE (quota $QUOTA, $total requests)"
