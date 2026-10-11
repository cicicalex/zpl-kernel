#!/usr/bin/env bash
# Scan built images for things that must not travel in them: paths from the machine
# that built them, account names, e-mail addresses, and credential-shaped strings.
#
#   bash scripts/diag-leak-scan.sh FILE...
#
# Reads every printable run of bytes, in 8-bit and in UTF-16 (the EFI loaders keep
# their strings in UTF-16), so it also sees inside the FAT image the ISO embeds.
#
# The building machine's own home directory and account name are taken from the
# environment at run time rather than written here: this file is published, and a
# list of names to look for would be the very leak it is meant to catch.
#
# Before scanning, each pattern list is run once against a planted sample that must
# match, so a broken pattern fails loudly instead of passing everything.
set -euo pipefail

if (( $# == 0 )); then
  echo "usage: $0 FILE..." >&2
  exit 2
fi

patterns=(
  '/home/[A-Za-z0-9._-]+'
  '/Users/[A-Za-z0-9._-]+'
  '/root/'
  '[A-Za-z]:\\Users\\'
  '\.rustup/'
  '\.cargo/(registry|git)'
  '/tmp/'
  'ghp_[A-Za-z0-9]{20,}'
  'gho_[A-Za-z0-9]{20,}'
  'github_pat_[A-Za-z0-9_]{20,}'
  'AKIA[0-9A-Z]{16}'
  'BEGIN [A-Z ]*PRIVATE KEY'
  '[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.[A-Za-z0-9.-]*[a-z]{2,}'
)
me="$(id -un 2>/dev/null || true)"
# Short or generic account names ("root", "runner") would match ordinary words, so
# only names that are long and not one of those are added.
if [[ -n "$me" && ${#me} -ge 5 && "$me" != "runner" ]]; then
  patterns+=("$me")
fi
if [[ -n "${HOME:-}" && "$HOME" != "/" ]]; then
  patterns+=("$(printf '%s' "$HOME" | sed 's/[.[\*^$/]/\\&/g')")
fi
regex="$(IFS='|'; echo "${patterns[*]}")"

# Positive control: each kind of leak, planted, must be found.
controls=(
  '/home/someone/project/src/main.rs'
  'C:\Users\someone\.cargo\registry'
  'ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123'
  'someone@example.com'
)
for c in "${controls[@]}"; do
  if ! printf '%s\n' "$c" | grep -qaE -- "$regex"; then
    echo "[E_LEAK_SCAN_001_CONTROL] the patterns do not match the planted sample: $c" >&2
    exit 2
  fi
done

found=0
for f in "$@"; do
  if [[ ! -f "$f" ]]; then
    echo "[E_LEAK_SCAN_002_NO_FILE] $f is missing." >&2
    exit 2
  fi
  hits="$( { strings -a -n 4 "$f"; strings -a -n 4 -e l "$f"; } | grep -aE -- "$regex" | sort -u || true)"
  if [[ -n "$hits" ]]; then
    echo "[E_LEAK_SCAN_003_FOUND] $f carries:" >&2
    printf '%s\n' "$hits" | head -n 20 >&2
    found=1
  else
    echo "leak scan PASS  $f ($(stat -c %s "$f") bytes, ${#patterns[@]} patterns)"
  fi
done
exit "$found"
