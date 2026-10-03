param(
  [string]$Binary = ".\target\x86_64-zpl-kernel\release\zpl-kernel-bin",
  [string]$Disassembly = "",
  [int]$Window = 8,
  [switch]$Verbose
)

# Gate for one specific class of bug: a `cpuid` whose EBX result is
# thrown away before it is read.
#
# Background. Inline assembly on x86-64 cannot bind an output operand to
# `rbx`, because the compiler reserves it. The usual workaround saves
# `rbx`, runs `cpuid`, copies EBX into a scratch register and restores
# `rbx`. If the scratch register is requested with a constraint that lets
# the register allocator choose freely, the allocator may choose `rbx`
# itself -- and then the copy assembles to `mov %ebx, %ebx`, a no-op, and
# the following `pop %rbx` overwrites the result with the saved value.
# That shipped in this kernel until 26 September 2026 (see
# `docs/V04_TEST_RECORD.md` section 2.2). The source looked correct, the
# host tests passed, the boot printed a plausible number and the
# determinism check was happy, because a stale register is perfectly
# stable. Only the disassembly showed it.
#
# So this gate reads the disassembly, not the source. It is deliberately
# a SEPARATE tool and is NOT wired into the pre-commit hook: it needs a
# built bare-metal artifact, which the hook does not produce.
#
# Two patterns are reported:
#
#   SELF-MOVE  a `mov %R, %R` within the window after a `cpuid`. Always
#              wrong: nobody writes a register to itself on purpose here.
#   LOST-EBX   a `pop %rbx` (or `mov ... , %rbx`) within the window with
#              no earlier instruction in that window copying `ebx`/`rbx`
#              into a different register. The result is discarded.
#
# Usage:
#   scripts\cpuid-clobber-check.ps1
#   scripts\cpuid-clobber-check.ps1 -Binary path\to\elf
#   scripts\cpuid-clobber-check.ps1 -Disassembly dump.txt   # for testing the gate itself
#
# Exit 0 = no suspect site. Exit 1 = suspect site found. Exit 2 = could
# not run (missing binary or disassembler), which is NOT a pass.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-ObjdumpPath {
  $cmd = Get-Command "llvm-objdump" -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  $roots = Get-ChildItem "$env:USERPROFILE\.rustup\toolchains" -Directory -ErrorAction SilentlyContinue
  foreach ($r in $roots) {
    $p = Join-Path $r.FullName "lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-objdump.exe"
    if (Test-Path $p) { return $p }
  }
  return $null
}

# --- get the disassembly text -------------------------------------------------

if ($Disassembly -ne "") {
  if (-not (Test-Path $Disassembly)) {
    Write-Output "[E_CPUID_003_NO_DISASM] disassembly file not found: $Disassembly"
    exit 2
  }
  $lines = Get-Content $Disassembly
  $source = $Disassembly
}
else {
  if (-not (Test-Path $Binary)) {
    Write-Output "[E_CPUID_001_NO_BINARY] binary not found: $Binary"
    Write-Output "Build it first, then re-run. A missing artifact is not a pass."
    exit 2
  }
  $objdump = Get-ObjdumpPath
  if (-not $objdump) {
    Write-Output "[E_CPUID_002_NO_OBJDUMP] llvm-objdump not found (PATH or rustup toolchain)."
    exit 2
  }
  $lines = & $objdump -d --no-show-raw-insn $Binary 2>$null
  $source = $Binary
}

# --- scan ---------------------------------------------------------------------

# One instruction line looks like:  "  106ada:      	movl	%ebx, %ebx"
$insnRe = '^\s*[0-9a-f]+:\s*(?<text>\S.*)$'
$insns = @()
foreach ($l in $lines) {
  $m = [regex]::Match($l, $insnRe)
  if ($m.Success) {
    $insns += ($m.Groups["text"].Value -replace "\s+", " ").Trim()
  }
}

