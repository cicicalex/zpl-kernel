#!/usr/bin/env bash
# Build the committed tree twice, from clean, in two different directories, and
# compare the SHA-256 of what comes out.
#
# Two directories, not one built twice: a build that is the same only when it runs
# from the same path is a build that still carries the path somewhere. The second
# directory's name is longer than the first, so a path that leaked in would also
# shift every offset after it.
#
# Compared: the `-kernel` ELF, the Limine ELF, and the ISO. Each output is also run
# through `scripts/path-leak-gate.py`.
#
#   bash scripts/repro-check.sh [work-dir]
#
# Builds HEAD as committed: uncommitted changes are not in the two checkouts.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Outside the repository, on purpose: cargo reads every `.cargo/config.toml` from the
# build directory up to the root, so a checkout nested inside this one would pick up
# both copies, pass the linker script twice, and fail to link.
WORK="${1:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/zpl-repro-check}"
case "$WORK" in
  "$ROOT_DIR"/*)
    echo "[E_REPRO_SH_001_NESTED] $WORK is inside the repository; pick a directory outside it" >&2
    exit 2 ;;
esac
A="$WORK/a"
B="$WORK/second-checkout-with-a-longer-path"
EPOCH="$(git log -1 --format=%ct)"
REV="$(git rev-parse HEAD)"

cleanup() {
  git worktree remove --force "$A" >/dev/null 2>&1 || true
  git worktree remove --force "$B" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup
rm -rf "$WORK"
mkdir -p "$WORK"

build_in() {
  local dir="$1"
  git worktree add --detach "$dir" "$REV" >/dev/null
  # The Limine zip is checked against its pinned SHA-256 by build-iso.sh, so a copy
  # already fetched can be shared rather than downloaded twice.
  if [[ -d artifacts/iso/limine-dist ]]; then
    mkdir -p "$dir/artifacts/iso"
    cp -r artifacts/iso/limine-dist "$dir/artifacts/iso/"
  fi
  (
    cd "$dir"
    unset CARGO_TARGET_DIR
    echo "==> clean build in $dir"
    cargo -Zbuild-std=core,compiler_builtins,alloc \
      -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec \
      build -p zpl-kernel-bin --target crates/zpl-kernel/x86_64-zpl-kernel.json \
      --release 2>&1 | tail -n 1
    SOURCE_DATE_EPOCH="$EPOCH" bash scripts/build-iso.sh >/dev/null
  )
}

build_in "$A"
build_in "$B"

OUTPUTS=(
  target/x86_64-zpl-kernel/release/zpl-kernel-bin
  target/x86_64-zpl-limine/release/limine-bridge
  artifacts/iso/zpl-kernel.iso
)

echo "==> SHA-256, commit ${REV:0:12}, SOURCE_DATE_EPOCH=$EPOCH"
differ=0
for out in "${OUTPUTS[@]}"; do
  ha="$(sha256sum "$A/$out" | cut -d' ' -f1)"
  hb="$(sha256sum "$B/$out" | cut -d' ' -f1)"
  if [[ "$ha" == "$hb" ]]; then
    echo "  same     $ha  $out"
  else
    echo "  DIFFERS  $out"
    echo "           a: $ha"
    echo "           b: $hb"
    cmp -l "$A/$out" "$B/$out" | head -n 8 | sed 's/^/           /' || true
    differ=1
  fi
done

gate_files=()
for out in "${OUTPUTS[@]}"; do
  gate_files+=("$A/$out" "$B/$out")
done
python3 -I scripts/path-leak-gate.py "${gate_files[@]}"

if (( differ )); then
  echo "repro check FAIL: two clean builds of one commit differ" >&2
  exit 1
fi
echo "repro check PASS: two clean builds from two directories are byte-identical"