$findings = @()
for ($i = 0; $i -lt $insns.Count; $i++) {
  if ($insns[$i] -notmatch '^cpuid\b') { continue }

  $copiedOut = $false
  $hi = [Math]::Min($i + $Window, $insns.Count - 1)
  for ($j = $i + 1; $j -le $hi; $j++) {
    $t = $insns[$j]

    # Stop at control flow: past a branch we are no longer on this path.
    if ($t -match '^(j[a-z]+|call|ret|cpuid)\b') { break }

    # SELF-MOVE: mov with identical source and destination register.
    $sm = [regex]::Match($t, '^mov[a-z]*\s+%(?<r>[a-z0-9]+),\s*%(?<r2>[a-z0-9]+)$')
    if ($sm.Success -and $sm.Groups["r"].Value -eq $sm.Groups["r2"].Value) {
      $findings += [PSCustomObject]@{
        Kind = "SELF-MOVE"; CpuidAt = $i; Offset = $j - $i; Text = $t
      }
    }

    # Did anything copy ebx/rbx OUT to a different register?
    $mv = [regex]::Match($t, '^(mov[a-z]*|xchg[a-z]*)\s+%(?<src>[re]bx),\s*%(?<dst>[a-z0-9]+)$')
    if ($mv.Success -and $mv.Groups["dst"].Value -notmatch '^[re]bx$') { $copiedOut = $true }
    # `xchg %rbx, %rN` also swaps the result out.
    $xc = [regex]::Match($t, '^xchg[a-z]*\s+%(?<a>[re]bx),\s*%(?<b>[a-z0-9]+)$')
    if ($xc.Success -and $xc.Groups["b"].Value -notmatch '^[re]bx$') { $copiedOut = $true }

    # LOST-EBX: rbx is overwritten and nothing took the result first.
    if ($t -match '^pop[a-z]*\s+%rbx$' -or $t -match '^mov[a-z]*\s+.*,\s*%[re]bx$') {
      if (-not $copiedOut) {
        $findings += [PSCustomObject]@{
          Kind = "LOST-EBX"; CpuidAt = $i; Offset = $j - $i; Text = $t
        }
      }
      break
    }
  }
}

$cpuidCount = ($insns | Where-Object { $_ -match '^cpuid\b' }).Count

Write-Output "==> cpuid-clobber-check on $source"
Write-Output "    instructions: $($insns.Count)   cpuid sites: $cpuidCount"

if ($Verbose) {
  for ($i = 0; $i -lt $insns.Count; $i++) {
    if ($insns[$i] -match '^cpuid\b') {
      $hi = [Math]::Min($i + $Window, $insns.Count - 1)
      Write-Output "    site at index $i :"
      for ($j = $i; $j -le $hi; $j++) { Write-Output "        $($insns[$j])" }
    }
  }
}

if ($cpuidCount -eq 0) {
  Write-Output ""
  Write-Output "No cpuid instruction in this binary. Nothing to check -- reported as such"
  Write-Output "rather than as a pass, because a gate that silently checks nothing is worse"
  Write-Output "than no gate."
  exit 0
}

if ($findings.Count -gt 0) {
  # One site can raise both symptoms -- a self-move is also an overwrite of
  # rbx -- so report findings and distinct sites separately rather than
  # letting the count imply two bugs where there is one.
  # @() so a single unique value is still an array; StrictMode has no
  # `.Count` on a bare scalar.
  $sites = @($findings | Select-Object -ExpandProperty CpuidAt -Unique).Count
  Write-Output ""
  Write-Output "SUSPECT SITES:"
  foreach ($f in $findings) {
    Write-Output ("  [{0}] cpuid at index {1}, +{2} instruction(s): {3}" -f $f.Kind, $f.CpuidAt, $f.Offset, $f.Text)
  }
  Write-Output ""
  Write-Output "[E_CPUID_010_CLOBBER] $($findings.Count) finding(s) at $sites site(s). The EBX result of a"
  Write-Output "cpuid is discarded before it is read. Use core::arch::x86_64::__cpuid rather"
  Write-Output "than hand-written inline assembly."
  exit 1
}

Write-Output ""
Write-Output "cpuid-clobber-check: PASS ($cpuidCount site(s), EBX copied out before rbx is restored)"
exit 0
